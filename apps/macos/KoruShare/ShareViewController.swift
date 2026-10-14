// SPDX-License-Identifier: MPL-2.0

import AppKit
import Foundation
import UniformTypeIdentifiers

/// Koru's row in the macOS share sheet — ADR-137's iOS extension, ported.
///
/// **This process never opens the Store.** One SQLite root, one iroh endpoint,
/// one process (ADR-136): a share extension is a second process, so it stages
/// what was shared into an inbox directory, activates Koru, and finishes. The
/// app drains the inbox on activation and imports through its ordinary file
/// importer. Everything this file does is copy bytes and write one JSON file.
///
/// The format is not defined here. `ShareInbox.swift` is compiled into this
/// target straight out of `apps/ios/KoruShared/`, unmodified — the same
/// file iOS's extension and iOS's app compile — so there is exactly one
/// spelling of the manifest, the sweep, the commit order and the provider
/// rules across three products. The Rust reader is `crates/koru-share-inbox`,
/// pinned against iOS's own fixture.
///
/// Two things differ from iOS, and only two:
///
/// 1. **The inbox is a plain directory under the channel root**, not an App
///    Group container. See `inboxRoot()`.
/// 2. **There is no destination picker.** macOS has no `destinations.json`
///    exporter, so `workspaceId` is never written and the app imports into the
///    open workspace — the same place a dragged file lands. TECHDEBT 83.
/// **`@objc(ShareViewController)` is load-bearing.** `NSExtensionPrincipalClass`
/// is looked up through the Objective-C runtime, and a Swift class's runtime
/// name is `<module>.<class>` — Xcode writes `$(PRODUCT_MODULE_NAME).…` into
/// the plist for exactly this reason. Naming the class explicitly unties the
/// plist `koru-release` writes from the `-module-name` two files away, so
/// renaming one cannot silently produce an extension the system will not load.
@objc(ShareViewController)
final class ShareViewController: NSViewController {
    private let label = NSTextField(labelWithString: "Saving to Koru…")

    /// No storyboard, so `loadView` is where the view comes from. It is
    /// deliberately almost nothing: the sheet shows this for the moment the
    /// copy takes, and a picker would be the only reason to draw more.
    override func loadView() {
        let view = NSView(frame: NSRect(x: 0, y: 0, width: 280, height: 72))
        label.alignment = .center
        label.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(label)
        NSLayoutConstraint.activate([
            label.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            label.centerYAnchor.constraint(equalTo: view.centerYAnchor),
        ])
        self.view = view
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        stage()
    }

    // ------------------------------------------------------------ the inbox

    /// The channel this build of the extension belongs to, written into the
    /// bundle's `Info.plist` by `koru-release` at package time.
    ///
    /// Baked in rather than discovered: five channels install side by side,
    /// each with its own appex, and an extension that guessed the channel
    /// would stage into another build's inbox. `internal-debug` is the value a
    /// hand-registered development copy carries, matching what an un-stamped
    /// `koru-channel` reports.
    private static var channelSegment: String {
        Bundle.main.object(forInfoDictionaryKey: "KoruChannelSegment") as? String
            ?? "internal-debug"
    }

    /// `<channel root>/share-inbox`, resolved the way `koru_channel` resolves
    /// it — and, because this process may be sandboxed, through Foundation
    /// rather than through a hand-built `$HOME` path.
    ///
    /// **Two facts shape this, and one of them is not settled.**
    ///
    /// macOS keeps a *sticky legacy root*: `koru_channel::channel_root_of`
    /// prefers `~/Library/Application Support/Koru/<segment>` but stays on
    /// `$XDG_DATA_HOME/Koru/<segment>` when that one exists and the preferred
    /// one does not. Rather than reimplement that rule and let the two copies
    /// drift, this asks a simpler question with the same answer: **which
    /// candidate already holds a `share-inbox`?** The app creates it at
    /// startup, so the directory's existence *is* the app's answer. Only a
    /// machine where Koru has never run falls through to the preferred root.
    ///
    /// And a macOS app extension may be sandboxed even though its containing
    /// app is not — the plan assumes not, and it cannot be proven either way
    /// without a signed install. `FileManager`'s Application Support URL is
    /// correct under both: it is the real one when it is not sandboxed, and
    /// the extension's own container when it is. The app looks in both places
    /// (`koru_share_inbox::every_inbox`), so neither answer loses a share.
    private static func inboxRoot() -> URL? {
        let segment = channelSegment
        let manager = FileManager.default
        var candidates: [URL] = []
        if let support = manager.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
        {
            candidates.append(support.appendingPathComponent("Koru/\(segment)", isDirectory: true))
        }
        let environment = ProcessInfo.processInfo.environment
        let xdg =
            environment["XDG_DATA_HOME"].map { URL(fileURLWithPath: $0) }
            ?? URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent(
                ".local/share", isDirectory: true)
        candidates.append(xdg.appendingPathComponent("Koru/\(segment)", isDirectory: true))

        let named = { (root: URL) in root.appendingPathComponent("share-inbox", isDirectory: true) }
        for candidate in candidates where manager.fileExists(atPath: named(candidate).path) {
            return named(candidate)
        }
        return candidates.first.map(named)
    }

    // ----------------------------------------------------------- the staging

    private func stage() {
        let items = extensionContext?.inputItems as? [NSExtensionItem] ?? []
        let subject = items.first?.attributedTitle?.string
        guard let root = Self.inboxRoot(), !items.isEmpty else {
            finish(cancelled: true)
            return
        }
        let inbox = ShareInbox(root: root)
        Task {
            do {
                let entry = try inbox.begin()
                var files: [ShareInbox.File] = []
                for item in items {
                    let providers = item.attachments ?? []
                    // Providers on one item are representations of *one thing*
                    // — Safari offers a page as both `public.url` and
                    // `public.plain-text`. The rule is shared with both phones
                    // and lives in `ShareInbox.swift`.
                    let chosen = stagedProviderIndices(providers.map(\.registeredTypeIdentifiers))
                    for index in chosen {
                        if let file = try await Self.stage(
                            providers[index], subject: subject, into: entry)
                        {
                            files.append(file)
                        }
                    }
                }
                guard !files.isEmpty else {
                    try? FileManager.default.removeItem(at: entry)
                    finish(cancelled: true)
                    return
                }
                // Payloads first, `manifest.json` last: a half-written entry
                // must never look complete. `workspaceId` stays nil — macOS
                // has no picker, so the app asks nothing and imports into the
                // open workspace.
                try inbox.commit(entry, manifest: ShareInbox.Manifest(files: files))
                Self.activateKoru()
                finish(cancelled: false)
            } catch {
                finish(cancelled: true)
            }
        }
    }

    /// One provider becomes at most one staged payload. The dispatch order is
    /// iOS's, line for line: a file that is also a URL stages as the file.
    private static func stage(
        _ provider: NSItemProvider, subject: String?, into entry: URL
    ) async throws -> ShareInbox.File? {
        if provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier) {
            return try await stageFile(provider, into: entry)
        }
        if provider.hasItemConformingToTypeIdentifier(UTType.url.identifier),
            let url = try? await provider.loadItem(forTypeIdentifier: UTType.url.identifier) as? URL
        {
            return try stageText(
                SharedText.normalize(url.absoluteString, subject: subject), into: entry)
        }
        if provider.hasItemConformingToTypeIdentifier(UTType.html.identifier),
            let html = try? await provider.loadItem(forTypeIdentifier: UTType.html.identifier)
                as? String
        {
            return try stageText(
                SharedText.normalize(html, subject: subject, html: true), into: entry)
        }
        if provider.hasItemConformingToTypeIdentifier(UTType.plainText.identifier),
            let text = try? await provider.loadItem(forTypeIdentifier: UTType.plainText.identifier)
                as? String
        {
            return try stageText(SharedText.normalize(text, subject: subject), into: entry)
        }
        if provider.hasItemConformingToTypeIdentifier(UTType.data.identifier) {
            return try await stageFile(provider, into: entry)
        }
        return nil
    }

    private static func stageFile(
        _ provider: NSItemProvider, into entry: URL
    ) async throws -> ShareInbox.File? {
        let type =
            provider.registeredTypeIdentifiers
            .compactMap(UTType.init)
            .first { $0.conforms(to: .data) } ?? .data
        let staged: URL? = try await withCheckedThrowingContinuation { continuation in
            provider.loadFileRepresentation(forTypeIdentifier: type.identifier) { url, error in
                if let error {
                    continuation.resume(throwing: error)
                    return
                }
                guard let url else {
                    continuation.resume(returning: nil)
                    return
                }
                // Copy before returning: the representation is deleted when
                // this closure ends.
                let destination = entry.appendingPathComponent(UUID().uuidString)
                do {
                    try FileManager.default.copyItem(at: url, to: destination)
                    continuation.resume(returning: destination)
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }
        guard let staged else { return nil }
        // The payload is named by uuid and its title travels in the manifest,
        // which is what keeps a display name from ever becoming a path. The
        // Rust reader refuses one that tries.
        let name = safeDisplayName(provider.suggestedName ?? "")
        let size = (try? staged.resourceValues(forKeys: [.fileSizeKey]))?.fileSize
        return ShareInbox.File(
            file: staged.lastPathComponent,
            name: name,
            mimeType: type.preferredMIMEType ?? "application/octet-stream",
            declaredSize: size.map(Int64.init))
    }

    private static func stageText(
        _ text: SharedText.TextFile, into entry: URL
    ) throws -> ShareInbox.File {
        let destination = entry.appendingPathComponent(UUID().uuidString)
        let bytes = Data(text.content.utf8)
        try bytes.write(to: destination, options: .atomic)
        return ShareInbox.File(
            file: destination.lastPathComponent,
            name: text.name,
            mimeType: text.mimeType,
            declaredSize: Int64(bytes.count))
    }

    // -------------------------------------------------------- the activation

    /// Bring Koru to the front, because activation is what triggers the drain.
    ///
    /// **By path, not by identifier.** This appex lives at
    /// `Koru.app/Contents/PlugIns/KoruShare.appex`, so walking three
    /// components up names *this build's* app exactly — where asking
    /// LaunchServices for `do.koru.desktop`, or opening a `koru://` link,
    /// would pick one of up to five installed channels and might pick another
    /// one. The URL is the fallback for the case where the walk is wrong (a
    /// development copy registered with `pluginkit` from a build directory),
    /// and it goes through `extensionContext` rather than `NSWorkspace`
    /// because that route is the one a sandboxed extension is allowed.
    ///
    /// iOS deliberately does *not* do this — a phone's share sheet returns you
    /// to what you were doing. On a Mac the app is a window that can come
    /// forward without taking the screen, and a share that appears to do
    /// nothing at all is the worse outcome.
    private static func activateKoru() {
        let appex = Bundle.main.bundleURL
        let app =
            appex
            .deletingLastPathComponent()  // PlugIns
            .deletingLastPathComponent()  // Contents
            .deletingLastPathComponent()  // Koru.app
        if app.pathExtension == "app", FileManager.default.fileExists(atPath: app.path) {
            NSWorkspace.shared.openApplication(at: app, configuration: .init())
            return
        }
        if let link = URL(string: "koru://") {
            NSExtensionContext().open(link)
        }
    }

    private func finish(cancelled: Bool) {
        if cancelled {
            extensionContext?.cancelRequest(
                withError: NSError(domain: "do.koru.desktop.share", code: 1))
        } else {
            extensionContext?.completeRequest(returningItems: nil)
        }
    }
}

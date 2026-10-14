// SPDX-License-Identifier: MPL-2.0

import UIKit
import UniformTypeIdentifiers

/// The share extension: pick a destination, stage into the inbox, commit,
/// done — ADR-137, with its recorded follow-up built.
///
/// This process never opens the Store (one SQLite root, one iroh endpoint, one
/// process — ADR-136). What it *can* do is read the destinations snapshot the
/// app exports into the App Group, so the sharer picks the workspace **here**,
/// the way Android's share activity does — and the app finishes the import
/// silently on its next activation. With no snapshot (a fresh install, an app
/// that never ran) it degrades to v1: stage without a destination, and the app
/// asks.
final class ShareViewController: UIViewController {
    private let spinner = UIActivityIndicatorView(style: .large)
    private let label = UILabel()
    private var table: UITableView?
    private var destinations: ShareDestinations?
    /// The table's sections: the recent destinations first, then workspaces
    /// grouped by profile in the snapshot's order. Profiles with no live
    /// workspace are not offered.
    private var sections: [ShareDestinationSection] = []

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .systemBackground

        destinations = ShareDestinations.read()
        sections = destinations.map {
            destinationSections(
                $0, recents: ShareRecentDestinations.read().map(\.workspaceId))
        } ?? []

        if sections.isEmpty {
            // v1: nothing to pick from — stage now, the app asks later.
            showProgress(NSLocalizedString("share_saving", comment: "staging shared content"))
            stage(into: nil, confirmation: nil)
        } else {
            showPicker()
        }
    }

    // ------------------------------------------------------------ the picker

    private func showPicker() {
        let bar = UINavigationBar()
        bar.translatesAutoresizingMaskIntoConstraints = false
        let item = UINavigationItem(
            title: NSLocalizedString("share_destination", comment: "picker title"))
        item.leftBarButtonItem = UIBarButtonItem(
            barButtonSystemItem: .cancel, target: self, action: #selector(cancelPressed))
        bar.items = [item]

        let table = UITableView(frame: .zero, style: .insetGrouped)
        table.translatesAutoresizingMaskIntoConstraints = false
        table.dataSource = self
        table.delegate = self
        self.table = table

        view.addSubview(bar)
        view.addSubview(table)
        NSLayoutConstraint.activate([
            bar.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor),
            bar.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            bar.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            table.topAnchor.constraint(equalTo: bar.bottomAnchor),
            table.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            table.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            table.bottomAnchor.constraint(equalTo: view.bottomAnchor),
        ])
    }

    @objc private func cancelPressed() {
        finish(cancelled: true)
    }

    // ----------------------------------------------------------- the staging

    private func showProgress(_ text: String) {
        table?.removeFromSuperview()
        label.text = text
        label.textAlignment = .center
        label.numberOfLines = 0
        label.font = .preferredFont(forTextStyle: .body)
        let stack = UIStackView(arrangedSubviews: [spinner, label])
        stack.axis = .vertical
        stack.spacing = 12
        stack.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            stack.centerYAnchor.constraint(equalTo: view.centerYAnchor),
            stack.leadingAnchor.constraint(
                greaterThanOrEqualTo: view.leadingAnchor, constant: 24),
        ])
        spinner.startAnimating()
    }

    /// Stage everything, commit with `workspaceId`, then — when a destination
    /// was chosen — show the Android snackbar's equivalent for a beat before
    /// dismissing: it is also where the one irreducible iOS difference is said
    /// out loud (the import completes when Koru next opens).
    private func stage(into workspaceId: String?, confirmation: String?) {
        let items = (extensionContext?.inputItems as? [NSExtensionItem]) ?? []
        let subject = items.first?.attributedTitle?.string
        guard let inbox = ShareInbox.shared(), !items.isEmpty else {
            finish(cancelled: true)
            return
        }
        Task {
            do {
                let entry = try inbox.begin()
                var files: [ShareInbox.File] = []
                for item in items {
                    let providers = item.attachments ?? []
                    // Providers on one item are representations of one thing —
                    // stage the best, not all of them, or a Safari URL lands
                    // twice. The choice is the pure `stagedProviderIndices`.
                    let chosen = stagedProviderIndices(
                        providers.map(\.registeredTypeIdentifiers))
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
                try inbox.commit(
                    entry, manifest: ShareInbox.Manifest(files: files, workspaceId: workspaceId))
                if let workspaceId {
                    ShareRecentDestinations.record(workspaceId: workspaceId)
                }
                if let confirmation {
                    spinner.stopAnimating()
                    label.text = confirmation
                    try? await Task.sleep(for: .milliseconds(900))
                }
                finish(cancelled: false)
            } catch {
                finish(cancelled: true)
            }
        }
    }

    /// One attachment into the entry directory. Files are copied under a
    /// UUID name (the provider's grant dies with this call, exactly like
    /// Android's URI grant); text follows the shared normalization — a single
    /// web URL becomes a `.url` file, matching `SharedText.kt`.
    private static func stage(
        _ provider: NSItemProvider, subject: String?, into entry: URL
    ) async throws -> ShareInbox.File? {
        // Order matters: a file that is also a URL must stage as the file.
        if provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier) {
            return try await stageFile(provider, into: entry)
        }
        if provider.hasItemConformingToTypeIdentifier(UTType.url.identifier),
            let url = try? await provider.loadItem(forTypeIdentifier: UTType.url.identifier)
                as? URL
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
            let text = try? await provider.loadItem(
                forTypeIdentifier: UTType.plainText.identifier) as? String
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

    private func finish(cancelled: Bool) {
        if cancelled {
            extensionContext?.cancelRequest(
                withError: NSError(domain: "do.koru.app.share", code: 1))
        } else {
            extensionContext?.completeRequest(returningItems: nil)
        }
    }
}

// ------------------------------------------------------------- picker table

extension ShareViewController: UITableViewDataSource, UITableViewDelegate {
    func numberOfSections(in tableView: UITableView) -> Int {
        sections.count
    }

    func tableView(_ tableView: UITableView, titleForHeaderInSection section: Int) -> String? {
        guard let profile = sections[section].profile else {
            return NSLocalizedString("share_recent", comment: "recent destinations section")
        }
        if let emoji = profile.leadingEmoji {
            return "\(emoji) \(profile.label)"
        }
        return profile.label
    }

    func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int {
        sections[section].workspaces.count
    }

    func tableView(
        _ tableView: UITableView, cellForRowAt indexPath: IndexPath
    ) -> UITableViewCell {
        let workspace = sections[indexPath.section].workspaces[indexPath.row]
        let cell = UITableViewCell(style: .default, reuseIdentifier: nil)
        var content = cell.defaultContentConfiguration()
        content.text =
            workspace.leadingEmoji.map { "\($0) \(workspace.label)" } ?? workspace.label
        cell.contentConfiguration = content
        return cell
    }

    func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        let workspace = sections[indexPath.section].workspaces[indexPath.row]
        showProgress(NSLocalizedString("share_saving", comment: "staging shared content"))
        let confirmation = String(
            format: NSLocalizedString(
                "share_saved_to", comment: "saved confirmation with workspace name"),
            workspace.label)
        stage(into: workspace.id, confirmation: confirmation)
    }
}

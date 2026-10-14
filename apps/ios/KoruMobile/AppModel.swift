// SPDX-License-Identifier: MPL-2.0

import Combine
import Foundation

@MainActor
final class AppModel: ObservableObject {
    @Published private(set) var profiles: [Profile] = []
    @Published private(set) var workspaces: [Workspace] = []
    @Published private(set) var documents: [DocumentSummary] = []
    @Published private(set) var snapshot: DocumentSnapshot?
    @Published private(set) var activeProfileID: ProfileID?
    @Published private(set) var activeWorkspaceID: WorkspaceID?
    @Published private(set) var activeDocumentID: DocumentID?
    @Published var failure: AppFailure?
    @Published var loading = false

    @Published private(set) var syncStatus = ContentSyncStatus()
    @Published private(set) var linkState: DeviceLinkState = .unavailable(reason: "no_device_link")
    @Published private(set) var linkedDevices: [LinkedDevice] = []
    @Published private(set) var thisDevice: LinkedDevice?
    /// The relay connection as the host reads it now — the status line under
    /// the Devices screen's relay toggle. `nil` until the host has answered.
    @Published private(set) var relayStatus: RelayStatus?
    /// The Documents screen's partition: `kind == "file"` rows are Files.
    @Published var documentProjection: DocumentProjection = .documents
    @Published var showArchivedWorkspaces = false
    @Published var showDeletedWorkspaces = false

    enum DocumentProjection: Hashable { case documents, files }

    let services: AppServices
    private let logger = AppLogger()
    private var profileTask: Task<Void, Never>?
    private var workspaceTask: Task<Void, Never>?
    private var documentTask: Task<Void, Never>?
    private var snapshotTask: Task<Void, Never>?
    private var eventTask: Task<Void, Never>?
    private var syncTask: Task<Void, Never>?
    private var linkTask: Task<Void, Never>?
    private var devicesTask: Task<Void, Never>?
    private var thisDeviceTask: Task<Void, Never>?
    private var relayStatusTask: Task<Void, Never>?
    private var session: (any DocumentSession)?

    init(services: AppServices) {
        self.services = services
        logger.event("app_start")
        observeProfiles()
        observeSync()
    }

    /// Workspaces as the list shows them: filters first, then the sort the
    /// Android screen settled on — live before archived, then most recently
    /// modified first.
    ///
    /// **The filter is a cascade, not two independent conditions, and deleted
    /// wins.** A
    /// workspace that is both archived and deleted is one the person deleted;
    /// *Show deleted* is the toggle that should reveal it, and *Show archived*
    /// alone should not. Anding the two conditions asked for both toggles at
    /// once and hid the row from either — and it disagreed with the `sorted`
    /// block three lines below, which has always tested `deleted` first and so
    /// already treated it as the dominant fact. Android's `workspaceVisible`
    /// is the same cascade; matching it is what makes the twin in
    /// `WorkspaceProjectionTests` able to pin the row at all.
    var visibleWorkspaces: [Workspace] {
        workspaces
            .filter { row in
                if row.deleted { return showDeletedWorkspaces }
                if row.archived { return showArchivedWorkspaces }
                return true
            }
            .sorted { a, b in
                if a.deleted != b.deleted { return !a.deleted }
                if a.archived != b.archived { return !a.archived }
                return a.modifiedAtMillis > b.modifiedAtMillis
            }
    }

    /// The active projection's rows. Files list; they do not open.
    var visibleDocuments: [DocumentSummary] {
        documents.filter { (documentProjection == .files) == ($0.kind == "file") }
    }

    func canOpen(_ document: DocumentSummary) -> Bool { document.kind != "file" }

    func selectProfile(_ id: ProfileID) {
        activeProfileID = id; activeWorkspaceID = nil; activeDocumentID = nil
        workspaceTask?.cancel()
        workspaceTask = Task { [weak self] in
            guard let self else { return }
            do { for try await rows in await services.workspaces.observeWorkspaces(profileID: id) { workspaces = rows } }
            catch is CancellationError {} catch { show(error) }
        }
        logger.event("navigate_workspaces", id: id.value)
    }

    func selectWorkspace(_ id: WorkspaceID) {
        activeWorkspaceID = id; activeDocumentID = nil
        documentTask?.cancel()
        documentTask = Task { [weak self] in
            guard let self else { return }
            do { for try await rows in await services.documents.observeDocuments(workspaceID: id) { documents = rows } }
            catch is CancellationError {} catch { show(error) }
        }
        logger.event("navigate_documents", id: id.value)
    }

    func openDocument(_ id: DocumentID) {
        activeDocumentID = id
        snapshotTask?.cancel(); eventTask?.cancel()
        Task { [weak self] in
            guard let self else { return }
            await session?.close()
            do {
                let opened = try await services.documents.openDocument(id: id)
                session = opened
                snapshotTask = Task { [weak self] in
                    do {
                        for try await value in opened.snapshots {
                            await MainActor.run {
                                self?.snapshot = value
                                self?.landIn(value.workspaceID)
                            }
                        }
                    } catch is CancellationError {
                    } catch { await MainActor.run { self?.show(error) } }
                }
                eventTask = Task { [weak self] in
                    do { for try await _ in opened.events {} }
                    catch is CancellationError {} catch { await MainActor.run { self?.show(error) } }
                }
                logger.event("document_open", id: id.value)
            } catch { show(error) }
        }
    }

    /// A followed link may land in another workspace — the catalog, not the
    /// list the person came from, says where a document lives. Keep the
    /// active workspace and profile truthful about what is on screen without
    /// restarting the observers; Back still returns along the pushed path.
    private func landIn(_ workspaceID: WorkspaceID) {
        guard activeWorkspaceID != workspaceID else { return }
        activeWorkspaceID = workspaceID
        Task { [weak self] in
            guard let self,
                let workspace = try? await services.workspaces.workspace(id: workspaceID)
            else { return }
            await MainActor.run {
                if self.activeProfileID != workspace.profileID {
                    self.activeProfileID = workspace.profileID
                }
            }
        }
    }

    func createProfile(name: String, locale: String) async -> ProfileID? {
        await perform { try await services.profiles.createProfile(displayName: name, locale: locale).id }
    }
    func createWorkspace(title: String) async -> WorkspaceID? {
        guard let activeProfileID else { return nil }
        return await perform { try await services.workspaces.createWorkspace(profileID: activeProfileID, title: title).id }
    }
    func createDocument(title: String) async -> DocumentID? {
        guard let activeWorkspaceID else { return nil }
        return await perform { try await services.documents.createDocument(workspaceID: activeWorkspaceID, title: title).id }
    }

    func execute(_ command: DocumentCommand) {
        guard let session else { return }
        Task { do { try await session.execute(command) } catch { await MainActor.run { self.show(error) } } }
    }

    func addBlock(_ content: BlockContent, parentID: BlockID? = nil) {
        execute(.createBlock(Block(id: BlockID(), parentID: parentID, content: content), siblingIndex: .max))
    }

    func addImage() {
        Task {
            do {
                let bytes = Data(base64Encoded: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=")!
                let blob = try await services.blobs.put(bytes, mimeType: "image/png", width: 1, height: 1)
                await MainActor.run { addBlock(.image(blob)) }
            } catch { await MainActor.run { show(error) } }
        }
    }

    func addReference(to documentID: DocumentID, blockID: BlockID, atUTF8: Int) {
        let object = InlineObject(id: InlineObjectID(), anchorUTF8: atUTF8, role: "document", reference: .document(documentID), label: documents.first { $0.id == documentID }?.title)
        execute(.insertInlineObject(blockID, object))
    }

    // Workspace lifecycle — the Android screen's long-press actions.
    func renameProfile(_ id: ProfileID, displayName: String) async {
        _ = await perform { try await services.profiles.rename(id: id, displayName: displayName) }
    }
    func renameWorkspace(_ id: WorkspaceID, title: String) async {
        _ = await perform { try await services.workspaces.rename(id: id, title: title) }
    }
    func setWorkspaceArchived(_ id: WorkspaceID, archived: Bool) async {
        _ = await perform { try await services.workspaces.setArchived(id: id, archived: archived) }
    }
    func deleteWorkspace(_ id: WorkspaceID) async {
        _ = await perform { try await services.workspaces.delete(id: id) }
    }
    func restoreWorkspace(_ id: WorkspaceID) async {
        _ = await perform { try await services.workspaces.restore(id: id) }
    }
    func renameDocument(_ id: DocumentID, title: String) async {
        _ = await perform { try await services.documents.rename(id: id, title: title) }
    }
    func trashDocument(_ id: DocumentID) async {
        _ = await perform { try await services.documents.moveToTrash(id: id) }
    }

    /// Open the file's note — its existing one, or *Add note*'s ensured one.
    /// A note opens like any other document; `open` pushes its route.
    func openNote(for file: DocumentSummary, open: (DocumentID) -> Void) async {
        guard let files = services.files else { return }
        if let existing = file.noteId {
            openDocument(existing)
            open(existing)
            return
        }
        if let note = await perform({ try await files.ensureNote(file: file.id) }) {
            openDocument(note)
            open(note)
        }
    }

    // Shared content --------------------------------------------------------

    /// Committed inbox entries awaiting a destination — the app half of
    /// ADR-137. The extension staged them; draining happens on activation.
    @Published private(set) var pendingShares: [ShareInbox.Entry] = []

    func drainShareInbox() {
        guard services.files != nil, let inbox = ShareInbox.shared() else { return }
        let entries = inbox.drain()
        // An entry whose destination was already chosen in the extension's
        // picker imports silently — that is the whole point of the picker.
        // The sheet stays for the rest: pre-picker entries, the no-snapshot
        // fallback, and a chosen workspace that no longer exists (the
        // snapshot is hints; this validation is the truth).
        var toAsk: [ShareInbox.Entry] = []
        for entry in entries {
            if let chosen = entry.manifest.workspaceId {
                Task { await importChosen(entry, into: WorkspaceID(chosen)) }
            } else {
                toAsk.append(entry)
            }
        }
        pendingShares = toAsk
    }

    /// Import an entry whose destination the sharer already picked. The
    /// workspace is validated through the live inventory, with a short grace
    /// because this races the very first inventory poll at cold start; only
    /// when it stays unknown does the entry fall back to the ask-sheet.
    private func importChosen(_ entry: ShareInbox.Entry, into workspace: WorkspaceID) async {
        for _ in 0..<6 {
            if (try? await services.workspaces.workspace(id: workspace)) != nil {
                await importShare(entry, into: workspace)
                return
            }
            try? await Task.sleep(for: .milliseconds(500))
        }
        if !pendingShares.contains(where: { $0.id == entry.id }) {
            pendingShares.append(entry)
        }
    }

    /// Import one entry into the chosen workspace through the ordinary
    /// `WorkspaceFileService.save` — the same all-or-nothing path a live share
    /// takes on Android. The inbox entry is removed only on success, so a
    /// failed import is offered again next activation. `names` and `notes`
    /// carry the sheet's per-file edits — names through the shared
    /// `safeDisplayName` rules, a blank note meaning no note; `nil` (the
    /// extension-picked path) keeps the staged names and notes.
    func importShare(_ entry: ShareInbox.Entry, into workspace: WorkspaceID, names: [String]? = nil, notes: [String]? = nil) async {
        guard let files = services.files else { return }
        let share = IncomingShare(
            files: entry.manifest.files.enumerated().map { index, file in
                let url = entry.directory.appendingPathComponent(file.file)
                let edited = names.flatMap { index < $0.count ? $0[index] : nil }
                let editedNote = notes.flatMap { index < $0.count ? $0[index] : nil }
                let note = (editedNote ?? file.note)?
                    .trimmingCharacters(in: .whitespacesAndNewlines)
                return IncomingSharedFile(
                    name: edited.map(safeDisplayName) ?? file.name,
                    mimeType: file.mimeType,
                    declaredSize: file.declaredSize,
                    note: note?.isEmpty == false ? note : nil,
                    open: {
                        guard let stream = InputStream(url: url) else {
                            throw AppFailure(code: .unavailable, detail: "shared content could not be read")
                        }
                        return stream
                    })
            })
        let saved = await perform { try await files.save(workspaceID: workspace, share: share) }
        if saved != nil {
            ShareInbox.shared()?.remove(entry)
            pendingShares.removeAll { $0.id == entry.id }
            // Lead the next share with this workspace — where someone just
            // saved is most likely where they save next.
            ShareRecentDestinations.record(workspaceId: workspace.value)
            logger.event("shared_content_saved", id: workspace.value)
        }
    }

    /// The sheet's "Recent" rows: the recorded destinations that still exist,
    /// resolved against the live inventory (the persisted ids are hints; this
    /// is the truth), most recent first. A workspace that is gone, archived,
    /// or deleted is dropped.
    func recentShareDestinations() async -> [RecentShareDestination] {
        var rows: [RecentShareDestination] = []
        for entry in ShareRecentDestinations.read() {
            guard let workspace = try? await services.workspaces.workspace(
                id: WorkspaceID(entry.workspaceId)),
                !workspace.archived, !workspace.deleted
            else { continue }
            let profile = profiles.first { $0.id == workspace.profileID }
            rows.append(RecentShareDestination(workspace: workspace, profile: profile))
        }
        return rows
    }

    func discardShare(_ entry: ShareInbox.Entry) {
        ShareInbox.shared()?.remove(entry)
        pendingShares.removeAll { $0.id == entry.id }
    }

    /// Postpone: drop the offer for this session; the inbox keeps the entries
    /// and the next activation offers them again.
    func postponeShares() {
        pendingShares = []
    }

    // Device linking ------------------------------------------------------

    /// The Universal Link `https://koru.do/link-device`, the one deep link
    /// mobile has (`koru://` is desktop-scope, ADR-132). The whole URL is the
    /// link code, exactly as Android's `acceptLinkIntent` passes it — the
    /// native host parses it.
    nonisolated static func isLinkURL(_ url: URL) -> Bool {
        url.scheme == "https" && url.host == "koru.do" && url.path == "/link-device"
    }

    func acceptLink(_ url: URL) {
        guard Self.isLinkURL(url) else { return }
        let code = url.absoluteString
        Task { await link(code: code) }
    }

    func link(code: String) async {
        _ = await perform { try await services.deviceLink?.link(code: code) }
    }
    func startPairing() async -> PairingOffer? {
        await perform { try await services.deviceLink?.startPairing() }.flatMap { $0 }
    }
    func cancelPairing() async {
        await services.deviceLink?.cancelPairing()
    }
    func unlink(deviceID: String) async {
        _ = await perform { try await services.deviceLink?.unlink(deviceID: deviceID) }
    }
    func renameThisDevice(_ name: String) async {
        _ = await perform { try await services.deviceLink?.rename(name: name) }
        thisDevice = await services.deviceLink?.thisDevice
    }

    /// Apply the relay toggle to the live host (decision №6: no restart).
    /// The toggle's `@AppStorage` binding already persisted the value in
    /// UserDefaults, which owns it; this only makes the running host match.
    func setRelayEnabled(_ enabled: Bool) async {
        _ = await perform { try await services.deviceLink?.setRelay(enabled: enabled) }
    }

    /// Fetched when a device is opened, never with the periodic status — the
    /// same on-demand shape as Android, so routine refreshes stay small.
    func deviceHistory(deviceID: String, before: Int64?) async throws -> DeviceHistoryPage {
        guard let link = services.deviceLink else { throw AppFailure(code: .unavailable, detail: "no_device_link") }
        return try await link.history(deviceID: deviceID, before: before)
    }

    func resetDeviceIdentity() async {
        _ = await perform { try await services.deviceLink?.resetIdentity() }
        if let link = services.deviceLink {
            linkState = await link.state
            thisDevice = await link.thisDevice
            linkedDevices = await link.devices
        }
    }

    private func observeProfiles() {
        profileTask = Task { [weak self] in
            guard let self else { return }
            do { for try await rows in await services.profiles.observeProfiles() { profiles = rows } }
            catch is CancellationError {} catch { show(error) }
        }
    }

    private func observeSync() {
        syncTask = Task { [weak self] in
            guard let self else { return }
            for await value in services.syncStatus() { syncStatus = value }
        }
        guard let deviceLink = services.deviceLink else { return }
        linkTask = Task { [weak self] in
            guard let self else { return }
            for await value in await deviceLink.observeState() { linkState = value }
        }
        devicesTask = Task { [weak self] in
            guard let self else { return }
            for await value in await deviceLink.observeDevices() { linkedDevices = value }
        }
        thisDeviceTask = Task { [weak self] in
            guard let self, let native = deviceLink as? KoruDeviceLinkService else { return }
            for await value in await native.observeThisDevice() { thisDevice = value }
        }
        relayStatusTask = Task { [weak self] in
            guard let self, let native = deviceLink as? KoruDeviceLinkService else { return }
            for await value in await native.observeRelayStatus() { relayStatus = value }
        }
    }

    private func perform<T: Sendable>(_ operation: () async throws -> T) async -> T? {
        loading = true; defer { loading = false }
        do { return try await operation() } catch { show(error); return nil }
    }

    private func show(_ error: Error) { failure = error as? AppFailure ?? AppFailure(code: .unknown, detail: String(describing: error)) }
}

/// A recorded share destination resolved against the live inventory — the
/// workspace the sheet offers one tap into, and its profile when the
/// inventory knows it (the row's subtitle).
struct RecentShareDestination: Equatable, Identifiable {
    let workspace: Workspace
    let profile: Profile?
    var id: String { workspace.id.value }
}

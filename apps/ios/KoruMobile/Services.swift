// SPDX-License-Identifier: MPL-2.0

import Foundation

// The service seam, mirrored member-for-member from Android's `Services.kt` so
// the two apps replace the same adapter. Mock* stays the test/preview
// implementation; KoruStore* is the embedded Store behind `KoruNative`.

enum DeviceLinkState: Equatable, Sendable {
    case unlinked
    case linking(peerName: String? = nil)
    case linked(peerName: String?)
    case failed(code: String)
    /// The seeds in the Keychain no longer produce the keys `device.json`
    /// names — an app deleted and reinstalled (iOS drops the Keychain item
    /// with it), or data restored onto another phone. Content is intact and
    /// the native library is fine; only [`DeviceLinkService.resetIdentity`]
    /// leads out of it.
    case identityMismatch
    case unavailable(reason: String)
}

struct PairingOffer: Equatable, Sendable {
    let payload: String
    let expiresInSeconds: Int
}

/// The four readings of the relay connection. `off` is this device's own
/// switch; `disconnected` covers both a relay that died after connecting
/// and one never reached, which is why the UI words it "not connected"
/// where that is the honest common form.
enum RelayState: Equatable, Sendable {
    case off, connecting, connected, disconnected
}

/// One relay reading: the state, and the relay's host — empty for `.off`
/// (the switch beside it already says which switch) and when no relay has
/// been named yet. The mobile mirror of the desktop tray's status row,
/// carried on the same devices query the screen already polls.
struct RelayStatus: Equatable, Sendable {
    let state: RelayState
    let host: String
}

struct LinkedDevice: Equatable, Hashable, Sendable, Identifiable {
    let deviceID: String
    let displayName: String
    let linkedAtMillis: Int64
    let connected: Bool
    let lastSeenMillis: Int64?
    /// The directly paired device this one arrived through — a mesh peer, not
    /// a local pairing. `nil` for a directly linked device.
    var introducedBy: String? = nil
    /// `version · channel · branch · OS`, as the device's latest self-signed
    /// build record puts it. `nil` for a device that has never said.
    var buildLabel: String? = nil
    /// Semver drift from this device — "same", "patch", "minor", "major",
    /// "incompatible" (different peer protocol numbers: the two do not sync)
    /// or "unknown". `nil` when either side has not said. Decided by the
    /// native host, which holds both descriptions; the view only colors it.
    var buildDrift: String? = nil
    /// "warning" (a minor apart) or "danger" (a major apart, or incompatible).
    var buildSeverity: String? = nil
    /// Only when incompatible: "peer" or "this_device" — who has to update.
    var updateNeededOn: String? = nil
    /// "direct" or "relay" while connected and the path is known.
    var connection: String? = nil
    /// When this device last changed something here — the moment the device
    /// list shows instead of an indefinite "syncing". `nil` until it has.
    var lastSyncMillis: Int64? = nil
    var id: String { deviceID }
}

/// One thing a linked device changed here: a catalog, profile, workspace,
/// document or blob event, metadata only — never the content. Seven days are
/// kept, newest first, fifty to a page.
struct DeviceStateEvent: Equatable, Hashable, Sendable, Identifiable {
    var eventId: Int64?
    var atMillis: Int64
    var kind: String
    var profileId: String?
    var workspaceId: String?
    var documentId: String?
    var documentName: String?
    var blobHash: String?
    var detail: String?
    var items: Int?
    /// Durable events carry an id; a live fallback row (history could not be
    /// written) does not, and is identified by its moment instead.
    var id: String { eventId.map { "e\($0)" } ?? "t\(atMillis)-\(kind)" }
}

struct DeviceHistoryPage: Equatable, Sendable {
    var deviceId: String
    var events: [DeviceStateEvent]
    /// Pass back as `before` for the next older page; `nil` at the end.
    var nextBefore: Int64?
}

/// What this app tells linked devices it runs. Keys match
/// `koru_device::BuildPayload`; the version is the bundle's — what the App
/// Store or TestFlight says — because the native library has no build info of
/// its own. The branch and commit come from `KoruGitBranch` / `KoruGitCommit`
/// in Info.plist, which `koru.sh` stamps from `git rev-parse`; a build made
/// without them advertises neither, like a released desktop build.
struct BuildPayload: Codable, Equatable, Sendable {
    var appVersion: String
    var channel: String
    var branch: String?
    var commit: String?
    var os: String?
    var osVersion: String?

    enum CodingKeys: String, CodingKey {
        case appVersion = "app_version", channel, branch, commit, os
        case osVersion = "os_version"
    }

    /// The row label the host would render: what the log line says.
    var label: String {
        [appVersion, channel, branch, os].compactMap { $0 }.joined(separator: " · ")
    }

    /// The stamp the profiles screen draws, quietly, so "which build is on
    /// this phone?" never needs Developer → Diagnostics: `v0.1.0 #7 - branch`.
    /// Android's `KoruApp.kt` writes the same shape from `VERSION_NAME` and
    /// `VERSION_CODE`; here the two numbers are the bundle's, which is what
    /// TestFlight and the App Store show. **The branch is what tells two
    /// installs of one version apart**, and it is absent only when git was
    /// unavailable at build time — then the label just ends, rather than
    /// trailing a separator with nothing after it.
    static func stampLabel(
        info: [String: Any]? = Bundle.main.infoDictionary,
        gitStamp: [String: String]? = bundledGitStamp()
    ) -> String {
        let version = info?["CFBundleShortVersionString"] as? String ?? "0.0.0"
        let build = info?["CFBundleVersion"] as? String ?? "0"
        let branch = gitStamp?["branch"].flatMap { $0.isEmpty ? nil : " - \($0)" } ?? ""
        return "v\(version) #\(build)\(branch)"
    }

    /// What `koru.sh` wrote beside the sources; `nil` when nothing did.
    static func bundledGitStamp(in bundle: Bundle = .main) -> [String: String]? {
        guard let url = bundle.url(forResource: "GitStamp", withExtension: "json"),
            let data = try? Data(contentsOf: url)
        else { return nil }
        return try? JSONDecoder().decode([String: String].self, from: data)
    }

    static func current(
        info: [String: Any]? = Bundle.main.infoDictionary,
        gitStamp: [String: String]? = bundledGitStamp()
    ) -> BuildPayload {
        let version = info?["CFBundleShortVersionString"] as? String ?? "0.0.0"
        let build = info?["CFBundleVersion"] as? String ?? "0"
        let nonBlank = { (key: String) -> String? in
            gitStamp?[key].flatMap { $0.isEmpty ? nil : $0 }
        }
        #if DEBUG
        let channel = "debug"
        #else
        let channel = "release"
        #endif
        let system = ProcessInfo.processInfo.operatingSystemVersion
        let release = "\(system.majorVersion).\(system.minorVersion)" + (system.patchVersion == 0 ? "" : ".\(system.patchVersion)")
        return BuildPayload(
            appVersion: "\(version) (\(build))",
            channel: channel,
            branch: nonBlank("branch"),
            commit: nonBlank("commit"),
            os: "iOS \(release)",
            osVersion: "\(release) (\(ProcessInfo.processInfo.operatingSystemVersionString))")
    }
}

protocol DeviceLinkService: Sendable {
    /// Current value first, then every change — the AsyncStream shape of
    /// Kotlin's StateFlow.
    func observeState() async -> AsyncStream<DeviceLinkState>
    func observeDevices() async -> AsyncStream<[LinkedDevice]>
    var state: DeviceLinkState { get async }
    var devices: [LinkedDevice] { get async }
    /// This device as the roster shows it — current (possibly gossiped) name.
    var thisDevice: LinkedDevice? { get async }
    func link(code: String) async throws
    func startPairing() async throws -> PairingOffer
    func cancelPairing() async
    /// Rename this device; the new name gossips to every linked device.
    func rename(name: String) async throws
    func unlink(deviceID: String) async throws
    /// Start over as a new device after `.identityMismatch`, keeping every
    /// workspace. The old identity is set aside on disk, never deleted; it
    /// stays listed offline on the other devices until unlinked there.
    func resetIdentity() async throws
    /// Apply this device's relay on/off value to the live host — no app
    /// restart (iroh-relay roadmap, decision №6). Off stops relayed
    /// connections only: same-network and direct sync continue. The
    /// persisted value is UserDefaults' (`KoruNative.relayEnabledKey`),
    /// written by the toggle; this call only makes the running host match.
    func setRelay(enabled: Bool) async throws
    /// One page of what `deviceID` changed here, newest first; `before` is the
    /// previous page's `nextBefore`, `nil` for the newest page.
    func history(deviceID: String, before: Int64?) async throws -> DeviceHistoryPage
}

struct ContentSyncStatus: Equatable, Sendable {
    var syncing = false
    var profileCount = 0
    var workspaceCount = 0
    var documentCount = 0
}

protocol ProfileService: Sendable {
    func observeProfiles() async -> AsyncThrowingStream<[Profile], Error>
    func profile(id: ProfileID) async throws -> Profile
    func createProfile(displayName: String, locale: String) async throws -> Profile
    func rename(id: ProfileID, displayName: String) async throws -> Profile
}

protocol WorkspaceService: Sendable {
    func observeWorkspaces(profileID: ProfileID) async -> AsyncThrowingStream<[Workspace], Error>
    func workspace(id: WorkspaceID) async throws -> Workspace
    func createWorkspace(profileID: ProfileID, title: String) async throws -> Workspace
    func rename(id: WorkspaceID, title: String) async throws -> Workspace
    func setArchived(id: WorkspaceID, archived: Bool) async throws -> Workspace
    func delete(id: WorkspaceID) async throws -> Workspace
    func restore(id: WorkspaceID) async throws -> Workspace
}

protocol DocumentService: Sendable {
    func observeDocuments(workspaceID: WorkspaceID) async -> AsyncThrowingStream<[DocumentSummary], Error>
    func document(id: DocumentID) async throws -> DocumentSummary
    func createDocument(workspaceID: WorkspaceID, title: String) async throws -> DocumentSummary
    func rename(id: DocumentID, title: String) async throws -> DocumentSummary
    func moveToTrash(id: DocumentID) async throws
    func openDocument(id: DocumentID) async throws -> any DocumentSession
}

/// The six-state hydration story a remote image goes through — ADR-128:
/// a placeholder, never an error, and a manual Retry rather than a timer.
enum BlobLoadState: Equatable, Sendable {
    case checkingLocal
    case noLinkedDevices
    case waitingForPeer(deviceName: String?)
    case fetching(receivedBytes: Int64, totalBytes: Int64?)
    case available(Data)
    case failed(code: String, retryable: Bool)
}

protocol BlobService: Sendable {
    func observeAvailability(id: BlobID) async -> AsyncThrowingStream<Bool, Error>
    func observeLoad(_ reference: BlobReference) -> AsyncStream<BlobLoadState>
    func put(_ bytes: Data, mimeType: String, width: UInt32?, height: UInt32?) async throws -> BlobReference
    func load(_ reference: BlobReference) async throws -> Data
}

extension BlobService {
    /// The plain fallback Kotlin's interface default provides: local check,
    /// then one load attempt. `KoruStoreBlobService` overrides with the peer-
    /// aware states.
    func observeLoad(_ reference: BlobReference) -> AsyncStream<BlobLoadState> {
        AsyncStream { continuation in
            let task = Task {
                continuation.yield(.checkingLocal)
                do {
                    continuation.yield(.available(try await load(reference)))
                } catch {
                    let code = (error as? AppFailure)?.detail ?? "blob_load_failed"
                    continuation.yield(.failed(code: code, retryable: true))
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }
}

struct IncomingSharedFile: Sendable {
    let name: String
    let mimeType: String
    let declaredSize: Int64?
    /// The share screen's optional note, saved as the file's one note.
    var note: String? = nil
    /// Opens the shared payload for staging; the path, not the bytes, will
    /// cross the native boundary.
    let open: @Sendable () throws -> InputStream
}

struct IncomingShare: Sendable {
    let files: [IncomingSharedFile]
}

/// Ingest for externally shared content.
///
/// A saved item becomes an ordinary `kind == "file"` document in the Store, so
/// it lists in the Documents screen's Files tab and replicates like anything
/// else. There is no second projection to observe.
protocol WorkspaceFileService: Sendable {
    func save(workspaceID: WorkspaceID, share: IncomingShare) async throws -> [DocumentSummary]
    /// The file's one note, created (empty, seeded) on first ask — *Add note*.
    func ensureNote(file: DocumentID) async throws -> DocumentID
}

protocol DocumentSession: Sendable {
    var documentID: DocumentID { get }
    var snapshots: AsyncThrowingStream<DocumentSnapshot, Error> { get }
    var events: AsyncThrowingStream<DocumentEvent, Error> { get }
    func execute(_ command: DocumentCommand, operationID: UUID) async throws
    func close() async
}

extension DocumentSession {
    func execute(_ command: DocumentCommand) async throws { try await execute(command, operationID: UUID()) }
}

struct AppServices: Sendable {
    let profiles: any ProfileService
    let workspaces: any WorkspaceService
    let documents: any DocumentService
    let blobs: any BlobService
    let deviceLink: (any DeviceLinkService)?
    let files: (any WorkspaceFileService)?
    let syncStatus: @Sendable () -> AsyncStream<ContentSyncStatus>
    let implementation: String

    init(
        profiles: any ProfileService,
        workspaces: any WorkspaceService,
        documents: any DocumentService,
        blobs: any BlobService,
        deviceLink: (any DeviceLinkService)? = nil,
        files: (any WorkspaceFileService)? = nil,
        syncStatus: @escaping @Sendable () -> AsyncStream<ContentSyncStatus> = {
            AsyncStream { $0.yield(ContentSyncStatus()); $0.finish() }
        },
        implementation: String
    ) {
        self.profiles = profiles
        self.workspaces = workspaces
        self.documents = documents
        self.blobs = blobs
        self.deviceLink = deviceLink
        self.files = files
        self.syncStatus = syncStatus
        self.implementation = implementation
    }
}

// SPDX-License-Identifier: MPL-2.0

import Foundation

// The embedded, replicated Store behind `KoruNative` — the iOS mirror of
// Android's `NativeDeviceLinkService.kt` + `NativeContentServices.kt`, decoded
// from the same JSON `koru-mobile-host` produces (its golden tests are the
// authority on every shape read here). The polls match Kotlin's: 1 s for
// status/devices, 500 ms for the inventory and an open document.

// ------------------------------------------------------------ wire models

private struct NativeLinkResult: Decodable {
    var state: String?
    var peerName: String?
    var errorCode: String?

    var asState: DeviceLinkState {
        switch state ?? "failed" {
        case "unlinked": .unlinked
        case "linking": .linking(peerName: peerName)
        case "linked": .linked(peerName: peerName)
        case "failed": .failed(code: errorCode ?? "pairing_failed")
        case _ where errorCode == "device_identity_mismatch": .identityMismatch
        default: .unavailable(reason: errorCode ?? "native_status")
        }
    }
}

/// JSON projection produced by `koru-mobile-host`. Internal so the boundary's
/// field-completeness tests can decode the exact shape the app consumes.
struct NativeInventory: Decodable, Equatable {
    struct Profile: Decodable, Equatable {
        var id: String
        var name: String
        var locale: String
        var icon: String?
        var leadingEmoji: String?
        var displayName: String?
        var color: String?
    }
    struct Workspace: Decodable, Equatable {
        var id: String
        var profileId: String
        var name: String
        var icon: String?
        var color: String?
        var leadingEmoji: String?
        var displayName: String?
        var archived: Bool?
        var deletedAtMillis: Int64?
        var modifiedAtMillis: Int64?
    }
    struct Document: Decodable, Equatable {
        var id: String
        var workspaceId: String
        var title: String
        var displayTitle: String?
        var leadingEmoji: String?
        var kind: String?
        var mime: String?
    }
    var profiles: [Profile] = []
    var workspaces: [Workspace] = []
    var documents: [Document] = []
    // Health counters, logged so a missing document is distinguishable from
    // an oscillating one — the same reason Android carries them.
    var deletedWorkspaces: Int?
    var trashedDocuments: Int?
    var failedWorkspaces: Int?
    var errorCode: String?
}

private struct NativeDeviceSnapshot: Decodable {
    struct Device: Decodable, Equatable {
        var deviceId: String
        var displayName: String
        var linkedAtMillis: Int64
        var connected: Bool
        var lastSeenMillis: Int64?
        var introducedBy: String?
        var buildLabel: String?
        var buildDrift: String?
        var buildSeverity: String?
        var updateNeededOn: String?
        var connection: String?
        var lastSyncMillis: Int64?
    }
    struct SyncEvent: Decodable, Equatable {
        var sequence: Int64
        var atMillis: Int64
        var direction: String
        var peer: String?
        var stage: String
        var workspaceId: String?
        var items: Int?
        var errorCode: String?
    }
    var devices: [Device] = []
    var thisDevice: Device?
    var syncEvent: SyncEvent?
    var errorCode: String?
}

struct NativeDeviceHistory: Decodable {
    struct Event: Decodable {
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
    }
    var deviceId: String = ""
    var events: [Event] = []
    var nextBefore: Int64?
    var errorCode: String?

    var page: DeviceHistoryPage {
        DeviceHistoryPage(
            deviceId: deviceId,
            events: events.map {
                DeviceStateEvent(
                    eventId: $0.eventId, atMillis: $0.atMillis, kind: $0.kind,
                    profileId: $0.profileId, workspaceId: $0.workspaceId,
                    documentId: $0.documentId, documentName: $0.documentName,
                    blobHash: $0.blobHash, detail: $0.detail, items: $0.items)
            },
            nextBefore: nextBefore)
    }
}

private struct NativePairingOffer: Decodable {
    var state: String?
    var qrPayload: String?
    var expiresInSeconds: Int?
    var errorCode: String?
}

private struct NativeDocumentEnvelope: Decodable {
    struct Document: Decodable {
        var id: String
        var workspaceId: String
        var title: String
        var revision: UInt64
        var blocks: [Block]
    }
    struct Block: Decodable {
        var id: String
        var parentId: String?
        var kind: String
        var properties: Properties
        var richText: Rich?
        var blob: Blob?
        var reference: Reference?
    }
    struct Properties: Decodable {
        var name: String?
        var icon: String?
        var color: String?
        var done: Bool?
        var alignment: String?
        var headingLevel: UInt32?
        var listStyle: String?
        var imageWidth: UInt32?
        var imageHeight: UInt32?
        var language: String?
    }
    struct Rich: Decodable {
        var text: String
        var marks: [Mark] = []
        var inlineObjects: [Inline] = []
    }
    struct Mark: Decodable {
        var kind: String
        var startUtf8: Int
        var endUtf8: Int
        var value: String?
    }
    struct Inline: Decodable {
        var id: String
        var anchorUtf8: Int
        var role: String
        var reference: Reference
        var label: String?
    }
    struct Blob: Decodable {
        var id: String
        var mimeType: String
        var size: UInt64
        var width: UInt32?
        var height: UInt32?
    }
    struct Reference: Decodable {
        var kind: String
        var documentId: String?
        var blockId: String?
        var url: String?
        var value: String?
    }
    var document: Document?
    var errorCode: String?
}

private let wireDecoder: JSONDecoder = {
    let decoder = JSONDecoder()
    decoder.keyDecodingStrategy = .convertFromSnakeCase
    return decoder
}()

private func decode<T: Decodable>(_ type: T.Type, from json: String) throws -> T {
    try wireDecoder.decode(type, from: Data(json.utf8))
}

private func failure(_ code: String) -> AppFailure {
    AppFailure(code: .unavailable, detail: code)
}

// ---------------------------------------------------------------- broadcast

/// A current value plus everyone watching it — the AsyncStream shape of
/// Kotlin's StateFlow, small enough to read in one screen.
actor Broadcast<Value: Sendable> {
    private var value: Value
    private var watchers: [UUID: AsyncStream<Value>.Continuation] = [:]

    init(_ initial: Value) { value = initial }

    var current: Value { value }

    func set(_ next: Value) {
        value = next
        for continuation in watchers.values { continuation.yield(next) }
    }

    func stream() -> AsyncStream<Value> {
        AsyncStream { continuation in
            let token = UUID()
            watchers[token] = continuation
            continuation.yield(value)
            continuation.onTermination = { [weak self] _ in
                Task { await self?.forget(token) }
            }
        }
    }

    private func forget(_ token: UUID) { watchers[token] = nil }
}

// ---------------------------------------------------------------- the core

/// Root, secrets, and every `KoruNative` call in one place, serialized: the
/// host walks SQLite, so calls stay off the main actor by construction.
actor KoruStoreCore {
    let root: String
    let secrets: Data
    private let logger = AppLogger()

    init(root: String, secrets: Data) {
        self.root = root
        self.secrets = secrets
    }

    func status() -> DeviceLinkState {
        guard let parsed = try? KoruNative.status(root: root, secrets: secrets) else {
            return .unavailable(reason: "native_status")
        }
        return linkState(parsed.state, parsed.hasPeerName ? parsed.peerName : nil,
                         parsed.hasErrorCode ? parsed.errorCode : nil)
    }

    func link(code: String) throws -> DeviceLinkState {
        let parsed = try KoruNative.link(root: root, secrets: secrets, code: code)
        logger.event("device_link", id: parsed.state)
        let error = parsed.hasErrorCode ? parsed.errorCode : nil
        if parsed.state == "failed" { throw failure(error ?? "pairing_failed") }
        return linkState(parsed.state, parsed.hasPeerName ? parsed.peerName : nil, error)
    }

    func startPairing() throws -> PairingOffer {
        let parsed = try KoruNative.startPairing(root: root, secrets: secrets)
        if parsed.hasErrorCode { throw failure(parsed.errorCode) }
        guard parsed.hasQrPayload, !parsed.qrPayload.isEmpty else { throw failure("native_pairing_missing_qr_payload") }
        guard parsed.expiresInSeconds > 0 else { throw failure("native_pairing_invalid_expiry") }
        logger.event("pairing_offer_created")
        return PairingOffer(payload: parsed.qrPayload, expiresInSeconds: Int(parsed.expiresInSeconds))
    }

    func cancelPairing() {
        _ = try? KoruNative.cancelPairing(root: root, secrets: secrets)
    }

    func unlink(deviceID: String) throws {
        let value = try KoruNative.unlink(root: root, secrets: secrets, deviceID: deviceID)
        try mutation(value)
        logger.event("device_unlinked", id: deviceID)
    }

    func deviceSnapshot() throws -> ([LinkedDevice], LinkedDevice?, NativeSyncEventSummary?, RelayStatus?) {
        let parsed = try KoruNative.devices(root: root, secrets: secrets)
        if parsed.hasErrorCode { throw failure(parsed.errorCode) }
        let linked = { (wire: KORAPILinkedDevice) in
            LinkedDevice(
                deviceID: wire.deviceID, displayName: wire.displayName,
                linkedAtMillis: wire.linkedAtMillis, connected: wire.connected,
                lastSeenMillis: wire.hasLastSeenMillis ? wire.lastSeenMillis : nil,
                introducedBy: wire.hasIntroducedBy ? wire.introducedBy : nil,
                buildLabel: wire.hasBuildLabel ? wire.buildLabel : nil,
                buildDrift: wire.hasBuildDrift ? wire.buildDrift : nil,
                buildSeverity: wire.hasBuildSeverity ? wire.buildSeverity : nil,
                updateNeededOn: wire.hasUpdateNeededOn ? wire.updateNeededOn : nil,
                connection: wire.hasConnection ? wire.connection : nil,
                lastSyncMillis: wire.hasLastSyncMillis ? wire.lastSyncMillis : nil)
        }
        let event = parsed.hasSyncEvent
            ? NativeSyncEventSummary(sequence: Int64(parsed.syncEvent.sequence), stage: parsed.syncEvent.stage, peer: parsed.syncEvent.peer)
            : nil
        // `unspecified` (and any value this build does not know) is
        // "nothing to claim" — an old host simply shows no status line.
        var relay: RelayStatus?
        if parsed.hasRelay {
            let state: RelayState? = switch parsed.relay.state {
            case .off: .off
            case .connecting: .connecting
            case .connected: .connected
            case .disconnected: .disconnected
            default: nil
            }
            relay = state.map { RelayStatus(state: $0, host: parsed.relay.host) }
        }
        return (parsed.devices.map(linked), parsed.hasThisDevice ? linked(parsed.thisDevice) : nil, event, relay)
    }

    func rename(name: String) throws {
        try mutation(KoruNative.rename(root: root, secrets: secrets, name: name))
        logger.event("device_renamed")
    }

    func resetIdentity() throws {
        try mutation(KoruNative.resetIdentity(root: root, secrets: secrets))
        logger.event("device_identity_reset")
    }

    func deviceHistory(deviceID: String, before: Int64?) throws -> DeviceHistoryPage {
        let parsed = try KoruNative.history(root: root, secrets: secrets, deviceID: deviceID, before: before)
        if parsed.hasErrorCode { throw failure(parsed.errorCode) }
        return DeviceHistoryPage(
            deviceId: parsed.deviceID,
            events: parsed.events.map(stateEvent),
            nextBefore: parsed.hasNextBefore ? parsed.nextBefore : nil)
    }

    /// Apply the relay on/off change to the live host (decision №6: no app
    /// restart). UserDefaults owns the persisted value; the toggle wrote it
    /// before this call, and the next open reads it back.
    func setRelay(enabled: Bool) throws {
        try mutation(KoruNative.setRelay(root: root, secrets: secrets, enabled: enabled))
        logger.event("relay_toggled", id: enabled ? "on" : "off")
    }

    /// What this device tells its peers it runs. The host writes a record only
    /// when the description changed, so a plain relaunch writes nothing.
    func advertiseBuild(_ payload: BuildPayload) throws {
        try mutation(KoruNative.advertiseBuild(root: root, secrets: secrets, payload: payload))
        logger.event("build_advertised", id: payload.label)
    }

    func inventory() throws -> Inventory {
        let parsed = try KoruNative.inventory(root: root, secrets: secrets)
        if parsed.hasErrorCode { throw failure(parsed.errorCode) }
        let profiles = parsed.profiles.map {
            Profile(
                id: ProfileID($0.id), displayName: $0.name, locale: $0.locale,
                icon: $0.hasIcon ? $0.icon : nil,
                leadingEmoji: $0.hasLeadingEmoji ? $0.leadingEmoji : nil,
                displayLabel: $0.displayName, color: $0.hasColor ? $0.color : nil)
        }
        let workspaces = parsed.workspaces.map {
            Workspace(
                id: WorkspaceID($0.id), profileID: ProfileID($0.profileID), title: $0.name,
                archived: $0.archived,
                deletedAtMillis: $0.hasDeletedAtMillis ? $0.deletedAtMillis : nil,
                modifiedAtMillis: $0.modifiedAtMillis,
                icon: $0.hasIcon ? $0.icon : nil, color: $0.hasColor ? $0.color : nil,
                leadingEmoji: $0.hasLeadingEmoji ? $0.leadingEmoji : nil,
                displayTitle: $0.displayName)
        }
        let documents = parsed.documents.map {
            DocumentSummary(
                id: DocumentID($0.id), workspaceID: WorkspaceID($0.workspaceID), title: $0.title,
                displayTitle: $0.displayTitle,
                leadingEmoji: $0.hasLeadingEmoji ? $0.leadingEmoji : nil,
                kind: $0.kind, mimeType: $0.hasMime ? $0.mime : nil,
                noteId: $0.hasNoteID ? DocumentID($0.noteID) : nil)
        }
        let deleted = Int(parsed.deletedWorkspaces)
        let trashed = Int(parsed.trashedDocuments)
        let failed = Int(parsed.failedWorkspaces)
        if deleted + trashed + failed > 0 {
            logger.event(
                "inventory_health",
                id: "deleted=\(deleted) trashed=\(trashed) failed=\(failed)")
        }
        return Inventory(profiles: profiles, workspaces: workspaces, documents: documents)
    }

    func document(id: DocumentID) throws -> DocumentSnapshot {
        let document = try KoruNative.document(root: root, secrets: secrets, documentID: id.value)
        if document.hasErrorCode { throw failure(document.errorCode) }
        let workspace = WorkspaceID(document.workspaceID)
        return DocumentSnapshot(
            id: DocumentID(document.id), workspaceID: workspace, title: document.title,
            revision: document.revision, blocks: document.blocks.map { protoBlock(from: $0, workspace: workspace) })
    }

    func mutateProfile(id: ProfileID, action: String, value: String = "") throws {
        try mutation(KoruNative.mutateProfile(
            root: root, secrets: secrets, profileID: id.value, action: action, value: value))
        logger.event("profile_change", id: id.value)
    }

    func mutateWorkspace(id: WorkspaceID, action: String, title: String = "") throws {
        try mutation(KoruNative.mutateWorkspace(
            root: root, secrets: secrets, workspaceID: id.value, action: action, title: title))
        logger.event("workspace_change", id: id.value)
    }

    func mutateDocument(id: DocumentID, action: String, title: String = "") throws {
        try mutation(KoruNative.mutateDocument(
            root: root, secrets: secrets, documentID: id.value, action: action, title: title))
        logger.event("document_change", id: id.value)
    }

    func importFile(workspaceID: WorkspaceID, name: String, mimeType: String, path: String, note: String? = nil) throws -> DocumentID {
        let parsed = try KoruNative.importFile(
            root: root, secrets: secrets, workspaceID: workspaceID.value,
            name: name, mime: mimeType, path: path, note: note)
        if parsed.hasErrorCode { throw failure(parsed.errorCode) }
        guard parsed.hasDocumentID else { throw failure("import_missing_document_id") }
        return DocumentID(parsed.documentID)
    }

    func ensureFileNote(id: DocumentID) throws -> DocumentID {
        let parsed = try KoruNative.ensureFileNote(root: root, secrets: secrets, documentID: id.value)
        if parsed.hasErrorCode { throw failure(parsed.errorCode) }
        guard parsed.hasDocumentID else { throw failure("note_missing_document_id") }
        return DocumentID(parsed.documentID)
    }

    func blob(workspaceID: WorkspaceID, id: BlobID) throws -> Data {
        let value = try KoruNative.blob(root: root, secrets: secrets, workspaceID: workspaceID.value, hash: id.value)
        if value.hasErrorCode { throw failure(value.errorCode) }
        return value.value
    }

    private func mutation(_ value: KORAPIMutationResult) throws {
        if value.hasErrorCode { throw failure(value.errorCode) }
        if !value.ok { throw failure(value.state.isEmpty ? "native_mutation_failed" : value.state) }
    }

    /// **The code is read before the state, and that ordering is the fix.**
    ///
    /// The host reports a device-identity mismatch under *two* states:
    /// `status` answers `unavailable`, while `link_device` and `start_pairing`
    /// answer `failed`. This switched on the state and only looked for the code
    /// inside the `failed` arm — and the one-second poll that drives the
    /// Devices screen goes through `status`, so a real mismatch fell to
    /// `default` and drew *"This build does not include the native sync
    /// library"*. The row existed, was tested, and could not be reached on the
    /// path a person is actually on (TECHDEBT 85).
    ///
    /// Matching the code first is the same decision Android took
    /// ([ADR-413](../../../docs/decisions/ADR-413-mismatchrow.md)); the alternative — normalising
    /// the state in the host — would have made two shims agree by changing what
    /// the host means, which is the wrong layer to fix a reader's mistake in.
    private func linkState(_ state: String, _ peer: String?, _ error: String?) -> DeviceLinkState {
        if error == "device_identity_mismatch" { return .identityMismatch }
        return switch state {
        case "unlinked": .unlinked
        case "linking": .linking(peerName: peer)
        case "linked": .linked(peerName: peer)
        case "failed": .failed(code: error ?? "pairing_failed")
        default: .unavailable(reason: error ?? "native_status")
        }
    }

    private func stateEvent(_ wire: KORAPIStateEvent) -> DeviceStateEvent {
        DeviceStateEvent(
            eventId: wire.hasEventID ? wire.eventID : nil,
            atMillis: wire.atMillis, kind: wire.kind,
            profileId: wire.hasProfileID ? wire.profileID : nil,
            workspaceId: wire.hasWorkspaceID ? wire.workspaceID : nil,
            documentId: wire.hasDocumentID ? wire.documentID : nil,
            documentName: wire.hasDocumentName ? wire.documentName : nil,
            blobHash: wire.hasBlobHash ? wire.blobHash : nil,
            detail: wire.hasDetail ? wire.detail : nil,
            items: wire.hasItems ? Int(wire.items) : nil)
    }

    private func protoBlock(from wire: KORAPIBlock, workspace: WorkspaceID) -> Block {
        let raw = wire.properties
        let alignment: KoruAlignment = switch raw.alignment {
        case "center": .center
        case "end": .end
        case "justify": .justify
        default: .start
        }
        let listStyle: KoruListStyle? = switch raw.hasListStyle ? raw.listStyle : "" {
        case "bullet": .bullet
        case "checkbox": .checkbox
        case "number", "numbered": .numbered
        case "letter": .letter
        case "roman": .roman
        default: nil
        }
        let properties = BlockProperties(
            name: raw.hasName ? raw.name : nil,
            icon: raw.hasIcon ? raw.icon : nil,
            color: raw.hasColor ? raw.color : nil,
            done: raw.done,
            alignment: alignment,
            headingLevel: raw.hasHeadingLevel ? raw.headingLevel : nil,
            listStyle: listStyle,
            imageWidth: raw.hasImageWidth ? raw.imageWidth : nil,
            imageHeight: raw.hasImageHeight ? raw.imageHeight : nil,
            language: raw.hasLanguage ? raw.language : nil)
        let rich = wire.hasRichText ? protoRichText(from: wire.richText) : nil
        let blob = wire.hasBlob ? BlobReference(
            id: BlobID(value: wire.blob.id), mimeType: wire.blob.mimeType,
            size: wire.blob.size,
            width: wire.blob.hasWidth ? wire.blob.width : nil,
            height: wire.blob.hasHeight ? wire.blob.height : nil,
            workspaceID: workspace) : nil
        let reference = wire.hasReference ? protoReference(from: wire.reference) : nil
        let content: BlockContent = switch wire.kind {
        case "paragraph": .paragraph(rich ?? RichText(text: ""))
        case "quote": .quote(rich ?? RichText(text: ""))
        case "task": .task(rich ?? RichText(text: ""))
        case "listblock": .list(properties.listStyle ?? .bullet)
        case "image": blob.map(BlockContent.image) ?? .unsupported(kindCode: 0, typeURL: "koru.native/image", payload: Data())
        case "canvas": blob.map(BlockContent.canvas) ?? .unsupported(kindCode: 0, typeURL: "koru.native/canvas", payload: Data())
        case "code": .code(rich ?? RichText(text: ""))
        case "table": .table(rich ?? RichText(text: ""))
        case "divider": .divider
        case "embed": reference.map(BlockContent.embed) ?? .unsupported(kindCode: 0, typeURL: "koru.native/embed", payload: Data())
        case "contact": .contact(rich ?? RichText(text: ""))
        default: .unsupported(kindCode: 0, typeURL: "koru.native/\(wire.kind)", payload: Data())
        }
        return Block(
            id: BlockID(wire.id),
            parentID: wire.hasParentID ? BlockID(wire.parentID) : nil,
            properties: properties, content: content)
    }

    private func protoRichText(from wire: KORAPIRichText) -> RichText {
        RichText(
            text: wire.text,
            marks: wire.marks.map { mark in
                TextMark(
                    range: UTF8Range(start: Int(mark.startUtf8), end: Int(mark.endUtf8)),
                    kind: MarkKind(rawValue: mark.kind) ?? .unknown,
                    wireKind: mark.kind,
                    value: mark.hasValue ? Data(mark.value.utf8) : Data())
            },
            inlineObjects: wire.inlineObjects.map { object in
                InlineObject(
                    id: InlineObjectID(object.id), anchorUTF8: Int(object.anchorUtf8),
                    role: object.role, reference: protoReference(from: object.reference),
                    label: object.hasLabel ? object.label : nil)
            })
    }

    private func protoReference(from wire: KORAPIEntityReference) -> EntityReference {
        switch wire.kind {
        case "document": .document(DocumentID(wire.documentID))
        case "block": .block(
            BlockID(wire.blockID),
            documentID: wire.hasDocumentID ? DocumentID(wire.documentID) : nil)
        case "url": .url(wire.url)
        case "label": .label(wire.value)
        default: .label(wire.hasValue ? wire.value : "Reference")
        }
    }

    private func block(from wire: NativeDocumentEnvelope.Block, workspace: WorkspaceID) -> Block {
        let alignment: KoruAlignment = switch wire.properties.alignment ?? "start" {
        case "center": .center
        case "end": .end
        case "justify": .justify
        default: .start
        }
        let listStyle: KoruListStyle? = switch wire.properties.listStyle {
        case "bullet": .bullet
        case "checkbox": .checkbox
        case "number", "numbered": .numbered
        case "letter": .letter
        case "roman": .roman
        default: nil
        }
        let properties = BlockProperties(
            name: wire.properties.name, icon: wire.properties.icon, color: wire.properties.color,
            done: wire.properties.done ?? false,
            alignment: alignment,
            headingLevel: (wire.properties.headingLevel).flatMap { $0 > 0 ? $0 : nil },
            listStyle: listStyle,
            imageWidth: (wire.properties.imageWidth).flatMap { $0 > 0 ? $0 : nil },
            imageHeight: (wire.properties.imageHeight).flatMap { $0 > 0 ? $0 : nil },
            language: wire.properties.language)
        let rich = wire.richText.map(richText(from:))
        let blob = wire.blob.map {
            BlobReference(
                id: BlobID(value: $0.id), mimeType: $0.mimeType, size: $0.size,
                width: ($0.width).flatMap { $0 > 0 ? $0 : nil },
                height: ($0.height).flatMap { $0 > 0 ? $0 : nil },
                workspaceID: workspace)
        }
        let reference = wire.reference.map(entityReference(from:))
        let content: BlockContent = switch wire.kind {
        case "paragraph": .paragraph(rich ?? RichText(text: ""))
        case "quote": .quote(rich ?? RichText(text: ""))
        case "task": .task(rich ?? RichText(text: ""))
        case "listblock": .list(properties.listStyle ?? .bullet)
        case "image": blob.map(BlockContent.image) ?? .unsupported(kindCode: 0, typeURL: "koru.native/image", payload: Data())
        case "canvas": blob.map(BlockContent.canvas) ?? .unsupported(kindCode: 0, typeURL: "koru.native/canvas", payload: Data())
        case "code": .code(rich ?? RichText(text: ""))
        case "table": .table(rich ?? RichText(text: ""))
        case "divider": .divider
        case "embed": reference.map(BlockContent.embed) ?? .unsupported(kindCode: 0, typeURL: "koru.native/embed", payload: Data())
        case "contact": .contact(rich ?? RichText(text: ""))
        default: .unsupported(kindCode: 0, typeURL: "koru.native/\(wire.kind)", payload: Data())
        }
        return Block(
            id: BlockID(wire.id), parentID: wire.parentId.map(BlockID.init),
            properties: properties, content: content)
    }

    private func richText(from wire: NativeDocumentEnvelope.Rich) -> RichText {
        RichText(
            text: wire.text,
            marks: wire.marks.map { mark in
                TextMark(
                    range: UTF8Range(start: mark.startUtf8, end: mark.endUtf8),
                    kind: MarkKind(rawValue: mark.kind) ?? .unknown,
                    wireKind: mark.kind,
                    value: mark.value.map { Data($0.utf8) } ?? Data())
            },
            inlineObjects: wire.inlineObjects.map { object in
                InlineObject(
                    id: InlineObjectID(object.id), anchorUTF8: object.anchorUtf8,
                    role: object.role, reference: entityReference(from: object.reference),
                    label: object.label)
            })
    }

    private func entityReference(from wire: NativeDocumentEnvelope.Reference) -> EntityReference {
        switch wire.kind {
        case "document": .document(DocumentID(wire.documentId ?? ""))
        case "block": .block(BlockID(wire.blockId ?? ""), documentID: wire.documentId.map(DocumentID.init))
        case "url": .url(wire.url ?? "")
        case "label": .label(wire.value ?? "")
        default: .label(wire.value ?? "Reference")
        }
    }

    struct Inventory: Equatable, Sendable {
        var profiles: [Profile] = []
        var workspaces: [Workspace] = []
        var documents: [DocumentSummary] = []
    }
}

struct NativeSyncEventSummary: Equatable, Sendable {
    let sequence: Int64
    let stage: String
    /// The peer's display name (Win11, Pixel8) — a log line naming only a
    /// uuid is a log nobody can act on.
    let peer: String
}

// -------------------------------------------------------------- device link

final class KoruDeviceLinkService: DeviceLinkService {
    private let core: KoruStoreCore
    private let stateBroadcast = Broadcast<DeviceLinkState>(.unlinked)
    private let devicesBroadcast = Broadcast<[LinkedDevice]>([])
    private let thisDeviceBroadcast = Broadcast<LinkedDevice?>(nil)
    private let relayStatusBroadcast = Broadcast<RelayStatus?>(nil)
    let syncEvents = Broadcast<NativeSyncEventSummary?>(nil)
    private let poll: Task<Void, Never>

    init(core: KoruStoreCore) {
        self.core = core
        let state = stateBroadcast
        let devices = devicesBroadcast
        let mine = thisDeviceBroadcast
        let relay = relayStatusBroadcast
        let events = syncEvents
        poll = Task {
            // Tell linked devices what this one runs, before the list is first
            // read — Android does the same in its service's init.
            do { try await core.advertiseBuild(BuildPayload.current()) } catch {
                AppLogger().event("build_advertised", id: "error: \(error)")
            }
            while !Task.isCancelled {
                let status = await core.status()
                // A link in flight owns the state row until it resolves.
                if await state.current != status, !Self.isLinking(await state.current) {
                    await state.set(status)
                }
                if let (snapshot, this, event, reading) = try? await core.deviceSnapshot() {
                    if await devices.current != snapshot { await devices.set(snapshot) }
                    if await mine.current != this { await mine.set(this) }
                    if await relay.current != reading { await relay.set(reading) }
                    if let event, await events.current != event { await events.set(event) }
                }
                try? await Task.sleep(for: .seconds(1))
            }
        }
    }

    deinit { poll.cancel() }

    private static func isLinking(_ state: DeviceLinkState) -> Bool {
        if case .linking = state { return true }
        return false
    }

    var state: DeviceLinkState {
        get async { await stateBroadcast.current }
    }
    var devices: [LinkedDevice] {
        get async { await devicesBroadcast.current }
    }
    var thisDevice: LinkedDevice? {
        get async { await thisDeviceBroadcast.current }
    }
    func observeThisDevice() async -> AsyncStream<LinkedDevice?> {
        await thisDeviceBroadcast.stream()
    }
    /// The relay status row's stream — like `observeThisDevice`, native-host
    /// only, so it lives beside the protocol rather than widening it.
    func observeRelayStatus() async -> AsyncStream<RelayStatus?> {
        await relayStatusBroadcast.stream()
    }
    func observeState() async -> AsyncStream<DeviceLinkState> { await stateBroadcast.stream() }
    func observeDevices() async -> AsyncStream<[LinkedDevice]> { await devicesBroadcast.stream() }

    func link(code: String) async throws {
        guard !code.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw failure("invalid_link_code")
        }
        await stateBroadcast.set(.linking())
        do {
            let landed = try await core.link(code: code)
            await stateBroadcast.set(landed)
        } catch {
            await stateBroadcast.set(.failed(code: (error as? AppFailure)?.detail ?? "pairing_failed"))
            throw error
        }
    }

    func startPairing() async throws -> PairingOffer { try await core.startPairing() }
    func cancelPairing() async { await core.cancelPairing() }
    func setRelay(enabled: Bool) async throws { try await core.setRelay(enabled: enabled) }

    func rename(name: String) async throws {
        try await core.rename(name: name)
        if let (_, this, _, _) = try? await core.deviceSnapshot() {
            await thisDeviceBroadcast.set(this)
        }
    }

    func unlink(deviceID: String) async throws {
        try await core.unlink(deviceID: deviceID)
        if let (snapshot, _, _, _) = try? await core.deviceSnapshot() {
            await devicesBroadcast.set(snapshot)
        }
        await stateBroadcast.set(await core.status())
    }

    func history(deviceID: String, before: Int64?) async throws -> DeviceHistoryPage {
        try await core.deviceHistory(deviceID: deviceID, before: before)
    }

    func resetIdentity() async throws {
        try await core.resetIdentity()
        // The fresh identity should say what it runs before anyone sees it.
        try? await core.advertiseBuild(BuildPayload.current())
        if let (snapshot, this, _, _) = try? await core.deviceSnapshot() {
            await devicesBroadcast.set(snapshot)
            await thisDeviceBroadcast.set(this)
        }
        await stateBroadcast.set(await core.status())
    }
}

// ----------------------------------------------------------------- content

/// Mobile projection of the embedded, replicated Store: one 500 ms inventory
/// poll feeding every observe stream, mirrors of Kotlin's services.
final class KoruContentServices: ProfileService, WorkspaceService, DocumentService, BlobService,
    WorkspaceFileService
{
    private let core: KoruStoreCore
    private let deviceLink: KoruDeviceLinkService
    private let inventory = Broadcast<KoruStoreCore.Inventory>(.init())
    private let syncBroadcast = Broadcast<ContentSyncStatus>(.init())
    private let logger = AppLogger()
    private let poll: Task<Void, Never>

    init(core: KoruStoreCore, deviceLink: KoruDeviceLinkService) {
        self.core = core
        self.deviceLink = deviceLink
        let inventory = inventory
        let sync = syncBroadcast
        let events = deviceLink.syncEvents
        poll = Task {
            var exported: ShareDestinations?
            while !Task.isCancelled {
                if let next = try? await core.inventory() {
                    if await inventory.current != next { await inventory.set(next) }
                    // The share extension's picker reads this snapshot from
                    // the App Group — display hints only; the import
                    // validates. Written when the content moved, so a
                    // renamed workspace shows its new name at the next share.
                    let destinations = Self.destinations(of: next)
                    if exported.map({ Self.sameContent($0, destinations) }) != true {
                        try? destinations.write()
                        exported = destinations
                    }
                    let syncing: Bool = switch await events.current?.stage {
                    case nil: await sync.current.syncing
                    case "sync_complete", "catalog_failed": false
                    default: true
                    }
                    let status = ContentSyncStatus(
                        syncing: syncing,
                        profileCount: next.profiles.count,
                        workspaceCount: next.workspaces.count,
                        documentCount: next.documents.count)
                    if await sync.current != status { await sync.set(status) }
                }
                try? await Task.sleep(for: .milliseconds(500))
            }
        }
    }

    deinit { poll.cancel() }

    func syncStatusStream() -> AsyncStream<ContentSyncStatus> {
        let broadcast = syncBroadcast
        return AsyncStream { continuation in
            let task = Task {
                for await value in await broadcast.stream() { continuation.yield(value) }
                continuation.finish()
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    /// The inventory, shaped for the extension's picker: live workspaces
    /// only, under their profiles.
    private static func destinations(of inventory: KoruStoreCore.Inventory) -> ShareDestinations {
        ShareDestinations(
            profiles: inventory.profiles.map {
                ShareDestinations.Profile(
                    id: $0.id.value, label: $0.displayLabel,
                    leadingEmoji: $0.icon ?? $0.leadingEmoji, color: $0.color)
            },
            workspaces: inventory.workspaces
                .filter { !$0.archived && !$0.deleted }
                .map {
                    ShareDestinations.Workspace(
                        id: $0.id.value, profileId: $0.profileID.value,
                        label: $0.displayTitle,
                        leadingEmoji: $0.leadingEmoji, color: $0.color)
                },
            generation: Int64(Date().timeIntervalSince1970 * 1000))
    }

    /// Equal apart from `generation`, which moves every export by design.
    private static func sameContent(_ a: ShareDestinations, _ b: ShareDestinations) -> Bool {
        a.profiles == b.profiles && a.workspaces == b.workspaces
    }

    // Profiles ------------------------------------------------------------

    func observeProfiles() async -> AsyncThrowingStream<[Profile], Error> {
        derived { $0.profiles }
    }
    func profile(id: ProfileID) async throws -> Profile {
        guard let row = await inventory.current.profiles.first(where: { $0.id == id }) else {
            throw AppFailure(code: .notFound, detail: "profile \(id.value) was not found")
        }
        return row
    }
    func createProfile(displayName: String, locale: String) async throws -> Profile {
        throw AppFailure(code: .unsupported, detail: "profiles are created on the desktop")
    }
    func rename(id: ProfileID, displayName: String) async throws -> Profile {
        try await core.mutateProfile(id: id, action: "rename", value: displayName)
        await refresh()
        return try await profile(id: id)
    }

    // Workspaces ----------------------------------------------------------

    func observeWorkspaces(profileID: ProfileID) async -> AsyncThrowingStream<[Workspace], Error> {
        derived { $0.workspaces.filter { $0.profileID == profileID } }
    }
    func workspace(id: WorkspaceID) async throws -> Workspace {
        guard let row = await inventory.current.workspaces.first(where: { $0.id == id }) else {
            throw AppFailure(code: .notFound, detail: "workspace \(id.value) was not found")
        }
        return row
    }
    func createWorkspace(profileID: ProfileID, title: String) async throws -> Workspace {
        throw AppFailure(code: .unsupported, detail: "workspaces are created on the desktop")
    }
    func rename(id: WorkspaceID, title: String) async throws -> Workspace {
        try await mutateWorkspace(id: id, action: "rename", title: title)
    }
    func setArchived(id: WorkspaceID, archived: Bool) async throws -> Workspace {
        try await mutateWorkspace(id: id, action: archived ? "archive" : "unarchive")
    }
    func delete(id: WorkspaceID) async throws -> Workspace {
        try await mutateWorkspace(id: id, action: "delete")
    }
    func restore(id: WorkspaceID) async throws -> Workspace {
        try await mutateWorkspace(id: id, action: "restore")
    }

    private func mutateWorkspace(id: WorkspaceID, action: String, title: String = "") async throws -> Workspace {
        try await core.mutateWorkspace(id: id, action: action, title: title)
        await refresh()
        return try await workspace(id: id)
    }

    // Documents -----------------------------------------------------------

    func observeDocuments(workspaceID: WorkspaceID) async -> AsyncThrowingStream<[DocumentSummary], Error> {
        derived { $0.documents.filter { $0.workspaceID == workspaceID } }
    }
    func document(id: DocumentID) async throws -> DocumentSummary {
        guard let row = await inventory.current.documents.first(where: { $0.id == id }) else {
            return try await resolve(id)
        }
        return row
    }

    /// The store catalog, not the polled inventory, says where a document
    /// lives. The inventory is a cache refreshed every half second and only
    /// as complete as the last sync pass, so a link into a workspace this
    /// device has not finished pulling must still resolve — the native layer
    /// locates the workspace itself and names the layer that failed.
    private func resolve(_ id: DocumentID) async throws -> DocumentSummary {
        do {
            let snapshot = try await core.document(id: id)
            return DocumentSummary(
                id: snapshot.id, workspaceID: snapshot.workspaceID, title: snapshot.title,
                displayTitle: nil, leadingEmoji: nil, kind: "document", mimeType: nil)
        } catch let error as AppFailure {
            let code = error.detail
            if code == "document_unlocated" || code == "document_not_listed" {
                throw AppFailure(
                    code: .notFound, detail: "document \(id.value) was not found (\(code))")
            }
            throw error
        }
    }
    func createDocument(workspaceID: WorkspaceID, title: String) async throws -> DocumentSummary {
        throw AppFailure(code: .unsupported, detail: "mobile document creation is not part of this slice")
    }
    func rename(id: DocumentID, title: String) async throws -> DocumentSummary {
        try await core.mutateDocument(id: id, action: "rename", title: title)
        await refresh()
        return try await document(id: id)
    }
    func moveToTrash(id: DocumentID) async throws {
        try await core.mutateDocument(id: id, action: "trash")
        await refresh()
    }
    func openDocument(id: DocumentID) async throws -> any DocumentSession {
        _ = try await document(id: id) // resolves through the catalog on a cache miss

        return KoruReadDocumentSession(core: core, documentID: id)
    }

    // Blobs ---------------------------------------------------------------

    func observeAvailability(id: BlobID) async -> AsyncThrowingStream<Bool, Error> {
        AsyncThrowingStream { continuation in
            continuation.yield(true)
            continuation.finish()
        }
    }

    func observeLoad(_ reference: BlobReference) -> AsyncStream<BlobLoadState> {
        let core = core
        let deviceLink = deviceLink
        return AsyncStream { continuation in
            let task = Task {
                continuation.yield(.checkingLocal)
                let peerStates = Task {
                    for await devices in await deviceLink.observeDevices() {
                        let state: BlobLoadState =
                            if devices.isEmpty {
                                .noLinkedDevices
                            } else if !devices.contains(where: { $0.connected }) {
                                .waitingForPeer(deviceName: devices.first?.displayName)
                            } else {
                                .fetching(receivedBytes: 0, totalBytes: Int64(reference.size))
                            }
                        continuation.yield(state)
                    }
                }
                defer { peerStates.cancel() }
                guard let workspace = reference.workspaceID else {
                    continuation.yield(.failed(code: "image_has_no_workspace", retryable: false))
                    continuation.finish()
                    return
                }
                // **A device that has the picture being offline is a wait, not
                // a failure.** The host answers `blob_waiting_for_peer`,
                // `blob_not_found` or `no_linked_devices` at once rather than
                // blocking (34d4aefd7), so the placeholder says who it waits
                // for and the load is asked again whenever a linked device
                // connects — Android's `blobLoadFailure`, here (I14).
                var attempts = 0
                while !Task.isCancelled {
                    attempts += 1
                    do {
                        let bytes = try await core.blob(workspaceID: workspace, id: reference.id)
                        peerStates.cancel()
                        continuation.yield(.available(bytes))
                        break
                    } catch {
                        peerStates.cancel()
                        let code = (error as? AppFailure)?.detail ?? "blob_load_failed"
                        let devices = await deviceLink.devices
                        let offline = devices.first(where: { !$0.connected })?.displayName
                        let state = Self.loadFailure(code: code, waitingOn: offline)
                        continuation.yield(state)
                        guard case .failed = state else {
                            // Bounded so a peer that keeps connecting without the
                            // bytes cannot make this a loop nobody asked for.
                            guard attempts < Self.maxPeerRetries,
                                  await Self.waitForANewConnection(deviceLink, after: devices)
                            else { break }
                            continuation.yield(.fetching(receivedBytes: 0, totalBytes: Int64(reference.size)))
                            continue
                        }
                        break
                    }
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    func put(_ bytes: Data, mimeType: String, width: UInt32?, height: UInt32?) async throws -> BlobReference {
        throw AppFailure(code: .unsupported, detail: "mobile blob upload is not part of this slice")
    }

    func load(_ reference: BlobReference) async throws -> Data {
        guard let workspace = reference.workspaceID else {
            throw AppFailure(code: .invalidData, detail: "image has no workspace")
        }
        return try await core.blob(workspaceID: workspace, id: reference.id)
    }

    /// How many times one placeholder asks again after a device connects.
    static let maxPeerRetries = 20

    /// The codes that mean the bytes exist on a device this one cannot reach
    /// yet — Android's `BLOB_NOT_HELD_CODES`.
    static let notHeldCodes: Set<String> = ["blob_not_found", "blob_waiting_for_peer"]

    /// What a picture load that produced no bytes shows, chosen from the
    /// host's stable code. Android's `blobLoadFailure`: not-held is a wait
    /// naming the device known to have it (if any), nothing linked keeps its
    /// own remedy, and only genuine damage is a failure.
    static func loadFailure(code: String, waitingOn deviceName: String? = nil) -> BlobLoadState {
        if notHeldCodes.contains(code) {
            return .waitingForPeer(deviceName: deviceName)
        }
        if code == "no_linked_devices" {
            return .noLinkedDevices
        }
        return .failed(code: code, retryable: retryable(code))
    }

    /// Resolve when a linked device connects that was not connected in
    /// `before` — a *new* connection, so a connected peer that lacks the bytes
    /// does not make the caller ask again at once. `false` if the stream ends.
    private static func waitForANewConnection(
        _ deviceLink: KoruDeviceLinkService, after before: [LinkedDevice]
    ) async -> Bool {
        let connectedBefore = Set(before.filter(\.connected).map(\.deviceID))
        for await devices in await deviceLink.observeDevices() {
            if Task.isCancelled { return false }
            if devices.contains(where: { $0.connected && !connectedBefore.contains($0.deviceID) }) {
                return true
            }
        }
        return false
    }

    private static func retryable(_ code: String) -> Bool {
        [
            "runtime_closed", "store_unavailable", "internal", "open", "read_length",
            "read_body", "write_length", "write_body", "finish", "timeout",
        ].contains(code)
    }

    // Shared content ------------------------------------------------------

    func save(workspaceID: WorkspaceID, share: IncomingShare) async throws -> [DocumentSummary] {
        guard !share.files.isEmpty else {
            throw AppFailure(code: .invalidData, detail: "shared content is empty")
        }
        var imported: [DocumentID] = []
        do {
            for incoming in share.files {
                let staged = try Self.stage(incoming)
                defer { try? FileManager.default.removeItem(at: staged) }
                imported.append(
                    try await core.importFile(
                        workspaceID: workspaceID,
                        name: incoming.name,
                        mimeType: incoming.mimeType,
                        path: staged.path,
                        note: incoming.note))
            }
        } catch {
            // A Store import is committed the moment it returns, so a later
            // item failing cannot be undone by deleting a temporary file. The
            // batch is still all-or-nothing from the sharer's side: what
            // landed is binned again, best effort.
            for id in imported {
                try? await core.mutateDocument(id: id, action: "trash")
            }
            await refresh()
            throw error
        }
        await refresh()
        logger.event("shared_content_saved", id: workspaceID.value)
        var summaries: [DocumentSummary] = []
        for id in imported {
            if let row = try? await document(id: id) { summaries.append(row) }
        }
        return summaries
    }

    /// The file's one note, created (empty, seeded) on first ask — *Add note*.
    func ensureNote(file: DocumentID) async throws -> DocumentID {
        let note = try await core.ensureFileNote(id: file)
        await refresh()
        return note
    }

    /// The path, not the bytes, crosses the native boundary — a share can be
    /// a video, and the host reads from disk regardless.
    private static func stage(_ incoming: IncomingSharedFile) throws -> URL {
        let scratch = FileManager.default.temporaryDirectory
            .appendingPathComponent("shared-content", isDirectory: true)
        try FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
        let destination = scratch.appendingPathComponent(UUID().uuidString)
        let input = try incoming.open()
        input.open()
        defer { input.close() }
        guard FileManager.default.createFile(atPath: destination.path, contents: nil) else {
            throw AppFailure(code: .invalidData, detail: "could not stage shared content")
        }
        let output = try FileHandle(forWritingTo: destination)
        defer { try? output.close() }
        let bufferSize = 128 * 1024
        var buffer = [UInt8](repeating: 0, count: bufferSize)
        while true {
            let read = input.read(&buffer, maxLength: bufferSize)
            if read < 0 {
                throw AppFailure(code: .invalidData, detail: "could not read shared content")
            }
            if read == 0 { break }
            try output.write(contentsOf: Data(buffer[0..<read]))
        }
        return destination
    }

    // Plumbing ------------------------------------------------------------

    private func refresh() async {
        if let next = try? await core.inventory() {
            if await inventory.current != next { await inventory.set(next) }
        }
    }

    private func derived<Row: Equatable & Sendable>(
        _ project: @escaping @Sendable (KoruStoreCore.Inventory) -> [Row]
    ) -> AsyncThrowingStream<[Row], Error> {
        let inventory = inventory
        return AsyncThrowingStream { continuation in
            let task = Task {
                var last: [Row]?
                for await snapshot in await inventory.stream() {
                    let rows = project(snapshot)
                    if rows != last {
                        last = rows
                        continuation.yield(rows)
                    }
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }
}

/// Read-only, like Android's `NativeReadDocumentSession`: a 500 ms poll of the
/// projected document, distinct snapshots only.
private final class KoruReadDocumentSession: DocumentSession {
    let documentID: DocumentID
    let snapshots: AsyncThrowingStream<DocumentSnapshot, Error>
    let events: AsyncThrowingStream<DocumentEvent, Error>
    private let stop: @Sendable () -> Void

    init(core: KoruStoreCore, documentID: DocumentID) {
        self.documentID = documentID
        let (events, eventContinuation) = AsyncThrowingStream<DocumentEvent, Error>.makeStream()
        eventContinuation.finish()
        self.events = events
        let (snapshots, continuation) = AsyncThrowingStream<DocumentSnapshot, Error>.makeStream()
        self.snapshots = snapshots
        let task = Task {
            var last: DocumentSnapshot?
            while !Task.isCancelled {
                do {
                    let snapshot = try await core.document(id: documentID)
                    if snapshot != last {
                        last = snapshot
                        continuation.yield(snapshot)
                    }
                } catch {
                    continuation.finish(throwing: error)
                    return
                }
                try? await Task.sleep(for: .milliseconds(500))
            }
            continuation.finish()
        }
        stop = { task.cancel() }
        continuation.onTermination = { _ in task.cancel() }
    }

    func execute(_ command: DocumentCommand, operationID: UUID) async throws {
        throw AppFailure(code: .unsupported, detail: "documents are read-only on mobile")
    }

    func close() async { stop() }
}

// ------------------------------------------------------------------ wiring

extension AppServices {
    /// The embedded Store, or the mock when the vault or filesystem refuses —
    /// the iOS analogue of Android's `loadLibrary` fallback. **The fallback
    /// names its reason** in `implementation`, because a mock that looks right
    /// is how "linking is broken" gets reported instead of the actual failure
    /// — found exactly that way: an unsigned simulator build has no
    /// application identifier, the Keychain refuses it, and the only symptom
    /// was a Devices screen that would not link.
    static func native() -> AppServices {
        do {
            let root = try KoruSecretVault.storeRoot().path
            let secrets = try KoruSecretVault().loadOrCreate()
            let core = KoruStoreCore(root: root, secrets: secrets)
            let deviceLink = KoruDeviceLinkService(core: core)
            let content = KoruContentServices(core: core, deviceLink: deviceLink)
            return AppServices(
                profiles: content,
                workspaces: content,
                documents: content,
                blobs: content,
                deviceLink: deviceLink,
                files: content,
                syncStatus: { content.syncStatusStream() },
                implementation: "native-link + replicated-store")
        } catch {
            AppLogger().event("native_services_unavailable", id: String(describing: error))
            var services = AppServices.mock()
            services = AppServices(
                profiles: services.profiles,
                workspaces: services.workspaces,
                documents: services.documents,
                blobs: services.blobs,
                implementation: "mock — native unavailable: \(error)")
            return services
        }
    }
}

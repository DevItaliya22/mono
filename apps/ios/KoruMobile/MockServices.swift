// SPDX-License-Identifier: MPL-2.0

import CryptoKit
import Foundation
import OSLog

private let objectReplacement = "\u{FFFC}"

struct AppLogger: Sendable {
    private let logger = Logger(subsystem: "do.koru.app", category: "mobile")

    func event(_ name: String, id: String? = nil) {
        if let id { logger.debug("\(name, privacy: .public) id=\(id, privacy: .public)") }
        else { logger.debug("\(name, privacy: .public)") }
    }

    func timed<T: Sendable>(_ name: String, id: String? = nil, operation: @Sendable () async throws -> T) async rethrows -> T {
        let clock = ContinuousClock()
        let start = clock.now
        defer { logger.debug("\(name, privacy: .public) id=\(id ?? "-", privacy: .public) duration=\(start.duration(to: clock.now).description, privacy: .public)") }
        return try await operation()
    }
}

actor MockKoruStore {
    private var profiles: [Profile] = []
    private var workspaces: [Workspace] = []
    private var summaries: [DocumentSummary] = []
    private var documents: [DocumentID: DocumentSnapshot] = [:]
    private var blobs: [BlobID: Data] = [:]
    private var applied: [DocumentID: [UUID: DocumentCommand]] = [:]
    private var profileObservers: [UUID: AsyncThrowingStream<[Profile], Error>.Continuation] = [:]
    private var workspaceObservers: [ProfileID: [UUID: AsyncThrowingStream<[Workspace], Error>.Continuation]] = [:]
    private var documentObservers: [WorkspaceID: [UUID: AsyncThrowingStream<[DocumentSummary], Error>.Continuation]] = [:]
    private var snapshotObservers: [DocumentID: [UUID: AsyncThrowingStream<DocumentSnapshot, Error>.Continuation]] = [:]
    private var eventObservers: [DocumentID: [UUID: AsyncThrowingStream<DocumentEvent, Error>.Continuation]] = [:]
    private var blobObservers: [BlobID: [UUID: AsyncThrowingStream<Bool, Error>.Continuation]] = [:]
    private let logger = AppLogger()

    init() {
        let seeded = Self.seedData()
        profiles = seeded.profiles
        workspaces = seeded.workspaces
        summaries = seeded.summaries
        documents = seeded.documents
        blobs = seeded.blobs
    }

    func profileStream() -> AsyncThrowingStream<[Profile], Error> {
        let token = UUID()
        let (stream, continuation) = AsyncThrowingStream<[Profile], Error>.makeStream()
        profileObservers[token] = continuation
        continuation.yield(profiles)
        return stream
    }

    func workspaceStream(profileID: ProfileID) -> AsyncThrowingStream<[Workspace], Error> {
        let token = UUID()
        let (stream, continuation) = AsyncThrowingStream<[Workspace], Error>.makeStream()
        workspaceObservers[profileID, default: [:]][token] = continuation
        continuation.yield(workspaces.filter { $0.profileID == profileID })
        return stream
    }

    func documentStream(workspaceID: WorkspaceID) -> AsyncThrowingStream<[DocumentSummary], Error> {
        let token = UUID()
        let (stream, continuation) = AsyncThrowingStream<[DocumentSummary], Error>.makeStream()
        documentObservers[workspaceID, default: [:]][token] = continuation
        continuation.yield(summaries.filter { $0.workspaceID == workspaceID })
        return stream
    }

    func blobStream(id: BlobID) -> AsyncThrowingStream<Bool, Error> {
        let token = UUID()
        let (stream, continuation) = AsyncThrowingStream<Bool, Error>.makeStream()
        blobObservers[id, default: [:]][token] = continuation
        continuation.yield(blobs[id] != nil)
        return stream
    }

    func profile(id: ProfileID) throws -> Profile { try profiles.first { $0.id == id }.orNotFound("profile") }
    func workspace(id: WorkspaceID) throws -> Workspace { try workspaces.first { $0.id == id }.orNotFound("workspace") }
    func summary(id: DocumentID) throws -> DocumentSummary { try summaries.first { $0.id == id }.orNotFound("document") }

    func createProfile(displayName: String, locale: String) throws -> Profile {
        let name = try label(displayName)
        guard Locale(identifier: locale).identifier.isEmpty == false else { throw invalid("locale") }
        let value = Profile(id: ProfileID(), displayName: name, locale: locale)
        profiles.append(value)
        profileObservers.values.forEach { $0.yield(profiles) }
        logger.event("profile_create", id: value.id.value)
        return value
    }

    func renameProfile(id: ProfileID, displayName: String) throws -> Profile {
        let name = try label(displayName)
        let existing = try profile(id: id)
        let renamed = Profile(
            id: existing.id, displayName: name, locale: existing.locale,
            icon: existing.icon, leadingEmoji: existing.leadingEmoji,
            displayLabel: nil, color: existing.color)
        profiles = profiles.map { $0.id == id ? renamed : $0 }
        profileObservers.values.forEach { $0.yield(profiles) }
        logger.event("profile_change", id: id.value)
        return renamed
    }

    func createWorkspace(profileID: ProfileID, title: String) throws -> Workspace {
        _ = try profile(id: profileID)
        let value = Workspace(id: WorkspaceID(), profileID: profileID, title: try label(title))
        workspaces.append(value)
        workspaceObservers[profileID]?.values.forEach { $0.yield(workspaces.filter { $0.profileID == profileID }) }
        logger.event("workspace_create", id: value.id.value)
        return value
    }

    func mutateWorkspace(id: WorkspaceID, transform: (Workspace) -> Workspace) throws -> Workspace {
        let existing = try workspace(id: id)
        let next = transform(existing)
        workspaces = workspaces.map { $0.id == id ? next : $0 }
        workspaceObservers[next.profileID]?.values.forEach { $0.yield(workspaces.filter { $0.profileID == next.profileID }) }
        return next
    }

    func renameDocumentSummary(id: DocumentID, title: String) throws -> DocumentSummary {
        var summary = try summary(id: id)
        summary = DocumentSummary(
            id: summary.id, workspaceID: summary.workspaceID, title: title,
            leadingEmoji: summary.leadingEmoji, kind: summary.kind, mimeType: summary.mimeType)
        summaries = summaries.map { $0.id == id ? summary : $0 }
        if var document = documents[id] {
            document.title = title
            documents[id] = document
        }
        documentObservers[summary.workspaceID]?.values.forEach { $0.yield(summaries.filter { $0.workspaceID == summary.workspaceID }) }
        return summary
    }

    func trashDocument(id: DocumentID) throws {
        let summary = try summary(id: id)
        summaries.removeAll { $0.id == id }
        documents[id] = nil
        documentObservers[summary.workspaceID]?.values.forEach { $0.yield(summaries.filter { $0.workspaceID == summary.workspaceID }) }
    }

    func createDocument(workspaceID: WorkspaceID, title: String) throws -> DocumentSummary {
        _ = try workspace(id: workspaceID)
        let value = DocumentSummary(id: DocumentID(), workspaceID: workspaceID, title: try label(title))
        let paragraph = Block(id: BlockID(), content: .paragraph(RichText(text: "")))
        summaries.append(value)
        documents[value.id] = DocumentSnapshot(id: value.id, workspaceID: workspaceID, title: value.title, revision: 0, blocks: [paragraph])
        documentObservers[workspaceID]?.values.forEach { $0.yield(summaries.filter { $0.workspaceID == workspaceID }) }
        logger.event("document_create", id: value.id.value)
        return value
    }

    func open(id: DocumentID) throws -> MockDocumentSession {
        _ = try summary(id: id)
        let snapshotToken = UUID()
        let eventToken = UUID()
        let (snapshots, snapshotContinuation) = AsyncThrowingStream<DocumentSnapshot, Error>.makeStream()
        snapshotObservers[id, default: [:]][snapshotToken] = snapshotContinuation
        snapshotContinuation.yield(documents[id]!)
        let (events, eventContinuation) = AsyncThrowingStream<DocumentEvent, Error>.makeStream()
        eventObservers[id, default: [:]][eventToken] = eventContinuation
        return MockDocumentSession(store: self, documentID: id, snapshotToken: snapshotToken, eventToken: eventToken, snapshots: snapshots, events: events)
    }

    func close(id: DocumentID, snapshotToken: UUID, eventToken: UUID) {
        snapshotObservers[id]?.removeValue(forKey: snapshotToken)?.finish()
        eventObservers[id]?.removeValue(forKey: eventToken)?.finish()
    }

    func execute(id: DocumentID, command: DocumentCommand, operationID: UUID) throws {
        if let prior = applied[id]?[operationID] {
            guard prior == command else { throw invalid("operation_id_reused") }
            return
        }
        guard var document = documents[id] else { throw notFound("document") }
        let outcome = try apply(command, to: document)
        document = outcome.snapshot
        documents[id] = document
        applied[id, default: [:]][operationID] = command
        summaries = summaries.map { $0.id == id ? DocumentSummary(id: $0.id, workspaceID: $0.workspaceID, title: document.title) : $0 }
        snapshotObservers[id]?.values.forEach { $0.yield(document) }
        eventObservers[id]?.values.forEach { $0.yield(outcome.event ?? .changed(revision: document.revision, blocks: outcome.changed)) }
        documentObservers[document.workspaceID]?.values.forEach { $0.yield(summaries.filter { $0.workspaceID == document.workspaceID }) }
        logger.event("document_command", id: id.value)
    }

    func simulateExternal(id: DocumentID, command: DocumentCommand) throws { try execute(id: id, command: command, operationID: UUID()) }

    func putBlob(_ bytes: Data, mimeType: String, width: UInt32?, height: UInt32?) throws -> BlobReference {
        guard !bytes.isEmpty, !mimeType.isEmpty else { throw invalid("blob") }
        let id = BlobID(value: "sha256:" + SHA256.hash(data: bytes).map { String(format: "%02x", $0) }.joined())
        blobs[id] = bytes
        blobObservers[id]?.values.forEach { $0.yield(true) }
        return BlobReference(id: id, mimeType: mimeType, size: UInt64(bytes.count), width: width, height: height)
    }

    func loadBlob(id: BlobID) throws -> Data { guard let value = blobs[id] else { throw notFound("blob") }; return value }

    private func apply(_ command: DocumentCommand, to source: DocumentSnapshot) throws -> (snapshot: DocumentSnapshot, changed: Set<BlockID>, event: DocumentEvent?) {
        var next = source
        var changed: Set<BlockID> = []
        var event: DocumentEvent?
        switch command {
        case let .createBlock(block, siblingIndex):
            guard !next.blocks.contains(where: { $0.id == block.id }) else { throw invalid("duplicate_block") }
            if let parent = block.parentID { _ = try requireBlock(parent, in: next.blocks) }
            next.blocks.insert(block, at: insertionIndex(in: next.blocks, parentID: block.parentID, siblingIndex: siblingIndex))
            changed = [block.id]
        case let .deleteBlock(id):
            _ = try requireBlock(id, in: next.blocks)
            let removed = descendants(of: id, in: next.blocks)
            next.blocks.removeAll { removed.contains($0.id) }
            changed = removed
        case let .moveBlock(id, parentID, siblingIndex):
            next.blocks = try move(id, parentID: parentID, siblingIndex: siblingIndex, in: next.blocks)
            changed = [id]
        case let .indentBlock(id):
            let target = try requireBlock(id, in: next.blocks)
            let siblings = next.blocks.filter { $0.parentID == target.parentID }
            guard let position = siblings.firstIndex(where: { $0.id == id }), position > 0 else { throw invalid("indent") }
            next.blocks = try move(id, parentID: siblings[position - 1].id, siblingIndex: .max, in: next.blocks)
            changed = [id]
        case let .outdentBlock(id):
            let target = try requireBlock(id, in: next.blocks)
            guard let parentID = target.parentID else { throw invalid("outdent") }
            let parent = try requireBlock(parentID, in: next.blocks)
            let siblings = next.blocks.filter { $0.parentID == parent.parentID }
            let index = (siblings.firstIndex(where: { $0.id == parentID }) ?? 0) + 1
            next.blocks = try move(id, parentID: parent.parentID, siblingIndex: index, in: next.blocks)
            changed = [id]
        case let .insertText(id, offset, text):
            guard !text.contains(objectReplacement) else { throw invalid("inline_object") }
            try updateRichText(id, in: &next.blocks) { try insert(text, at: offset, into: $0) }
            changed = [id]
        case let .deleteText(id, range):
            try updateRichText(id, in: &next.blocks) { try delete(range, from: $0, allowObject: false) }
            changed = [id]
        case let .addMark(id, mark):
            try updateRichText(id, in: &next.blocks) { rich in
                _ = try stringRange(mark.range, in: rich.text)
                var copy = rich; copy.marks.append(mark); return copy
            }
            changed = [id]
        case let .removeMark(id, range, kind):
            try updateRichText(id, in: &next.blocks) { rich in
                _ = try stringRange(range, in: rich.text)
                var copy = rich; copy.marks.removeAll { $0.kind == kind && $0.range == range }; return copy
            }
            changed = [id]
        case let .toggleTask(id, done):
            let block = try requireBlock(id, in: next.blocks)
            guard case .task = block.content else { throw invalid("task") }
            next.blocks = next.blocks.map { value in var value = value; if value.id == id { value.properties.done = done }; return value }
            changed = [id]
        case let .insertInlineObject(id, object):
            try updateRichText(id, in: &next.blocks) { rich in
                var copy = try insert(objectReplacement, at: object.anchorUTF8, into: rich)
                copy.inlineObjects.append(object); return copy
            }
            changed = [id]
        case let .removeInlineObject(id, objectID):
            try updateRichText(id, in: &next.blocks) { rich in
                guard let object = rich.inlineObjects.first(where: { $0.id == objectID }) else { throw notFound("inline_object") }
                var copy = rich; copy.inlineObjects.removeAll { $0.id == objectID }
                return try delete(UTF8Range(start: object.anchorUTF8, end: object.anchorUTF8 + 3), from: copy, allowObject: true)
            }
            changed = [id]
        case let .attachBlob(id, blob):
            let block = try requireBlock(id, in: next.blocks)
            guard case .image = block.content else { throw invalid("image") }
            next.blocks = next.blocks.map { value in var value = value; if value.id == id { value.content = .image(blob) }; return value }
            changed = [id]; event = .blobAvailable(blob.id)
        case let .renameDocument(title):
            next.title = try label(title); event = .renamed(next.title)
        }
        next.revision += 1
        return (next, changed, event)
    }

    private func updateRichText(_ id: BlockID, in blocks: inout [Block], transform: (RichText) throws -> RichText) throws {
        guard let index = blocks.firstIndex(where: { $0.id == id }) else { throw notFound("block") }
        switch blocks[index].content {
        case let .paragraph(rich): blocks[index].content = .paragraph(try transform(rich))
        case let .task(rich): blocks[index].content = .task(try transform(rich))
        default: throw invalid("editable_text")
        }
    }

    private func insert(_ value: String, at offset: Int, into rich: RichText) throws -> RichText {
        let index = try stringIndex(offset, in: rich.text)
        let delta = value.utf8.count
        var copy = rich
        copy.text.insert(contentsOf: value, at: index)
        copy.marks = copy.marks.map { mark in
            if mark.range.start >= offset { return TextMark(range: UTF8Range(start: mark.range.start + delta, end: mark.range.end + delta), kind: mark.kind, wireKind: mark.wireKind, value: mark.value) }
            return mark
        }
        copy.inlineObjects = copy.inlineObjects.map { object in
            object.anchorUTF8 >= offset ? InlineObject(id: object.id, anchorUTF8: object.anchorUTF8 + delta, role: object.role, reference: object.reference, label: object.label) : object
        }
        return copy
    }

    private func delete(_ range: UTF8Range, from rich: RichText, allowObject: Bool) throws -> RichText {
        let indices = try stringRange(range, in: rich.text)
        if !allowObject, rich.inlineObjects.contains(where: { range.start <= $0.anchorUTF8 && $0.anchorUTF8 < range.end }) { throw invalid("inline_object") }
        var copy = rich
        copy.text.removeSubrange(indices)
        let delta = range.end - range.start
        copy.marks = copy.marks.compactMap { mark in
            if mark.range.end <= range.start { return mark }
            if mark.range.start >= range.end { return TextMark(range: UTF8Range(start: mark.range.start - delta, end: mark.range.end - delta), kind: mark.kind, wireKind: mark.wireKind, value: mark.value) }
            return nil
        }
        copy.inlineObjects = copy.inlineObjects.compactMap { object in
            if range.start <= object.anchorUTF8 && object.anchorUTF8 < range.end { return allowObject ? nil : object }
            return object.anchorUTF8 >= range.end ? InlineObject(id: object.id, anchorUTF8: object.anchorUTF8 - delta, role: object.role, reference: object.reference, label: object.label) : object
        }
        return copy
    }

    private func move(_ id: BlockID, parentID: BlockID?, siblingIndex: Int, in source: [Block]) throws -> [Block] {
        _ = try requireBlock(id, in: source)
        let moved = descendants(of: id, in: source)
        if let parentID { _ = try requireBlock(parentID, in: source); guard !moved.contains(parentID) else { throw invalid("cycle") } }
        var segment = source.filter { moved.contains($0.id) }
        segment[0].parentID = parentID
        var remainder = source.filter { !moved.contains($0.id) }
        remainder.insert(contentsOf: segment, at: insertionIndex(in: remainder, parentID: parentID, siblingIndex: siblingIndex))
        return remainder
    }

    private func insertionIndex(in blocks: [Block], parentID: BlockID?, siblingIndex: Int) -> Int {
        let siblings = blocks.enumerated().filter { $0.element.parentID == parentID }
        if siblings.isEmpty { return parentID.flatMap { id in blocks.firstIndex(where: { $0.id == id }).map { $0 + 1 } } ?? blocks.endIndex }
        if siblingIndex >= 0 && siblingIndex < siblings.count { return siblings[siblingIndex].offset }
        let last = siblings.last!.element.id
        let family = descendants(of: last, in: blocks)
        return (blocks.lastIndex { family.contains($0.id) } ?? blocks.endIndex - 1) + 1
    }

    private func descendants(of root: BlockID, in blocks: [Block]) -> Set<BlockID> {
        var result: Set<BlockID> = [root]
        while true { let before = result.count; blocks.filter { $0.parentID.map(result.contains) == true }.forEach { result.insert($0.id) }; if before == result.count { return result } }
    }

    private func requireBlock(_ id: BlockID, in blocks: [Block]) throws -> Block { try blocks.first { $0.id == id }.orNotFound("block") }
    private func stringIndex(_ offset: Int, in text: String) throws -> String.Index {
        guard offset >= 0, offset <= text.utf8.count else { throw invalid("text_range") }
        let utf8 = text.utf8.index(text.utf8.startIndex, offsetBy: offset)
        guard let index = String.Index(utf8, within: text) else { throw invalid("text_range") }
        return index
    }
    private func stringRange(_ range: UTF8Range, in text: String) throws -> Range<String.Index> {
        guard range.start <= range.end else { throw invalid("text_range") }
        return try stringIndex(range.start, in: text)..<stringIndex(range.end, in: text)
    }
    private func label(_ value: String) throws -> String { let value = value.trimmingCharacters(in: .whitespacesAndNewlines); guard !value.isEmpty else { throw invalid("required_value") }; return value }

    private static func seedData() -> (
        profiles: [Profile],
        workspaces: [Workspace],
        summaries: [DocumentSummary],
        documents: [DocumentID: DocumentSnapshot],
        blobs: [BlobID: Data]
    ) {
        let profile = Profile(id: ProfileID("10000000-0000-4000-8000-000000000001"), displayName: "Koru Demo", locale: "en-US")
        let workspace = Workspace(id: WorkspaceID("10000000-0000-4000-8000-000000000002"), profileID: profile.id, title: "Mobile foundation")
        let documentID = DocumentID("10000000-0000-4000-8000-000000000003")
        let referenceID = DocumentID("10000000-0000-4000-8000-000000000004")
        let heading = Block(id: BlockID(), properties: BlockProperties(headingLevel: 1), content: .paragraph(RichText(text: "A semantic Koru document", marks: [TextMark(range: UTF8Range(start: 0, end: 10), kind: .bold)])))
        let paragraph = Block(id: BlockID(), content: .paragraph(RichText(text: "English, العربية, 日本語, and emoji 👋 stay one document.")))
        let list = Block(id: BlockID(), properties: BlockProperties(listStyle: .bullet), content: .list(.bullet))
        let item = Block(id: BlockID(), parentID: list.id, content: .paragraph(RichText(text: "Blocks carry structure")))
        let nested = Block(id: BlockID(), parentID: item.id, content: .paragraph(RichText(text: "Nested by identity")))
        let task = Block(id: BlockID(), content: .task(RichText(text: "Toggle this native task")))
        let imageData = Data(base64Encoded: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=")!
        let blobID = BlobID(value: "sha256:" + SHA256.hash(data: imageData).map { String(format: "%02x", $0) }.joined())
        let blob = BlobReference(id: blobID, mimeType: "image/png", size: UInt64(imageData.count), width: 1, height: 1)
        let image = Block(id: BlockID(), properties: BlockProperties(name: "Koru sample image"), content: .image(blob))
        let inline = InlineObject(id: InlineObjectID(), anchorUTF8: 5, role: "document", reference: .document(referenceID), label: "Project brief")
        let reference = Block(id: BlockID(), content: .paragraph(RichText(text: "Open \(objectReplacement) for the referenced document", inlineObjects: [inline])))
        let first = DocumentSummary(id: documentID, workspaceID: workspace.id, title: "Mobile block model")
        let second = DocumentSummary(id: referenceID, workspaceID: workspace.id, title: "Project brief")
        return (
            profiles: [profile],
            workspaces: [workspace],
            summaries: [first, second],
            documents: [
                documentID: DocumentSnapshot(id: documentID, workspaceID: workspace.id, title: first.title, revision: 1, blocks: [heading, paragraph, list, item, nested, task, image, reference]),
                referenceID: DocumentSnapshot(id: referenceID, workspaceID: workspace.id, title: second.title, revision: 1, blocks: [Block(id: BlockID(), content: .paragraph(RichText(text: "Referenced by identity, not title.")))])
            ],
            blobs: [blobID: imageData]
        )
    }
}

private extension Optional {
    func orNotFound(_ value: String) throws -> Wrapped { guard let self else { throw notFound(value) }; return self }
}
private func invalid(_ detail: String) -> AppFailure { AppFailure(code: .invalidData, detail: detail) }
private func notFound(_ detail: String) -> AppFailure { AppFailure(code: .notFound, detail: detail) }

struct MockProfileService: ProfileService {
    let store: MockKoruStore
    func observeProfiles() async -> AsyncThrowingStream<[Profile], Error> { await store.profileStream() }
    func profile(id: ProfileID) async throws -> Profile { try await store.profile(id: id) }
    func createProfile(displayName: String, locale: String) async throws -> Profile { try await store.createProfile(displayName: displayName, locale: locale) }
    func rename(id: ProfileID, displayName: String) async throws -> Profile { try await store.renameProfile(id: id, displayName: displayName) }
}
struct MockWorkspaceService: WorkspaceService {
    let store: MockKoruStore
    func observeWorkspaces(profileID: ProfileID) async -> AsyncThrowingStream<[Workspace], Error> { await store.workspaceStream(profileID: profileID) }
    func workspace(id: WorkspaceID) async throws -> Workspace { try await store.workspace(id: id) }
    func createWorkspace(profileID: ProfileID, title: String) async throws -> Workspace { try await store.createWorkspace(profileID: profileID, title: title) }
    func rename(id: WorkspaceID, title: String) async throws -> Workspace {
        try await store.mutateWorkspace(id: id) { row in
            Workspace(id: row.id, profileID: row.profileID, title: title, archived: row.archived, deletedAtMillis: row.deletedAtMillis, modifiedAtMillis: row.modifiedAtMillis, icon: row.icon, color: row.color, leadingEmoji: row.leadingEmoji)
        }
    }
    func setArchived(id: WorkspaceID, archived: Bool) async throws -> Workspace {
        try await store.mutateWorkspace(id: id) { row in
            Workspace(id: row.id, profileID: row.profileID, title: row.title, archived: archived, deletedAtMillis: row.deletedAtMillis, modifiedAtMillis: row.modifiedAtMillis, icon: row.icon, color: row.color, leadingEmoji: row.leadingEmoji, displayTitle: row.displayTitle)
        }
    }
    func delete(id: WorkspaceID) async throws -> Workspace {
        try await store.mutateWorkspace(id: id) { row in
            Workspace(id: row.id, profileID: row.profileID, title: row.title, archived: row.archived, deletedAtMillis: Int64(Date().timeIntervalSince1970 * 1000), modifiedAtMillis: row.modifiedAtMillis, icon: row.icon, color: row.color, leadingEmoji: row.leadingEmoji, displayTitle: row.displayTitle)
        }
    }
    func restore(id: WorkspaceID) async throws -> Workspace {
        try await store.mutateWorkspace(id: id) { row in
            Workspace(id: row.id, profileID: row.profileID, title: row.title, archived: row.archived, deletedAtMillis: nil, modifiedAtMillis: row.modifiedAtMillis, icon: row.icon, color: row.color, leadingEmoji: row.leadingEmoji, displayTitle: row.displayTitle)
        }
    }
}
struct MockDocumentService: DocumentService {
    let store: MockKoruStore
    func observeDocuments(workspaceID: WorkspaceID) async -> AsyncThrowingStream<[DocumentSummary], Error> { await store.documentStream(workspaceID: workspaceID) }
    func document(id: DocumentID) async throws -> DocumentSummary { try await store.summary(id: id) }
    func createDocument(workspaceID: WorkspaceID, title: String) async throws -> DocumentSummary { try await store.createDocument(workspaceID: workspaceID, title: title) }
    func rename(id: DocumentID, title: String) async throws -> DocumentSummary { try await store.renameDocumentSummary(id: id, title: title) }
    func moveToTrash(id: DocumentID) async throws { try await store.trashDocument(id: id) }
    func openDocument(id: DocumentID) async throws -> any DocumentSession { try await store.open(id: id) }
}
struct MockBlobService: BlobService {
    let store: MockKoruStore
    func observeAvailability(id: BlobID) async -> AsyncThrowingStream<Bool, Error> { await store.blobStream(id: id) }
    func put(_ bytes: Data, mimeType: String, width: UInt32?, height: UInt32?) async throws -> BlobReference { try await store.putBlob(bytes, mimeType: mimeType, width: width, height: height) }
    func load(_ reference: BlobReference) async throws -> Data { try await store.loadBlob(id: reference.id) }
}

final class MockDocumentSession: DocumentSession, @unchecked Sendable {
    let documentID: DocumentID
    let snapshots: AsyncThrowingStream<DocumentSnapshot, Error>
    let events: AsyncThrowingStream<DocumentEvent, Error>
    private let store: MockKoruStore
    private let snapshotToken: UUID
    private let eventToken: UUID

    init(store: MockKoruStore, documentID: DocumentID, snapshotToken: UUID, eventToken: UUID, snapshots: AsyncThrowingStream<DocumentSnapshot, Error>, events: AsyncThrowingStream<DocumentEvent, Error>) {
        self.store = store; self.documentID = documentID; self.snapshotToken = snapshotToken; self.eventToken = eventToken; self.snapshots = snapshots; self.events = events
    }
    func execute(_ command: DocumentCommand, operationID: UUID) async throws { try await store.execute(id: documentID, command: command, operationID: operationID) }
    func close() async { await store.close(id: documentID, snapshotToken: snapshotToken, eventToken: eventToken) }
}

extension AppServices {
    static func mock() -> AppServices {
        let store = MockKoruStore()
        return AppServices(profiles: MockProfileService(store: store), workspaces: MockWorkspaceService(store: store), documents: MockDocumentService(store: store), blobs: MockBlobService(store: store), implementation: "mock")
    }
}

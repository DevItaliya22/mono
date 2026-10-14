// SPDX-License-Identifier: MPL-2.0

import XCTest
@testable import KoruMobile

final class MockServicesConformanceTests: XCTestCase {
    /// Android's conformance twin: a rename replaces the name everywhere the
    /// service answers, and an empty name is refused without moving anything.
    func testAProfileRenameReplacesTheNameAndRefusesEmpty() async throws {
        let store = MockKoruStore()
        let profiles = MockProfileService(store: store)
        let profile = try await profiles.createProfile(displayName: "Before", locale: "en-US")

        let renamed = try await profiles.rename(id: profile.id, displayName: "  After 🌱  ")
        XCTAssertEqual(renamed.displayName, "After 🌱")
        let fetched = try await profiles.profile(id: profile.id)
        XCTAssertEqual(fetched.displayName, "After 🌱")

        do {
            _ = try await profiles.rename(id: profile.id, displayName: "   ")
            XCTFail("an empty name was accepted")
        } catch {}
        let unchanged = try await profiles.profile(id: profile.id)
        XCTAssertEqual(unchanged.displayName, "After 🌱")
    }

    /// Android's `workspaceLifecycleActionsAreDurableInTheServiceProjection`:
    /// every action a workspace row offers is durable in what the service
    /// answers *next*, not only in the value the call returned — a rename that
    /// the projection forgets is the failure this shape catches.
    func testWorkspaceLifecycleActionsAreDurableInTheServiceProjection() async throws {
        let store = MockKoruStore()
        let workspaces = MockWorkspaceService(store: store)
        let profile = try await MockProfileService(store: store)
            .createProfile(displayName: "Workspace actions", locale: "en")
        let created = try await workspaces.createWorkspace(profileID: profile.id, title: "Original")

        let renamed = try await workspaces.rename(id: created.id, title: "Renamed")
        XCTAssertEqual(renamed.title, "Renamed")
        let archived = try await workspaces.setArchived(id: created.id, archived: true)
        XCTAssertTrue(archived.archived)
        let unarchived = try await workspaces.setArchived(id: created.id, archived: false)
        XCTAssertFalse(unarchived.archived)
        let deleted = try await workspaces.delete(id: created.id)
        XCTAssertNotNil(deleted.deletedAtMillis)
        let restored = try await workspaces.restore(id: created.id)
        XCTAssertNil(restored.deletedAtMillis)

        let listed = try await workspaces.observeWorkspaces(profileID: profile.id)
            .first(where: { !$0.isEmpty })
        XCTAssertEqual(listed?.map(\.title), ["Renamed"])
    }

    func testSemanticCommandsReplayAndExternalUpdate() async throws {
        let store = MockKoruStore()
        let profiles = MockProfileService(store: store)
        let workspaces = MockWorkspaceService(store: store)
        let documents = MockDocumentService(store: store)
        let profile = try await profiles.createProfile(displayName: "Conformance", locale: "ar-EG")
        let workspace = try await workspaces.createWorkspace(profileID: profile.id, title: "مساحة Koru")
        let summary = try await documents.createDocument(workspaceID: workspace.id, title: "Document")
        let session = try await documents.openDocument(id: summary.id)
        var iterator = session.snapshots.makeAsyncIterator()
        let initial = try await iterator.next()!
        let root = initial.blocks[0].id

        try await session.execute(.insertText(root, atUTF8: 0, text: "é مرحبا 👋"))
        try await session.execute(.deleteText(root, range: UTF8Range(start: 0, end: 2)))
        let mark = TextMark(range: UTF8Range(start: 0, end: 1), kind: .bold)
        try await session.execute(.addMark(root, mark))
        try await session.execute(.removeMark(root, mark.range, .bold))
        let inline = InlineObject(id: InlineObjectID(), anchorUTF8: 0, role: "document", reference: .document(summary.id), label: "Document")
        try await session.execute(.insertInlineObject(root, inline))
        try await session.execute(.removeInlineObject(root, inline.id))

        let task = Block(id: BlockID(), content: .task(RichText(text: "Task")))
        try await session.execute(.createBlock(task, siblingIndex: 1))
        try await session.execute(.moveBlock(task.id, parentID: root, siblingIndex: 0))
        try await session.execute(.toggleTask(task.id, true))
        let blob = try await MockBlobService(store: store).put(Data([1, 2, 3]), mimeType: "image/png", width: 1, height: 1)
        let image = Block(id: BlockID(), content: .image(blob))
        try await session.execute(.createBlock(image, siblingIndex: 1))
        try await session.execute(.attachBlob(image.id, blob))
        try await session.execute(.renameDocument("Renamed"))

        let operation = UUID()
        try await session.execute(.renameDocument("Replay"), operationID: operation)
        try await session.execute(.renameDocument("Replay"), operationID: operation)
        await XCTAssertThrowsErrorAsync { try await session.execute(.renameDocument("Conflict"), operationID: operation) }

        try await store.simulateExternal(id: summary.id, command: .renameDocument("External"))
        var latest = try await iterator.next()!
        while latest.title != "External", let next = try await iterator.next() { latest = next }
        XCTAssertEqual(latest.title, "External")
        try await session.execute(.deleteBlock(task.id))
        await session.close()
    }

    func testMalformedOffsetsAndHierarchyAreRejected() async throws {
        let store = MockKoruStore()
        let profileStream = await store.profileStream()
        let profile = try await profileStream.first(where: { !$0.isEmpty })!.first!
        let workspaceStream = await store.workspaceStream(profileID: profile.id)
        let workspace = try await workspaceStream.first(where: { !$0.isEmpty })!.first!
        let documentStream = await store.documentStream(workspaceID: workspace.id)
        let summary = try await documentStream.first(where: { !$0.isEmpty })!.first!
        let session = try await store.open(id: summary.id)
        var iterator = session.snapshots.makeAsyncIterator()
        let root = try await iterator.next()!.blocks[0].id
        try await session.execute(.insertText(root, atUTF8: 0, text: "é"))
        await XCTAssertThrowsErrorAsync { try await session.execute(.insertText(root, atUTF8: 1, text: "bad")) }
        await XCTAssertThrowsErrorAsync {
            try await session.execute(.createBlock(Block(id: BlockID(), parentID: BlockID(), content: .paragraph(RichText(text: "orphan"))), siblingIndex: 0))
        }
        await XCTAssertThrowsErrorAsync { try await session.execute(.moveBlock(root, parentID: root, siblingIndex: 0)) }
        await session.close()
    }
}

private func XCTAssertThrowsErrorAsync(_ expression: () async throws -> Void) async {
    do { try await expression(); XCTFail("Expected error") } catch {}
}

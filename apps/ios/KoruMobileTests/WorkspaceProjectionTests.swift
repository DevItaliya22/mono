// SPDX-License-Identifier: MPL-2.0

import XCTest

@testable import KoruMobile

/// The twin of Android's `WorkspaceFilterTest` and `DocumentProjectionTest`.
///
/// Both phones project one manifest into the same two lists, so these are
/// product rules rather than facts about Compose or SwiftUI: a workspace's
/// lifecycle decides whether it is listed and where it sorts, and a row is a
/// file because the manifest says `kind == "file"` and never because of what
/// it is called. Android pins them as pure functions over `Workspace`; the
/// iOS implementations live on `AppModel`, so this drives the model over the
/// mock services — no embedded store, no endpoint, nothing on disk.
///
/// One row of Android's table is still missing here: a workspace that is
/// **both** archived and deleted. The platforms agree today — deleted wins on
/// both, since the visibleWorkspaces cascade — but the twin that pins the
/// agreement was never added (TECHDEBT 86).
@MainActor
final class WorkspaceProjectionTests: XCTestCase {
    /// A workspace to arm, in the shape Android's fixture builds one.
    private struct Row {
        var title: String
        var archived = false
        var deletedAt: Int64?
        var modified: Int64 = 0
    }

    func testActiveIsTheDefaultAndTheLifecycleFiltersAreIndependent() async throws {
        // Titles are multilingual on purpose: the rule is a lifecycle rule and
        // must not acquire an opinion about the script a name is written in.
        let armed = try await arm([
            Row(title: "実装", modified: 10),
            Row(title: "Archivado", archived: true, modified: 40),
            Row(title: "محذوف", deletedAt: 50, modified: 50),
        ])
        let (model, ids) = (armed.model, armed.ids)

        XCTAssertEqual(listed(model), [ids[0]], "only the live workspace by default")

        model.showArchivedWorkspaces = true
        XCTAssertEqual(Set(listed(model)), Set([ids[0], ids[1]]), "archived joins, deleted does not")

        model.showArchivedWorkspaces = false
        model.showDeletedWorkspaces = true
        XCTAssertEqual(Set(listed(model)), Set([ids[0], ids[2]]), "the two filters are independent")
    }

    /// Android's `archived_and_deleted_workspaces_stay_below_active_workspaces`:
    /// the lifecycle rank outranks recency, even where the deleted row is the
    /// most recently modified of the three.
    func testArchivedAndDeletedWorkspacesStayBelowActiveOnes() async throws {
        let armed = try await arm([
            Row(title: "Deleted", deletedAt: 50, modified: 50),
            Row(title: "Archived", archived: true, modified: 40),
            Row(title: "Active", modified: 10),
        ])
        armed.model.showArchivedWorkspaces = true
        armed.model.showDeletedWorkspaces = true

        XCTAssertEqual(listed(armed.model), [armed.ids[2], armed.ids[1], armed.ids[0]])
    }

    /// Android's `workspaces_sort_by_newest_modification_by_default`.
    func testWorkspacesSortByNewestModificationWithinTheirLifecycle() async throws {
        let armed = try await arm([
            Row(title: "Middle", modified: 20),
            Row(title: "Older", modified: 10),
            Row(title: "Newest", modified: 30),
        ])

        XCTAssertEqual(
            armed.model.visibleWorkspaces.map(\.title), ["Newest", "Middle", "Older"])
    }

    /// Android's `documents_and_files_use_manifest_kind_not_filename`, over
    /// both halves of the same rule on this side: `canOpen`, which decides
    /// whether a row opens, and `visibleDocuments`, which decides which tab
    /// lists it.
    func testDocumentsAndFilesUseTheManifestKindNotTheFilename() async throws {
        let armed = try await arm([Row(title: "Files")])
        let (store, model, workspace) = (armed.store, armed.model, armed.ids[0])

        let named = DocumentSummary(
            id: DocumentID(), workspaceID: workspace, title: "Looks like a file.pdf",
            kind: "document")
        let file = DocumentSummary(
            id: DocumentID(), workspaceID: workspace, title: "No extension", kind: "file")
        XCTAssertTrue(model.canOpen(named), "an extension in the title is not a kind")
        XCTAssertFalse(model.canOpen(file), "a file row lists and does not open")

        // The listing keys on the same field. The mock store mints only
        // `kind == "document"` rows — it has no seam for a shared file, which
        // is why the `file` half above is asserted against `canOpen` — so
        // Documents holds this row and Files is empty.
        _ = try await MockDocumentService(store: store)
            .createDocument(workspaceID: workspace, title: "Looks like a file.pdf")
        model.selectWorkspace(workspace)
        try await settle { model.documents.count == 1 }

        model.documentProjection = .documents
        XCTAssertEqual(model.visibleDocuments.map(\.title), ["Looks like a file.pdf"])
        model.documentProjection = .files
        XCTAssertTrue(model.visibleDocuments.isEmpty)
    }

    // MARK: - arming

    private func arm(_ rows: [Row]) async throws
        -> (store: MockKoruStore, model: AppModel, ids: [WorkspaceID])
    {
        let store = MockKoruStore()
        let workspaces = MockWorkspaceService(store: store)
        let model = AppModel(
            services: AppServices(
                profiles: MockProfileService(store: store),
                workspaces: workspaces,
                documents: MockDocumentService(store: store),
                blobs: MockBlobService(store: store),
                implementation: "mock"))

        let profile = try await MockProfileService(store: store)
            .createProfile(displayName: "لوحة Koru", locale: "ar-EG")
        var ids: [WorkspaceID] = []
        for row in rows {
            let created = try await workspaces.createWorkspace(
                profileID: profile.id, title: row.title)
            // The lifecycle fields are set through the store rather than the
            // service because only `delete` writes a timestamp, and it writes
            // the wall clock — which cannot be ordered against a fixture.
            try await setLifecycle(
                store, created.id,
                archived: row.archived, deletedAt: row.deletedAt, modified: row.modified)
            ids.append(created.id)
        }

        // Armed before the observer exists, so the stream's opening value is
        // already the finished fixture and no update can arrive out of order.
        model.selectProfile(profile.id)
        try await settle { model.workspaces.count == rows.count }
        return (store, model, ids)
    }

    private func listed(_ model: AppModel) -> [WorkspaceID] {
        model.visibleWorkspaces.map(\.id)
    }

    private func settle(
        _ file: StaticString = #filePath, line: UInt = #line, until: () -> Bool
    ) async throws {
        for _ in 0..<400 {
            if until() { return }
            try await Task.sleep(nanoseconds: 5_000_000)
        }
        XCTFail("the model never settled", file: file, line: line)
    }
}

/// Deliberately a free function rather than a method on the `@MainActor` test
/// case: `mutateWorkspace` takes a plain `(Workspace) -> Workspace`, and under
/// Swift 6's complete checking a closure formed on the main actor cannot be
/// *sent* into the store's actor — "sending value of non-Sendable type risks
/// causing data races", which is a compile error and not a warning. Nothing
/// about the closure is unsafe; it is where it was formed. `MockServices.swift`
/// passes the same shape from its nonisolated service structs, which is what
/// this mirrors.
private func setLifecycle(
    _ store: MockKoruStore, _ id: WorkspaceID,
    archived: Bool, deletedAt: Int64?, modified: Int64
) async throws {
    _ = try await store.mutateWorkspace(id: id) { existing in
        Workspace(
            id: existing.id, profileID: existing.profileID, title: existing.title,
            archived: archived, deletedAtMillis: deletedAt, modifiedAtMillis: modified)
    }
}

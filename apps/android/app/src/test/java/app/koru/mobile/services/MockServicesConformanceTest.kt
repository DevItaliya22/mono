// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import app.koru.mobile.domain.*
import app.koru.mobile.logging.AppLogger
import java.util.UUID
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test

class MockServicesConformanceTest {
    private val logger = AppLogger(sink = { _, _ -> })

    @Test fun textSplitPreservesFormattingAndReplaysWithoutDuplicateParagraphs() = runTest {
        val store = MockKoruStore(logger)
        val profile = MockProfileService(store).create("Split", "en")
        val workspace = MockWorkspaceService(store).create(profile.id, "Test")
        val documents = MockDocumentService(store)
        val session = documents.open(documents.create(workspace.id, "Test").id)
        val block = session.snapshots.first().blocks.single().id
        session.execute(DocumentCommand.EditBlockText(block, "", "😀Bold"))
        session.execute(DocumentCommand.AddMark(block, TextMark(TextRange(4, 8), MarkKind.Bold)))
        val tail = BlockId.new()
        val split = DocumentCommand.EditBlockText(block, "😀Bold", "😀Bold", split = BlockTextSplit(tail, 4))
        session.execute(split)
        session.execute(split)
        val result = session.snapshots.first().blocks
        assertEquals(listOf(block, tail), result.map { it.id })
        assertEquals("😀", result[0].content.editableText()!!.text)
        assertEquals("Bold", result[1].content.editableText()!!.text)
        assertEquals(TextRange(0, 4), result[1].content.editableText()!!.marks.single().range)
        session.close()
    }

    @Test fun semanticCommandScenarioAndExternalUpdates() = runTest {
        val store = MockKoruStore(logger)
        val profiles = MockProfileService(store)
        val workspaces = MockWorkspaceService(store)
        val documents = MockDocumentService(store)
        val blobs = MockBlobService(store)

        val profile = profiles.create("Conformance", "ar-EG")
        val workspace = workspaces.create(profile.id, "مساحة Koru")
        val summary = documents.create(workspace.id, "Document")
        val session = documents.open(summary.id)
        val initial = session.snapshots.first()
        val root = initial.blocks.single().id

        session.execute(DocumentCommand.InsertText(root, 0, "é مرحبا 👋"))
        val textBytes = "é مرحبا 👋".encodeToByteArray().size
        session.execute(DocumentCommand.DeleteText(root, TextRange(0, 2)))
        session.execute(DocumentCommand.AddMark(root, TextMark(TextRange(0, textBytes - 2), MarkKind.Bold)))
        session.execute(DocumentCommand.RemoveMark(root, TextRange(0, 1), MarkKind.Bold))
        val inline = InlineObject(InlineObjectId.new(), 0, "document", EntityReference.Document(summary.id), "Document")
        session.execute(DocumentCommand.InsertInlineObject(root, inline))
        session.execute(DocumentCommand.RemoveInlineObject(root, inline.id))

        val second = Block(BlockId.new(), content = BlockContent.Task(RichText("Task")))
        session.execute(DocumentCommand.CreateBlock(second, 1))
        session.execute(DocumentCommand.MoveBlock(second.id, root, 0))
        session.execute(DocumentCommand.ToggleTask(second.id, true))

        val blob = blobs.put(byteArrayOf(1, 2, 3), "image/png", 1, 1)
        val image = Block(BlockId.new(), content = BlockContent.Image(blob))
        session.execute(DocumentCommand.CreateBlock(image, 1))
        session.execute(DocumentCommand.AttachBlob(image.id, blob))
        session.execute(DocumentCommand.RenameDocument("Renamed"))

        val operation = UUID.randomUUID().toString()
        val replay = DocumentCommand.RenameDocument("Replayed once")
        session.execute(replay, operation)
        val replayRevision = session.snapshots.first().revision
        session.execute(replay, operation)
        assertEquals(replayRevision, session.snapshots.first().revision)
        assertFails<AppFailure> { session.execute(DocumentCommand.RenameDocument("Conflict"), operation) }

        store.simulateExternal(summary.id, DocumentCommand.RenameDocument("External"))
        val final = session.snapshots.first()
        assertEquals("External", final.title)
        assertTrue((final.blocks.first { it.id == second.id }.content as BlockContent.Task).richText.text == "Task")
        assertTrue(final.blocks.first { it.id == second.id }.properties.done)

        session.execute(DocumentCommand.DeleteBlock(second.id))
        assertFalse(session.snapshots.first().blocks.any { it.id == second.id })
        session.close()
    }

    @Test fun malformedOffsetsAndHierarchyAreRejected() = runTest {
        val store = MockKoruStore(logger)
        val summary = store.observeDocuments(WorkspaceId("10000000-0000-4000-8000-000000000002")).first().first()
        val session = store.open(summary.id)
        val root = session.snapshots.first().blocks.first().id

        session.execute(DocumentCommand.InsertText(root, 0, "é"))
        assertFails<AppFailure> { session.execute(DocumentCommand.InsertText(root, 1, "bad")) }
        assertFails<AppFailure> {
            session.execute(DocumentCommand.CreateBlock(Block(BlockId.new(), BlockId.new(), content = BlockContent.Paragraph(RichText("orphan"))), 0))
        }
        assertFails<AppFailure> { session.execute(DocumentCommand.MoveBlock(root, root, 0)) }
    }

    @Test fun profileRenameIsDurableInTheServiceProjection() = runTest {
        val store = MockKoruStore(logger)
        val service = MockProfileService(store)
        val profile = service.create("Original", "en")

        assertEquals("Renamed", service.rename(profile.id, " Renamed ").displayName)
        assertEquals("Renamed", service.get(profile.id).displayLabel)
        assertFails<AppFailure> { service.rename(profile.id, "   ") }
        assertEquals(
            "Renamed",
            service.observeProfiles().first().first { it.id == profile.id }.displayName,
        )
    }

    @Test fun workspaceLifecycleActionsAreDurableInTheServiceProjection() = runTest {
        val store = MockKoruStore(logger)
        val service = MockWorkspaceService(store)
        val profile = MockProfileService(store).create("Workspace actions", "en")
        val created = service.create(profile.id, "Original")

        assertEquals("Renamed", service.rename(created.id, "Renamed").title)
        assertTrue(service.setArchived(created.id, true).archived)
        assertFalse(service.setArchived(created.id, false).archived)
        assertNotNull(service.delete(created.id).deletedAtMillis)
        assertNull(service.restore(created.id).deletedAtMillis)
        assertEquals("Renamed", service.observeWorkspaces(profile.id).first().single().title)
    }

    private suspend inline fun <reified T : Throwable> assertFails(crossinline block: suspend () -> Unit) {
        try {
            block()
            fail("Expected ${T::class.simpleName}")
        } catch (error: Throwable) {
            if (error !is T) throw error
        }
    }
}

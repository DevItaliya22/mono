// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.ui

import app.koru.mobile.domain.ProfileId
import app.koru.mobile.domain.Workspace
import app.koru.mobile.domain.WorkspaceId
import org.junit.Assert.assertFalse
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class WorkspaceFilterTest {
    private val profile = ProfileId("10000000-0000-4000-8000-000000000001")

    @Test
    fun active_is_the_default_and_lifecycle_filters_are_independent() {
        val active = workspace("active")
        val archived = workspace("archived", archived = true)
        val deleted = workspace("deleted", archived = true, deletedAtMillis = 42)

        assertTrue(workspaceVisible(active, showArchived = false, showDeleted = false))
        assertFalse(workspaceVisible(archived, showArchived = false, showDeleted = false))
        assertFalse(workspaceVisible(deleted, showArchived = false, showDeleted = false))

        assertTrue(workspaceVisible(archived, showArchived = true, showDeleted = false))
        assertFalse(workspaceVisible(deleted, showArchived = true, showDeleted = false))
        assertTrue(workspaceVisible(deleted, showArchived = false, showDeleted = true))
    }

    @Test
    fun workspaces_sort_by_newest_modification_by_default() {
        val rows = listOf(
            workspace("Middle", modifiedAtMillis = 20),
            workspace("Older", modifiedAtMillis = 10),
            workspace("Newest", modifiedAtMillis = 30),
        )

        assertEquals(
            listOf("Newest", "Middle", "Older"),
            sortWorkspaces(rows, WorkspaceSort.LastModified).map { it.title },
        )
    }

    @Test
    fun workspaces_sort_by_title_in_both_directions() {
        val rows = listOf(workspace("beta"), workspace("Alpha"), workspace("charlie"))

        assertEquals(
            listOf("Alpha", "beta", "charlie"),
            sortWorkspaces(rows, WorkspaceSort.TitleAscending).map { it.title },
        )
        assertEquals(
            listOf("charlie", "beta", "Alpha"),
            sortWorkspaces(rows, WorkspaceSort.TitleDescending).map { it.title },
        )
    }

    @Test
    fun archived_and_deleted_workspaces_stay_below_active_workspaces() {
        val rows = listOf(
            workspace("Deleted", archived = true, deletedAtMillis = 50, modifiedAtMillis = 50),
            workspace("Archived", archived = true, modifiedAtMillis = 40),
            workspace("Active", modifiedAtMillis = 10),
        )

        WorkspaceSort.entries.forEach { sort ->
            assertEquals(
                listOf("Active", "Archived", "Deleted"),
                sortWorkspaces(rows, sort).map { it.title },
            )
        }
    }

    @Test
    fun workspace_actions_follow_lifecycle_state() {
        assertEquals(
            listOf(WorkspaceAction.Rename, WorkspaceAction.Archive, WorkspaceAction.Delete, WorkspaceAction.DebugInfo),
            workspaceActions(workspace("Active")),
        )
        assertEquals(
            listOf(WorkspaceAction.Rename, WorkspaceAction.Unarchive, WorkspaceAction.Delete, WorkspaceAction.DebugInfo),
            workspaceActions(workspace("Archived", archived = true)),
        )
        assertEquals(
            listOf(WorkspaceAction.Restore, WorkspaceAction.DebugInfo),
            workspaceActions(workspace("Deleted", deletedAtMillis = 42)),
        )
    }

    private fun workspace(
        name: String,
        archived: Boolean = false,
        deletedAtMillis: Long? = null,
        modifiedAtMillis: Long = 0,
    ) = Workspace(WorkspaceId(name), profile, name, archived, deletedAtMillis, modifiedAtMillis)
}

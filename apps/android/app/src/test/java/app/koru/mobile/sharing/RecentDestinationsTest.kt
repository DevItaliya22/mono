// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.sharing

import app.koru.mobile.domain.ProfileId
import app.koru.mobile.domain.WorkspaceId
import org.junit.Assert.assertEquals
import org.junit.Test

class RecentDestinationsTest {
    private val profile = ProfileId("10000000-0000-4000-8000-000000000001")
    private val other = ProfileId("10000000-0000-4000-8000-000000000002")

    @Test
    fun the_newest_destination_leads() {
        val entries = recordedRecentDestination(
            recordedRecentDestination(emptyList(), entry("w1")),
            entry("w2"),
        )

        assertEquals(listOf("w2", "w1"), entries.map { it.workspaceId.value })
    }

    @Test
    fun recording_an_existing_pair_moves_it_to_the_front() {
        var entries = emptyList<RecentDestination>()
        entries = recordedRecentDestination(entries, entry("w1"))
        entries = recordedRecentDestination(entries, entry("w2"))
        entries = recordedRecentDestination(entries, entry("w1"))

        assertEquals(listOf("w1", "w2"), entries.map { it.workspaceId.value })
    }

    @Test
    fun the_list_is_capped_at_three() {
        var entries = emptyList<RecentDestination>()
        listOf("w1", "w2", "w3", "w4").forEach { id ->
            entries = recordedRecentDestination(entries, entry(id))
        }

        assertEquals(listOf("w4", "w3", "w2"), entries.map { it.workspaceId.value })
    }

    @Test
    fun the_same_workspace_under_another_profile_is_a_separate_entry() {
        var entries = emptyList<RecentDestination>()
        entries = recordedRecentDestination(entries, entry("w1"))
        entries = recordedRecentDestination(entries, entry("w1", profile = other))

        assertEquals(2, entries.size)
        assertEquals(other, entries.first().profileId)
    }

    @Test
    fun encoding_round_trips_and_garbage_reads_as_empty() {
        val entries = listOf(entry("w1"), entry("w2", profile = other))

        assertEquals(entries, decodeRecentDestinations(encodeRecentDestinations(entries)))
        assertEquals(emptyList<RecentDestination>(), decodeRecentDestinations(null))
        assertEquals(emptyList<RecentDestination>(), decodeRecentDestinations(""))
        assertEquals(emptyList<RecentDestination>(), decodeRecentDestinations("{not a list"))
    }

    private fun entry(
        workspace: String,
        profile: ProfileId = this.profile,
    ) = RecentDestination(profile, WorkspaceId(workspace))
}

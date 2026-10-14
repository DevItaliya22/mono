// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import app.koru.mobile.domain.ProfileId
import org.junit.Assert.*
import org.junit.Test

class ProfileSelectionDraftTest {
    private val a = ProfileId("10000000-0000-4000-8000-000000000001")
    private val b = ProfileId("20000000-0000-4000-8000-000000000002")
    private val c = ProfileId("30000000-0000-4000-8000-000000000003")

    @Test fun oneReplacesAndSeveralTogglesWithoutAvailabilityPolicy() {
        val available = setOf(a, b, c)
        val one = ProfileSelectionDraft().toggle(a, available).toggle(b, available)
        assertEquals(setOf(b), one.ids)
        val several = one.choose(ProfileSelectionMode.Several, available).toggle(a, available).toggle(c, available).toggle(b, available)
        assertEquals(listOf(a, c), several.submission(available))
        assertEquals(ProfileSelectionMode.Several, several.mode)
    }

    @Test fun allIsASnapshotAndNewGrantsRequireAnotherExplicitChoice() {
        val all = ProfileSelectionDraft().choose(ProfileSelectionMode.All, setOf(a, b))
        assertEquals(listOf(a, b), all.submission(setOf(a, b)))
        val refreshed = all.reconcile(setOf(a, b, c))
        assertEquals(setOf(a, b), refreshed.ids)
        assertEquals(ProfileSelectionMode.Several, refreshed.mode)
        assertEquals(setOf(a, b, c), refreshed.choose(ProfileSelectionMode.All, setOf(a, b, c)).ids)
    }

    @Test fun revokedGrantIsRemovedAndUnrelatedProgressDoesNotChangeDraft() {
        val selected = ProfileSelectionDraft().choose(ProfileSelectionMode.All, setOf(a, b))
        assertEquals(selected, selected.reconcile(setOf(a, b)))
        assertEquals(setOf(b), selected.reconcile(setOf(b)).ids)
        assertEquals(listOf(b), selected.submission(setOf(b)))
        assertEquals(selected, selected.toggle(c, setOf(a, b)))
    }
}

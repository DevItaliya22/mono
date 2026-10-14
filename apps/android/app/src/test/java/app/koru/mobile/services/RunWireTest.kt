// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import app.koru.contract.api.v1.RunEconomics
import app.koru.contract.api.v1.RunResult
import app.koru.contract.api.v1.RunTimelineRow
import app.koru.contract.api.v1.RunTimelineWindow
import app.koru.contract.api.v1.RunView
import app.koru.mobile.domain.*
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The generated run DTOs decoded field by field — where an `optional` that
 * is absent must stay null and never become an empty string or a zero.
 */
class RunWireTest {
    @Test
    fun every_field_the_run_screen_shows_survives_the_wire() {
        val view = RunView.newBuilder()
            .setDocumentId("10000000-0000-4000-8000-000000000001")
            .setWorkspaceId("10000000-0000-4000-8000-000000000002")
            .setRunId("20000000-0000-4000-8000-000000000001")
            .setRevision(9)
            .setStatus("running")
            .addAttention("approval_required")
            .setControl("human")
            .setAgent("codex")
            .setObjective("Tidy the notes")
            .setScopeKind("temporary")
            .setContextCount(2)
            .setExecutingDeviceId("30000000-0000-4000-8000-000000000001")
            .setExecutingDeviceName("Desktop")
            .setExecutingHere(false)
            .setCreatedAtMillis(1_770_000_000_000)
            .addResults(
                RunResult.newBuilder()
                    .setArtifactId("40000000-0000-4000-8000-000000000001")
                    .setHeldByDeviceId("30000000-0000-4000-8000-000000000001")
                    .setHeldByDeviceName("Desktop")
                    .setHeldHere(false)
                    .setTitle("notes.md")
                    .setSizeBytes(2_048)
                    .setRegisteredAtMillis(1_770_000_000_500),
            )
            .setEconomics(
                RunEconomics.newBuilder()
                    .setOrchestratorTurns(2)
                    .setTurnsWithUsage(2)
                    .setInputTokens(16)
                    .setCachedInputUnavailableTurns(2)
                    .setToolCalls(21)
                    .setModels(1)
                    .setComplete(true),
            )
            .setRating("good")
            .setRatingNote("good enough")
            .setRatedAtMillis(9_000)
            .setTimelineLen(12)
            .build()

        val snapshot = view.toSnapshot()

        assertEquals(RunState.Running, snapshot.state)
        assertEquals(listOf("approval_required"), snapshot.attention)
        assertEquals("human", snapshot.control)
        assertNull(snapshot.failureStage)
        assertNull(snapshot.priorAttempt)
        assertEquals("Desktop", snapshot.executingDeviceName)
        assertFalse(snapshot.executingHere)
        val result = snapshot.results.single()
        assertEquals("notes.md", result.title)
        assertEquals("Desktop", result.heldByDeviceName)
        assertEquals(2_048L, result.sizeBytes)
        // Unavailable stays unavailable, never zero.
        assertEquals(16L, snapshot.economics.inputTokens)
        assertNull(snapshot.economics.outputTokens)
        assertNull(snapshot.economics.cachedInputTokens)
        assertEquals(2, snapshot.economics.cachedInputUnavailableTurns)
        assertNull(snapshot.economics.elapsedMs)
        assertEquals(RunRating.Good, snapshot.rating)
        assertEquals("good enough", snapshot.ratingNote)
        assertEquals(9_000L, snapshot.ratedAtMillis)
        assertEquals(12, snapshot.timelineLen)
        assertFalse(snapshot.needsNewerKoru)
    }

    @Test
    fun an_absent_optional_is_null_and_an_unknown_status_is_kept_as_its_token() {
        val snapshot = RunView.newBuilder()
            .setDocumentId("10000000-0000-4000-8000-000000000001")
            .setWorkspaceId("10000000-0000-4000-8000-000000000002")
            .setRunId("20000000-0000-4000-8000-000000000001")
            .setStatus("something_new")
            .setNeedsNewerKoru(true)
            .build()
            .toSnapshot()
        assertEquals(RunState.Unknown, snapshot.state)
        assertEquals("something_new", snapshot.stateToken)
        assertNull(snapshot.executingDeviceName)
        assertNull(snapshot.rating)
        assertNull(snapshot.ratingNote)
        assertTrue(snapshot.results.isEmpty())
        assertTrue(snapshot.needsNewerKoru)
    }

    @Test
    fun an_error_code_is_raised_as_the_native_codes_are() {
        val error = runCatching { RunView.newBuilder().setErrorCode("document_is_not_a_run").build().toSnapshot() }
            .exceptionOrNull()
        assertEquals("document_is_not_a_run", error?.message)
    }

    @Test
    fun a_window_keeps_steps_usage_and_gaps_in_their_places() {
        val page = RunTimelineWindow.newBuilder()
            .setDocumentId("10000000-0000-4000-8000-000000000001")
            .setNewestFirstFrom(50)
            .setTimelineLen(1_000)
            .setRevision(9)
            .addRows(RunTimelineRow.newBuilder().setKind("step").setId("a").setAtMillis(3).setStep("tools_completed").setCalls(4).setFailed(1).setRefs(1))
            .addRows(RunTimelineRow.newBuilder().setKind("usage").setId("b").setInputTokens(120).setRevision(0).setCompleteness("complete"))
            .addRows(RunTimelineRow.newBuilder().setKind("gap"))
            .addRows(RunTimelineRow.newBuilder().setKind("step").setId("c").setAtMillis(1).setStep("turn_cancelled").setOriginator("human").setReason("takeover_boundary"))
            .build()
            .toPage()

        assertEquals(50, page.newestFirstFrom)
        assertEquals(1_000, page.timelineLen)
        assertEquals(9L, page.revision)
        val tools = page.entries[0] as RunTimelineEntry.Step
        assertEquals("tools_completed", tools.step)
        assertEquals(4, tools.calls)
        assertEquals(1, tools.failed)
        assertNull(tools.originator)
        val usage = page.entries[1] as RunTimelineEntry.Usage
        assertEquals(120L, usage.inputTokens)
        assertNull(usage.outputTokens)
        assertEquals("complete", usage.completeness)
        assertEquals(RunTimelineEntry.Gap, page.entries[2])
        val cancelled = page.entries[3] as RunTimelineEntry.Step
        assertEquals("human", cancelled.originator)
        assertEquals("takeover_boundary", cancelled.reason)
    }
}

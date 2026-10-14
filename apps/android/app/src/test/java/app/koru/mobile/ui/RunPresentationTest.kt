// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.ui

import app.koru.mobile.domain.*
import app.koru.mobile.ui.run.*
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The run screen's decisions, state by state, without a device: what the
 * pill says, which glyph leads, when the executing device is named (D16),
 * how a result on another device reads (D27), and that a rating is shown
 * and never asked for (D15, D44).
 */
class RunPresentationTest {
    private val document = DocumentId("10000000-0000-4000-8000-000000000001")
    private val workspace = WorkspaceId("10000000-0000-4000-8000-000000000002")

    private fun run(
        state: RunState = RunState.Running,
        attention: List<String> = emptyList(),
        executingHere: Boolean = false,
        executingDeviceName: String? = "Desktop",
        objective: String = "Tidy the release notes\nand keep the headings",
        results: List<RunResultSummary> = emptyList(),
        rating: RunRating? = null,
        ratingNote: String? = null,
        failureStage: String? = null,
        priorAttempt: String? = null,
        control: String? = null,
        needsNewerKoru: Boolean = false,
    ) = RunSnapshot(
        documentId = document,
        workspaceId = workspace,
        runId = "20000000-0000-4000-8000-000000000001",
        revision = 7,
        state = state,
        stateToken = state.wireName,
        attention = attention,
        control = control,
        failureStage = failureStage,
        priorAttempt = priorAttempt,
        agent = "codex",
        objective = objective,
        scopeKind = "temporary",
        contextCount = 2,
        executingDeviceId = "30000000-0000-4000-8000-000000000001",
        executingDeviceName = executingDeviceName,
        executingHere = executingHere,
        createdAtMillis = 1_770_000_000_000,
        results = results,
        rating = rating,
        ratingNote = ratingNote,
        ratedAtMillis = rating?.let { 1_770_000_100_000 },
        timelineLen = 12,
        needsNewerKoru = needsNewerKoru,
    )

    private fun result(heldHere: Boolean, name: String?) = RunResultSummary(
        artifactId = "40000000-0000-4000-8000-000000000001",
        heldByDeviceId = "30000000-0000-4000-8000-000000000001",
        heldByDeviceName = name,
        heldHere = heldHere,
        title = "notes.md",
        sizeBytes = 2_048,
        registeredAtMillis = 1_770_000_000_500,
    )

    @Test
    fun an_empty_screen_is_loading_and_a_snapshot_is_ready() {
        assertEquals(RunScreenState.Loading, runScreenState(null))
        val ready = runScreenState(run()) as RunScreenState.Ready
        assertEquals(RunGlyph.Running, ready.overview.glyph)
        assertFalse(ready.overview.needsAttention)
    }

    @Test
    fun the_pill_leads_with_the_glyph_names_the_other_device_and_takes_the_first_line() {
        assertEquals("● Desktop · Tidy the release notes", runPillLabel(run(), "Stored", "Run", "another device"))
        // D16: this device is not named.
        assertEquals("● Tidy the release notes", runPillLabel(run(executingHere = true), null, "Run", "another device"))
        // A device this phone has never met is still named as another device.
        assertEquals("● another device · Tidy the release notes", runPillLabel(run(executingDeviceName = null), null, "Run", "another device"))
        // A run that has not reached this phone shows its stored label, then the fallback.
        assertEquals("Stored", runPillLabel(null, "Stored", "Run", "another device"))
        assertEquals("Run", runPillLabel(null, " ", "Run", "another device"))
    }

    @Test
    fun the_first_line_is_bounded_and_ellipsized() {
        assertEquals("", firstLine("  \n\n"))
        assertEquals("second", firstLine("\n  second \nthird"))
        val long = "x".repeat(80)
        assertEquals(60, firstLine(long).length)
        assertTrue(firstLine(long).endsWith("…"))
    }

    @Test
    fun the_bound_counts_characters_a_person_sees_and_never_splits_one() {
        // A surrogate pair is one character: 80 of them are 160 code units.
        val emoji = "\uD83D\uDE42".repeat(80)
        val cut = firstLine(emoji)
        assertTrue(cut.endsWith("…"))
        assertEquals(59, cut.dropLast(1).codePointCount(0, cut.length - 1))
        assertFalse(cut.any { it.isSurrogate() && cut.indexOf(it) == cut.length - 2 })
        // A combining mark stays with its base: 80 × "e\u0301" is 80 characters.
        val accented = "e\u0301".repeat(80)
        assertEquals("e\u0301".repeat(59) + "…", firstLine(accented))
        // Exactly the bound, in graphemes, is not cut.
        assertEquals("\uD83D\uDE42".repeat(60), firstLine("\uD83D\uDE42".repeat(60)))
    }

    @Test
    fun every_state_has_its_glyph_and_attention_wins_while_running() {
        assertEquals(RunGlyph.Waiting, runGlyph(run(RunState.Created)))
        assertEquals(RunGlyph.Waiting, runGlyph(run(RunState.Starting)))
        assertEquals(RunGlyph.Running, runGlyph(run(RunState.Running)))
        assertEquals(RunGlyph.Attention, runGlyph(run(RunState.Running, attention = listOf("approval_required"))))
        assertEquals(RunGlyph.Interrupted, runGlyph(run(RunState.Interrupted)))
        assertEquals(RunGlyph.Completed, runGlyph(run(RunState.Completed)))
        assertEquals(RunGlyph.Failed, runGlyph(run(RunState.Failed)))
        assertEquals(RunGlyph.Cancelled, runGlyph(run(RunState.Cancelled)))
        assertEquals(RunGlyph.Attention, runGlyph(run(RunState.AwaitingReview)))
        assertEquals(RunGlyph.Unknown, runGlyph(run(RunState.Unknown)))
    }

    @Test
    fun an_interrupted_run_says_whether_an_earlier_attempt_may_exist() {
        val unknown = runOverview(run(RunState.Interrupted, priorAttempt = "unknown"))
        assertTrue(unknown.priorAttemptUnknown)
        val ended = runOverview(run(RunState.Interrupted, priorAttempt = "ended"))
        assertFalse(ended.priorAttemptUnknown)
        // The stage only qualifies a failure.
        assertEquals("provider_start", runOverview(run(RunState.Failed, failureStage = "provider_start")).failureStage)
        assertNull(runOverview(run(RunState.Running, failureStage = "provider_start")).failureStage)
    }

    @Test
    fun a_completed_run_names_its_result_with_the_device_that_holds_it() {
        val elsewhere = runOverview(run(RunState.Completed, results = listOf(result(heldHere = false, name = "Desktop"))))
        assertEquals(RunDevice.Named("Desktop"), elsewhere.results.single().heldBy)
        assertEquals(RunDevice.Named("Desktop"), elsewhere.executing)
        val here = runOverview(run(RunState.Completed, executingHere = true, results = listOf(result(heldHere = true, name = "Pixel"))))
        assertEquals(RunDevice.Here, here.results.single().heldBy)
        assertEquals(RunDevice.Here, here.executing)
        val unmet = runOverview(run(RunState.Completed, executingDeviceName = null, results = listOf(result(heldHere = false, name = null))))
        assertEquals(RunDevice.Unnamed, unmet.results.single().heldBy)
    }

    @Test
    fun a_run_needing_attention_says_so_and_a_human_in_control_is_a_qualifier() {
        val attention = runOverview(run(RunState.Running, attention = listOf("finished_needing_attention"), control = "human"))
        assertTrue(attention.needsAttention)
        assertEquals(listOf("finished_needing_attention"), attention.attention)
        assertEquals("human", attention.control)
        // The orchestrator in control is the ordinary case and says nothing.
        assertNull(runOverview(run(control = "orchestrator")).control)
    }

    @Test
    fun a_rating_is_shown_when_given_and_an_unrated_run_is_a_normal_state() {
        assertEquals(RunRatingLine.Unrated, runOverview(run(RunState.Completed)).rating)
        val rated = runOverview(run(RunState.Completed, rating = RunRating.Good, ratingNote = "good enough")).rating
        assertEquals(RunRatingLine.Rated(RunRating.Good, "good enough", 1_770_000_100_000), rated)
    }

    @Test
    fun a_newer_run_is_flagged_rather_than_refused() {
        assertTrue(runOverview(run(needsNewerKoru = true)).needsNewerKoru)
        assertEquals(RunState.Unknown, RunState.of("something_new"))
        assertEquals(RunStepKind.Unknown, RunStepKind.of("something_new"))
        assertEquals(RunStepKind.ToolsCompleted, RunStepKind.of("tools_completed"))
    }

    @Test
    fun a_documents_run_pills_are_collected_by_their_run_document() {
        val other = DocumentId("10000000-0000-4000-8000-000000000009")
        val pill = InlineObject(
            InlineObjectId("50000000-0000-4000-8000-000000000001"),
            0,
            "koru.run",
            EntityReference.Run(document),
            "Tidy",
        )
        val snapshot = DocumentSnapshot(
            other,
            workspace,
            "Page",
            1,
            listOf(
                Block(BlockId("60000000-0000-4000-8000-000000000001"), content = BlockContent.Paragraph(RichText("⁠", inlineObjects = listOf(pill)))),
                Block(BlockId("60000000-0000-4000-8000-000000000002"), content = BlockContent.Embed(EntityReference.Run(other))),
                Block(BlockId("60000000-0000-4000-8000-000000000003"), content = BlockContent.Paragraph(RichText("plain"))),
            ),
        )
        assertEquals(setOf(document, other), snapshot.runReferences())
        assertTrue(DocumentSnapshot(other, workspace, "Empty", 1, emptyList()).runReferences().isEmpty())
    }
}

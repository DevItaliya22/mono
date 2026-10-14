// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.ui.run

import app.koru.mobile.domain.RunRating
import app.koru.mobile.domain.RunSnapshot
import app.koru.mobile.domain.RunState

/**
 * What the run screen and the run pill decide before they draw — pure
 * functions over a [RunSnapshot], so the decisions are testable without a
 * device and the composables only translate and lay out.
 *
 * Three rules from the roadmap live here: **D15**, the phone shows and never
 * acts, so nothing here yields a control; **D16**, the executing device is
 * named only when it is not this device; **D27**, a result is named with
 * the device that holds it.
 */

/** The one glyph a pill and the status row lead with. */
enum class RunGlyph(val text: String) {
    Waiting("○"),
    Running("●"),
    Attention("!"),
    Interrupted("‖"),
    Completed("✓"),
    Failed("✕"),
    Cancelled("⊘"),
    Unknown("?"),
}

fun runGlyph(run: RunSnapshot): RunGlyph = when (run.state) {
    RunState.Created, RunState.Starting -> RunGlyph.Waiting
    RunState.Running -> if (run.attention.isEmpty()) RunGlyph.Running else RunGlyph.Attention
    RunState.Interrupted -> RunGlyph.Interrupted
    RunState.Completed -> RunGlyph.Completed
    RunState.Failed -> RunGlyph.Failed
    RunState.Cancelled -> RunGlyph.Cancelled
    RunState.AwaitingReview, RunState.WaitingForParticipant -> RunGlyph.Attention
    RunState.Unknown -> RunGlyph.Unknown
}

/**
 * The objective's first line, bounded, for a title or a pill.
 *
 * `maxChars` counts what a person sees — grapheme clusters — not UTF-16 code
 * units: `String.length` counts an emoji or a CJK Extension B character as
 * two and `take` can cut one in half at the boundary, which renders as a
 * lone surrogate. `BreakIterator` is the JDK's own answer to "where does the
 * next character start", so a combining mark stays with its base too.
 */
fun firstLine(text: String, maxChars: Int = 60): String {
    val line = text.lineSequence().map(String::trim).firstOrNull { it.isNotEmpty() } ?: return ""
    val boundaries = graphemeBoundaries(line)
    if (boundaries.size - 1 <= maxChars) return line
    return line.substring(0, boundaries[maxChars - 1]).trimEnd() + "…"
}

/** Every grapheme boundary of `text`, from 0 to `text.length` inclusive. */
private fun graphemeBoundaries(text: String): List<Int> {
    val iterator = java.text.BreakIterator.getCharacterInstance()
    iterator.setText(text)
    val boundaries = mutableListOf(iterator.first())
    while (true) {
        val next = iterator.next()
        if (next == java.text.BreakIterator.DONE) break
        boundaries += next
    }
    return boundaries
}

/** A device as the screen names it: this phone, a name from its catalog, or
 *  a device it has never met. */
sealed interface RunDevice {
    data object Here : RunDevice
    data class Named(val name: String) : RunDevice
    data object Unnamed : RunDevice
}

private fun device(here: Boolean, name: String?): RunDevice = when {
    here -> RunDevice.Here
    name != null -> RunDevice.Named(name)
    else -> RunDevice.Unnamed
}

/**
 * The run pill's label (ADR-349): the status glyph, the executing device
 * when it is not this device (D16), and the objective's first line. Without
 * a resolved run — one that has not reached this phone yet — the pill shows
 * the label stored with it, and failing that [fallback].
 */
fun runPillLabel(run: RunSnapshot?, storedLabel: String?, fallback: String, anotherDevice: String): String {
    val stored = storedLabel?.takeIf { it.isNotBlank() }
    if (run == null) return stored ?: fallback
    val device = when (val executing = device(run.executingHere, run.executingDeviceName)) {
        RunDevice.Here -> null
        is RunDevice.Named -> executing.name
        RunDevice.Unnamed -> anotherDevice
    }
    val objective = firstLine(run.objective).ifEmpty { stored ?: fallback }
    return runGlyph(run).text + " " + listOfNotNull(device, objective).joinToString(" · ")
}

data class RunResultLine(
    val title: String,
    val sizeBytes: Long,
    val registeredAtMillis: Long,
    val heldBy: RunDevice,
)

sealed interface RunRatingLine {
    data object Unrated : RunRatingLine
    data class Rated(val rating: RunRating, val note: String?, val atMillis: Long?) : RunRatingLine
}

/** The Overview tab's decisions. */
data class RunOverview(
    val glyph: RunGlyph,
    val state: RunState,
    val stateToken: String,
    val attention: List<String>,
    val needsAttention: Boolean,
    val failureStage: String?,
    val priorAttemptUnknown: Boolean,
    val control: String?,
    val executing: RunDevice,
    val results: List<RunResultLine>,
    val rating: RunRatingLine,
    val needsNewerKoru: Boolean,
)

fun runOverview(run: RunSnapshot): RunOverview = RunOverview(
    glyph = runGlyph(run),
    state = run.state,
    stateToken = run.stateToken,
    attention = run.attention,
    needsAttention = runGlyph(run) == RunGlyph.Attention,
    failureStage = run.failureStage.takeIf { run.state == RunState.Failed },
    priorAttemptUnknown = run.state == RunState.Interrupted && run.priorAttempt == "unknown",
    control = run.control?.takeIf { it != "orchestrator" },
    executing = device(run.executingHere, run.executingDeviceName),
    results = run.results.map {
        RunResultLine(it.title, it.sizeBytes, it.registeredAtMillis, device(it.heldHere, it.heldByDeviceName))
    },
    rating = run.rating?.let { RunRatingLine.Rated(it, run.ratingNote, run.ratedAtMillis) } ?: RunRatingLine.Unrated,
    needsNewerKoru = run.needsNewerKoru,
)

/** The screen before and after the observer's first answer. */
sealed interface RunScreenState {
    data object Loading : RunScreenState
    data class Ready(val run: RunSnapshot, val overview: RunOverview) : RunScreenState
}

fun runScreenState(run: RunSnapshot?): RunScreenState =
    run?.let { RunScreenState.Ready(it, runOverview(it)) } ?: RunScreenState.Loading

/** The step tokens this build translates; anything else is [Unknown] and
 *  drawn as a step this Koru does not understand. */
enum class RunStepKind(val token: String) {
    RunCreated("run_created"),
    Started("started"),
    TurnStarted("turn_started"),
    ToolsCompleted("tools_completed"),
    ApprovalRequired("approval_required"),
    ApprovalResolved("approval_resolved"),
    WaitingForInput("waiting_for_input"),
    AttachmentAccepted("attachment_accepted"),
    AttachmentRejected("attachment_rejected"),
    ChildSessionDetected("child_session_detected"),
    TurnCompleted("turn_completed"),
    TurnCancelled("turn_cancelled"),
    TurnFailed("turn_failed"),
    Interrupted("interrupted"),
    Resumed("resumed"),
    ControlTaken("control_taken"),
    ControlReturned("control_returned"),
    ArtifactRegistered("artifact_registered"),
    Completed("completed"),
    Failed("failed"),
    Cancelled("cancelled"),
    Rated("rated"),
    Gap("gap"),
    Unknown("");

    companion object {
        fun of(token: String): RunStepKind = entries.firstOrNull { it != Unknown && it.token == token } ?: Unknown
    }
}

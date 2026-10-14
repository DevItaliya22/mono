// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.ui.run

import android.text.format.DateUtils
import android.text.format.Formatter
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import app.koru.mobile.R
import app.koru.mobile.domain.*
import app.koru.mobile.services.RunService
import app.koru.mobile.ui.theme.KoruSpacing
import java.text.NumberFormat
import kotlinx.coroutines.launch

/**
 * A delegated run, read-only (D15): an Overview and an Activity list that
 * pages a window at a time. Nothing here is an action — no Resume, no
 * Cancel, no Register, no rating control — and the screen refreshes when
 * the run observer reports a new revision, never on a timer of its own.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun RunScreen(
    run: RunSnapshot?,
    loadWindow: suspend (DocumentId, Int, Int) -> RunTimelinePage,
    onBack: () -> Unit,
) {
    var tab by remember { mutableIntStateOf(0) }
    val state = runScreenState(run)
    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Text(
                        run?.let { firstLine(it.objective) }?.takeIf(String::isNotEmpty)
                            ?: stringResource(R.string.run_title),
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                },
                navigationIcon = { TextButton(onClick = onBack) { Text(stringResource(R.string.back)) } },
            )
        },
    ) { padding ->
        Column(Modifier.padding(padding).fillMaxSize()) {
            PrimaryTabRow(selectedTabIndex = tab) {
                Tab(selected = tab == 0, onClick = { tab = 0 }, text = { Text(stringResource(R.string.run_tab_overview)) })
                Tab(selected = tab == 1, onClick = { tab = 1 }, text = { Text(stringResource(R.string.run_tab_activity)) })
            }
            when (state) {
                RunScreenState.Loading -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                    CircularProgressIndicator()
                }
                is RunScreenState.Ready -> when (tab) {
                    0 -> RunOverviewTab(state.run, state.overview)
                    else -> RunActivityTab(state.run, loadWindow)
                }
            }
        }
    }
}

@Composable
private fun deviceName(device: RunDevice): String = when (device) {
    RunDevice.Here -> stringResource(R.string.run_this_device)
    is RunDevice.Named -> device.name
    RunDevice.Unnamed -> stringResource(R.string.run_another_device)
}

@Composable
private fun statusLabel(state: RunState): String = stringResource(
    when (state) {
        RunState.Created -> R.string.run_status_created
        RunState.Starting -> R.string.run_status_starting
        RunState.Running -> R.string.run_status_running
        RunState.Interrupted -> R.string.run_status_interrupted
        RunState.Completed -> R.string.run_status_completed
        RunState.Failed -> R.string.run_status_failed
        RunState.Cancelled -> R.string.run_status_cancelled
        RunState.AwaitingReview -> R.string.run_status_awaiting_review
        RunState.WaitingForParticipant -> R.string.run_status_waiting_for_participant
        RunState.Unknown -> R.string.run_status_unknown
    },
)

@Composable
private fun stageLabel(stage: String?): String = stringResource(
    when (stage) {
        "scope_admission" -> R.string.run_stage_scope_admission
        "context_capture" -> R.string.run_stage_context_capture
        "context_materialization" -> R.string.run_stage_context_materialization
        "session_creation" -> R.string.run_stage_session_creation
        "provider_start" -> R.string.run_stage_provider_start
        "authority" -> R.string.run_stage_authority
        "turn" -> R.string.run_stage_turn
        else -> R.string.run_stage_unknown
    },
)

@Composable
private fun attentionLabel(marker: String): String = stringResource(
    when (marker) {
        "approval_required" -> R.string.run_attention_approval_required
        "waiting_for_input" -> R.string.run_attention_waiting_for_input
        "child_session_detected" -> R.string.run_attention_child_session_detected
        "finished_needing_attention" -> R.string.run_attention_finished_needing_attention
        else -> R.string.run_attention_unknown
    },
)

@Composable
private fun ratingLabel(rating: RunRating): String = stringResource(
    when (rating) {
        RunRating.Good -> R.string.run_rating_good
        RunRating.NotGood -> R.string.run_rating_not_good
    },
)

@Composable
private fun scopeLabel(kind: String): String = stringResource(
    when (kind) {
        "project_root" -> R.string.run_scope_project_root
        "project_folder" -> R.string.run_scope_project_folder
        "temporary" -> R.string.run_scope_temporary
        else -> R.string.run_scope_none
    },
)

private fun relative(atMillis: Long): String = DateUtils.getRelativeTimeSpanString(atMillis).toString()

private fun count(value: Long): String = NumberFormat.getIntegerInstance().format(value)

@Composable
private fun tokens(total: Long?, unavailableTurns: Int): String = when {
    total == null -> stringResource(R.string.run_unavailable)
    unavailableTurns > 0 -> pluralStringResource(R.plurals.run_unreported_turns, unavailableTurns, count(total), unavailableTurns)
    else -> count(total)
}

@Composable
private fun SectionTitle(text: String) {
    Text(
        text,
        modifier = Modifier.padding(horizontal = KoruSpacing.lg, vertical = KoruSpacing.sm),
        style = MaterialTheme.typography.titleMedium,
    )
}

@Composable
private fun Row(label: String, value: String, supporting: String? = null) {
    ListItem(
        headlineContent = { Text(value) },
        overlineContent = { Text(label) },
        supportingContent = supporting?.let { { Text(it) } },
    )
}

@Composable
private fun RunOverviewTab(run: RunSnapshot, overview: RunOverview) {
    val context = LocalContext.current
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
        if (overview.needsNewerKoru) {
            Surface(color = MaterialTheme.colorScheme.secondaryContainer, modifier = Modifier.fillMaxWidth()) {
                Text(stringResource(R.string.run_needs_newer_koru), Modifier.padding(KoruSpacing.md))
            }
        }
        // Status, with the glyph the pill leads with, and what qualifies it.
        val qualifiers = buildList {
            overview.failureStage?.let { add(stringResource(R.string.run_failed_at, stageLabel(it))) }
            if (overview.priorAttemptUnknown) add(stringResource(R.string.run_prior_attempt_unknown))
            when (overview.control) {
                "human" -> add(stringResource(R.string.run_control_human))
                "handover_pending" -> add(stringResource(R.string.run_control_handover))
            }
            overview.attention.forEach { add(attentionLabel(it)) }
        }
        Row(
            stringResource(R.string.run_status),
            "${overview.glyph.text} ${statusLabel(overview.state)}",
            qualifiers.takeIf { it.isNotEmpty() }?.joinToString("\n"),
        )
        // D16: the executing device, named when it is not this one — and
        // D15: this phone only shows the run.
        val executing = when (overview.executing) {
            RunDevice.Here -> stringResource(R.string.run_executing_here)
            else -> stringResource(R.string.run_executing_on, deviceName(overview.executing))
        }
        Row(
            stringResource(R.string.run_device),
            executing,
            if (overview.executing == RunDevice.Here) null else stringResource(R.string.run_read_only, deviceName(overview.executing)),
        )
        Row(stringResource(R.string.run_agent), run.agent)
        Row(stringResource(R.string.run_scope), scopeLabel(run.scopeKind))
        Row(stringResource(R.string.run_context), pluralStringResource(R.plurals.run_context_items, run.contextCount, run.contextCount))
        Row(stringResource(R.string.run_created), relative(run.createdAtMillis))

        // D27: results named, with the device that holds the content.
        SectionTitle(stringResource(R.string.run_results))
        if (overview.results.isEmpty()) {
            Text(stringResource(R.string.run_no_results), Modifier.padding(horizontal = KoruSpacing.lg))
        }
        overview.results.forEach { result ->
            ListItem(
                headlineContent = { Text(result.title, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                supportingContent = {
                    Column {
                        Text(
                            stringResource(
                                R.string.run_result_registered,
                                Formatter.formatShortFileSize(context, result.sizeBytes),
                                relative(result.registeredAtMillis),
                            ),
                        )
                        Text(
                            when (result.heldBy) {
                                RunDevice.Here -> stringResource(R.string.run_result_here)
                                else -> stringResource(R.string.run_result_on_device, deviceName(result.heldBy))
                            },
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                },
            )
        }

        // D39: the numbers, plainly. Unavailable is unavailable, never zero.
        val economics = run.economics
        SectionTitle(stringResource(R.string.run_economics))
        Row(
            stringResource(R.string.run_turns),
            if (economics.humanTurns > 0) {
                stringResource(R.string.run_turns_with_human, economics.orchestratorTurns + economics.humanTurns, economics.humanTurns)
            } else {
                count(economics.orchestratorTurns.toLong())
            },
        )
        Row(stringResource(R.string.run_tokens_in), tokens(economics.inputTokens, economics.inputUnavailableTurns))
        Row(stringResource(R.string.run_tokens_out), tokens(economics.outputTokens, economics.outputUnavailableTurns))
        Row(stringResource(R.string.run_tokens_cached), tokens(economics.cachedInputTokens, economics.cachedInputUnavailableTurns))
        Row(stringResource(R.string.run_tool_calls), count(economics.toolCalls.toLong()))
        Row(stringResource(R.string.run_interventions), count(economics.interventions.toLong()))
        Row(
            stringResource(R.string.run_elapsed),
            economics.elapsedMs?.let { DateUtils.formatElapsedTime(it / 1_000) } ?: stringResource(R.string.run_unavailable),
        )
        Row(stringResource(R.string.run_models), count(economics.models.toLong()))

        // D44: the rating, read here and given elsewhere (D15).
        SectionTitle(stringResource(R.string.run_rating))
        when (val rating = overview.rating) {
            RunRatingLine.Unrated -> Row(stringResource(R.string.run_rating), stringResource(R.string.run_unrated))
            is RunRatingLine.Rated -> Row(
                rating.atMillis?.let { stringResource(R.string.run_rated_at, relative(it)) } ?: stringResource(R.string.run_rating),
                ratingLabel(rating.rating),
                rating.note,
            )
        }
        Spacer(Modifier.height(KoruSpacing.xl))
    }
}

@Composable
private fun RunActivityTab(run: RunSnapshot, loadWindow: suspend (DocumentId, Int, Int) -> RunTimelinePage) {
    var entries by remember(run.documentId) { mutableStateOf<List<RunTimelineEntry>>(emptyList()) }
    // How many of the newest entries are on screen, and the length the last
    // answer reported — the two numbers paging and refreshing share.
    var loaded by remember(run.documentId) { mutableIntStateOf(0) }
    var seenLen by remember(run.documentId) { mutableIntStateOf(0) }
    var loading by remember(run.documentId) { mutableStateOf(false) }
    var failed by remember(run.documentId) { mutableStateOf(false) }
    val scope = rememberCoroutineScope()

    // The observer's revision, never a timer, is what refreshes: the first
    // window on open, then only the entries appended since — prepended, so
    // a person scrolled back keeps their place (proposal §9, step 4).
    LaunchedEffect(run.documentId, run.revision) {
        val added = run.timelineLen - seenLen
        if (entries.isEmpty() && loaded == 0) {
            loading = true
            runCatching { loadWindow(run.documentId, 0, RunService.WINDOW) }
                .onSuccess { page -> entries = page.entries; loaded = page.entries.size; failed = false }
                .onFailure { failed = true }
            loading = false
        } else if (added > 0) {
            runCatching { loadWindow(run.documentId, 0, added) }
                .onSuccess { page -> entries = page.entries + entries; loaded += page.entries.size }
        }
        seenLen = run.timelineLen
    }

    LazyColumn(Modifier.fillMaxSize()) {
        items(entries) { entry ->
            RunActivityRow(entry)
            HorizontalDivider()
        }
        item {
            when {
                loading -> LinearProgressIndicator(Modifier.fillMaxWidth())
                failed -> Text(stringResource(R.string.error_unknown), Modifier.padding(KoruSpacing.lg))
                entries.isEmpty() -> Text(stringResource(R.string.run_no_activity), Modifier.padding(KoruSpacing.lg))
                loaded < run.timelineLen -> Box(Modifier.fillMaxWidth().padding(KoruSpacing.md), contentAlignment = Alignment.Center) {
                    TextButton(onClick = {
                        scope.launch {
                            loading = true
                            runCatching { loadWindow(run.documentId, loaded, RunService.WINDOW) }
                                .onSuccess { page -> entries = entries + page.entries; loaded += page.entries.size }
                                .onFailure { failed = true }
                            loading = false
                        }
                    }) { Text(stringResource(R.string.run_earlier)) }
                }
            }
        }
    }
}

@Composable
private fun stepLabel(step: RunTimelineEntry.Step): String = when (RunStepKind.of(step.step)) {
    RunStepKind.RunCreated -> stringResource(R.string.run_step_run_created)
    RunStepKind.Started -> stringResource(R.string.run_step_started)
    RunStepKind.TurnStarted -> if (step.originator == "human") {
        stringResource(R.string.run_step_turn_started_human)
    } else {
        stringResource(R.string.run_step_turn_started)
    }
    RunStepKind.ToolsCompleted -> if ((step.failed ?: 0) > 0) {
        pluralStringResource(R.plurals.run_step_tools_failed, step.calls ?: 0, step.calls ?: 0, step.failed ?: 0)
    } else {
        pluralStringResource(R.plurals.run_step_tools_completed, step.calls ?: 0, step.calls ?: 0)
    }
    RunStepKind.ApprovalRequired -> stringResource(R.string.run_step_approval_required)
    RunStepKind.ApprovalResolved -> when (step.outcome) {
        "approved" -> stringResource(R.string.run_step_approval_approved)
        "rejected" -> stringResource(R.string.run_step_approval_rejected)
        else -> stringResource(R.string.run_step_approval_expired)
    }
    RunStepKind.WaitingForInput -> stringResource(R.string.run_step_waiting_for_input)
    RunStepKind.AttachmentAccepted -> stringResource(R.string.run_step_attachment_accepted)
    RunStepKind.AttachmentRejected -> stringResource(R.string.run_step_attachment_rejected)
    RunStepKind.ChildSessionDetected -> stringResource(R.string.run_step_child_session_detected)
    RunStepKind.TurnCompleted -> if ((step.artifactsRegistered ?: 0) > 0) {
        pluralStringResource(R.plurals.run_step_turn_completed_results, step.artifactsRegistered ?: 0, step.artifactsRegistered ?: 0)
    } else {
        stringResource(R.string.run_step_turn_completed)
    }
    RunStepKind.TurnCancelled -> if (step.reason == "takeover_boundary") {
        stringResource(R.string.run_step_turn_cancelled_takeover)
    } else {
        stringResource(R.string.run_step_turn_cancelled)
    }
    RunStepKind.TurnFailed -> stringResource(R.string.run_step_turn_failed, stageLabel(step.stage))
    RunStepKind.Interrupted -> stringResource(R.string.run_step_interrupted)
    RunStepKind.Resumed -> if (step.via == "native") {
        stringResource(R.string.run_step_resumed_native)
    } else {
        stringResource(R.string.run_step_resumed_recorded)
    }
    RunStepKind.ControlTaken -> if (step.discovered == true) {
        stringResource(R.string.run_step_control_discovered)
    } else {
        stringResource(R.string.run_step_control_taken)
    }
    RunStepKind.ControlReturned -> stringResource(R.string.run_step_control_returned)
    RunStepKind.ArtifactRegistered -> stringResource(R.string.run_step_artifact_registered)
    RunStepKind.Completed -> stringResource(R.string.run_step_completed)
    RunStepKind.Failed -> stringResource(R.string.run_step_failed, stageLabel(step.stage))
    RunStepKind.Cancelled -> stringResource(R.string.run_step_cancelled)
    RunStepKind.Rated -> stringResource(R.string.run_step_rated, step.rating?.let { ratingLabel(it) } ?: "")
    RunStepKind.Gap -> step.eventsLost?.let { pluralStringResource(R.plurals.run_step_gap_count, it, it) }
        ?: stringResource(R.string.run_step_gap)
    RunStepKind.Unknown -> stringResource(R.string.run_step_unknown)
}

@Composable
private fun RunActivityRow(entry: RunTimelineEntry) {
    when (entry) {
        is RunTimelineEntry.Step -> ListItem(
            headlineContent = { Text(stepLabel(entry)) },
            supportingContent = { Text(relative(entry.atMillis)) },
        )
        is RunTimelineEntry.Usage -> {
            val input = entry.inputTokens?.let(::count) ?: stringResource(R.string.run_unavailable)
            val output = entry.outputTokens?.let(::count) ?: stringResource(R.string.run_unavailable)
            ListItem(
                headlineContent = {
                    Text(
                        if (entry.completeness == "complete") {
                            stringResource(R.string.run_usage, input, output)
                        } else {
                            stringResource(R.string.run_usage_incomplete, input, output)
                        },
                    )
                },
                colors = ListItemDefaults.colors(headlineColor = MaterialTheme.colorScheme.onSurfaceVariant),
            )
        }
        RunTimelineEntry.Gap -> ListItem(
            headlineContent = { Text(stringResource(R.string.run_step_unknown)) },
            colors = ListItemDefaults.colors(headlineColor = MaterialTheme.colorScheme.onSurfaceVariant),
        )
    }
}

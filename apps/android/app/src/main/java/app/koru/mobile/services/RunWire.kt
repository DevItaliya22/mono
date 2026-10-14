// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import app.koru.contract.api.v1.RunTimelineRow as WireRunTimelineRow
import app.koru.contract.api.v1.RunTimelineWindow as WireRunTimelineWindow
import app.koru.contract.api.v1.RunView as WireRunView
import app.koru.mobile.domain.*

/**
 * The generated run DTOs become domain values here and nowhere else. Every
 * `optional` field is read through its `has…` — an absent token stays null
 * rather than becoming an empty string the screen would then have to guess
 * at.
 */
internal fun WireRunView.toSnapshot(): RunSnapshot {
    if (hasErrorCode()) error(errorCode)
    val economics = economics.takeIf { hasEconomics() }
    return RunSnapshot(
        documentId = DocumentId(documentId),
        workspaceId = WorkspaceId(workspaceId),
        runId = runId,
        revision = revision,
        state = RunState.of(status),
        stateToken = status,
        attention = attentionList.toList(),
        control = control.takeIf { hasControl() },
        failureStage = failureStage.takeIf { hasFailureStage() },
        priorAttempt = priorAttempt.takeIf { hasPriorAttempt() },
        agent = agent,
        objective = objective,
        scopeKind = scopeKind,
        contextCount = contextCount,
        executingDeviceId = executingDeviceId,
        executingDeviceName = executingDeviceName.takeIf { hasExecutingDeviceName() },
        executingHere = executingHere,
        createdAtMillis = createdAtMillis,
        results = resultsList.map {
            RunResultSummary(
                artifactId = it.artifactId,
                heldByDeviceId = it.heldByDeviceId,
                heldByDeviceName = it.heldByDeviceName.takeIf { _ -> it.hasHeldByDeviceName() },
                heldHere = it.heldHere,
                title = it.title,
                sizeBytes = it.sizeBytes,
                registeredAtMillis = it.registeredAtMillis,
            )
        },
        economics = economics?.let {
            RunEconomicsSummary(
                orchestratorTurns = it.orchestratorTurns,
                humanTurns = it.humanTurns,
                turnsWithUsage = it.turnsWithUsage,
                inputTokens = it.inputTokens.takeIf { _ -> it.hasInputTokens() },
                inputUnavailableTurns = it.inputUnavailableTurns,
                outputTokens = it.outputTokens.takeIf { _ -> it.hasOutputTokens() },
                outputUnavailableTurns = it.outputUnavailableTurns,
                cachedInputTokens = it.cachedInputTokens.takeIf { _ -> it.hasCachedInputTokens() },
                cachedInputUnavailableTurns = it.cachedInputUnavailableTurns,
                toolCalls = it.toolCalls,
                interventions = it.interventions,
                elapsedMs = it.elapsedMs.takeIf { _ -> it.hasElapsedMs() },
                models = it.models,
                complete = it.complete,
            )
        } ?: RunEconomicsSummary(),
        rating = RunRating.of(rating.takeIf { hasRating() }),
        ratingNote = ratingNote.takeIf { hasRatingNote() },
        ratedAtMillis = ratedAtMillis.takeIf { hasRatedAtMillis() },
        timelineLen = timelineLen,
        needsNewerKoru = needsNewerKoru,
    )
}

internal fun WireRunTimelineWindow.toPage(): RunTimelinePage {
    if (hasErrorCode()) error(errorCode)
    return RunTimelinePage(
        documentId = DocumentId(documentId),
        newestFirstFrom = newestFirstFrom,
        entries = rowsList.map(WireRunTimelineRow::toEntry),
        timelineLen = timelineLen,
        revision = revision,
    )
}

private fun WireRunTimelineRow.toEntry(): RunTimelineEntry = when (kind) {
    "step" -> RunTimelineEntry.Step(
        id = id,
        atMillis = atMillis,
        step = step,
        originator = originator.takeIf { hasOriginator() },
        calls = calls.takeIf { hasCalls() },
        failed = failed.takeIf { hasFailed() },
        outcome = outcome.takeIf { hasOutcome() },
        artifactsRegistered = artifactsRegistered.takeIf { hasArtifactsRegistered() },
        reason = reason.takeIf { hasReason() },
        stage = stage.takeIf { hasStage() },
        priorAttempt = priorAttempt.takeIf { hasPriorAttempt() },
        via = via.takeIf { hasVia() },
        discovered = discovered.takeIf { hasDiscovered() },
        rating = RunRating.of(rating.takeIf { hasRating() }),
        eventsLost = eventsLost.takeIf { hasEventsLost() },
        refs = refs,
    )
    "usage" -> RunTimelineEntry.Usage(
        id = id,
        inputTokens = inputTokens.takeIf { hasInputTokens() },
        outputTokens = outputTokens.takeIf { hasOutputTokens() },
        cachedInputTokens = cachedInputTokens.takeIf { hasCachedInputTokens() },
        revision = revision,
        completeness = completeness.takeIf { hasCompleteness() },
    )
    // A gap, and any kind this build does not know: a step in its place.
    else -> RunTimelineEntry.Gap
}

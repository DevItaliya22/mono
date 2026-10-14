// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import app.koru.mobile.domain.*
import app.koru.mobile.logging.AppLogger
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.onEach

/**
 * A delegated run through the embedded store, read-only (D15).
 *
 * `observe` is the run's counterpart of the document snapshot observer: it
 * samples the run at the same interval and emits only when something
 * changed. The native host keeps one fork per open and reuses it while the
 * version stands still, so a sample that finds nothing new costs the catalog
 * reads and no fork; a window is read from that fork without a store round
 * trip (P1.0-STORAGE-PROPOSAL.md §9).
 */
class NativeRunService(
    private val native: NativeDeviceLinkService,
    private val logger: AppLogger,
) : RunService {
    override suspend fun view(id: DocumentId): RunSnapshot = translate(id) { native.run(id) }

    override suspend fun timeline(id: DocumentId, newestFirstFrom: Int, len: Int): RunTimelinePage =
        translate(id) { native.runTimeline(id, newestFirstFrom, len) }

    override fun observe(id: DocumentId): Flow<RunSnapshot> = flow {
        while (true) {
            emit(view(id))
            delay(RUN_POLL_MILLIS)
        }
    }.distinctUntilChanged().onEach { snapshot ->
        // Ids, counts and closed tokens only — never the objective.
        logger.event(
            "run_snapshot_observed",
            mapOf(
                "document_id" to snapshot.documentId.value,
                "revision" to snapshot.revision,
                "state" to snapshot.stateToken,
                "timeline_len" to snapshot.timelineLen,
                "results" to snapshot.results.size,
            ),
        )
    }

    private suspend fun <T> translate(id: DocumentId, read: suspend () -> T): T = try {
        read()
    } catch (error: IllegalStateException) {
        val code = error.message ?: "unknown"
        throw when (code) {
            "document_unlocated", "document_not_listed", "document_is_not_a_run" ->
                AppFailure(AppErrorCode.NotFound, "run ${id.value} was not found ($code)")
            "run_needs_newer_koru" ->
                AppFailure(AppErrorCode.Unsupported, "run ${id.value} needs a newer Koru ($code)")
            else -> AppFailure(AppErrorCode.Unknown, "run ${id.value} failed to open ($code)")
        }
    }

    private companion object {
        /** The document observer's interval, shared so the phone has one
         *  sampling rhythm rather than two. */
        const val RUN_POLL_MILLIS = 500L
    }
}

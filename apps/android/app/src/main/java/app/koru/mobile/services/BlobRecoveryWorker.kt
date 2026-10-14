// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import android.content.Context
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import app.koru.mobile.KoruApplication

/**
 * Resume a blob transfer the operating system interrupted.
 *
 * **Not a tick.** D244 is explicit that an Android app makes no Cloud passes
 * while backgrounded and that its account poll is the heartbeat; this worker
 * does not fight that and does not become a second one. It is a *one-shot*,
 * enqueued when the process starts and when a transfer is known to be in
 * flight, whose whole job is that the picture a person was half-way through
 * receiving finishes after Android killed the process — and after an
 * app-storage restart, since the staging file on disk is the resume authority
 * and this build remembers nothing about it in memory.
 *
 * It retries only while bytes are actually outstanding and nothing is holding
 * them, which is what bounds it: a paused Workspace ends the work rather than
 * backing off forever, because the thing that clears a pause is a connection
 * change and `AndroidBlobPolicy` already answers that one without a worker.
 *
 * The engine does the deciding. This calls `recover_blobs`, which is the
 * staging sweep plus one pass, and reads the status back to decide whether
 * anything is still moving.
 */
class BlobRecoveryWorker(
    context: Context,
    parameters: WorkerParameters,
) : CoroutineWorker(context, parameters) {

    override suspend fun doWork(): Result {
        val container = (applicationContext as? KoruApplication)?.container ?: return Result.success()
        val logger = container.logger
        val workspaces = runCatching { container.deviceLink.inventory() }
            .getOrNull()
            ?.workspaces
            ?.map { it.id }
            .orEmpty()
        if (workspaces.isEmpty()) return Result.success()

        var moving = false
        var held = false
        for (workspace in workspaces) {
            runCatching { container.deviceLink.recoverBlobs(workspace) }
                .onFailure {
                    logger.event(
                        "blob_recovery_error",
                        mapOf("workspace_id" to workspace, "message" to (it.message ?: "?")),
                    )
                }
            val status = runCatching { container.deviceLink.blobStatus(workspace) }.getOrNull()
                ?: continue
            when (status.phase) {
                BlobTransferPhase.Working -> moving = true
                BlobTransferPhase.Paused, BlobTransferPhase.Refused -> held = true
                // `Waiting` is neither. Nothing is moving that a retry would
                // finish, and nothing is held that a retry would clear: the
                // bytes are on a device that is off, and the trigger that
                // changes that is its upload, not this worker asking again.
                else -> Unit
            }
            if (status.phase != BlobTransferPhase.Idle) {
                logger.event(
                    "blob_recovery",
                    mapOf(
                        "workspace_id" to workspace,
                        "phase" to status.phase.name,
                        "outstanding" to status.outstanding.toString(),
                        // Not the same number: a file being uploaded is
                        // already on this device, so it moves while nothing
                        // is outstanding (D259).
                        "moving" to status.moving.toString(),
                        "pending_reason" to status.pendingReason,
                    ),
                )
            }
        }
        // Still carrying something, and nothing holding it: come back and
        // finish it. Held or finished, the work ends here — the connection
        // change that clears a pause raises the engine's own trigger.
        return if (moving && !held) Result.retry() else Result.success()
    }

    companion object {
        /** One name, so a second enqueue joins the work rather than racing it. */
        const val NAME = "koru-blob-recovery"

        /**
         * Ask Android to resume interrupted transfers once there is a network.
         *
         * `KEEP` rather than `REPLACE`: a recovery already scheduled is the
         * same recovery, and replacing it would restart its backoff every time
         * the app was opened.
         *
         * **Answers whether it was scheduled.** WorkManager initialises from a
         * content provider and can refuse for reasons this app does not
         * control; swallowing that would mean recovery silently never
         * happening while every screen looked healthy, which is the failure
         * mode this branch has already paid for twice on this platform.
         */
        fun enqueue(context: Context): kotlin.Result<Unit> {
            val request = OneTimeWorkRequestBuilder<BlobRecoveryWorker>()
                // **`CONNECTED`, not `UNMETERED`.** Whether a metered link may
                // be used is D50's question and the engine's answer; deciding
                // it here as well would mean a person who allowed cellular
                // transfers still got nothing until they found Wi-Fi.
                .setConstraints(
                    Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build(),
                )
                .addTag(NAME)
                .build()
            return runCatching {
                WorkManager.getInstance(context)
                    .enqueueUniqueWork(NAME, ExistingWorkPolicy.KEEP, request)
                Unit
            }
        }
    }
}

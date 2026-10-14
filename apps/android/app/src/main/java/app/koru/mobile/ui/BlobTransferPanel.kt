// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.ui

import android.text.format.Formatter
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.CloudDone
import androidx.compose.material.icons.outlined.CloudOff
import androidx.compose.material.icons.outlined.CloudQueue
import androidx.compose.material.icons.outlined.ErrorOutline
import androidx.compose.material.icons.outlined.PauseCircleOutline
import androidx.compose.material.icons.outlined.SyncDisabled
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedCard
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import app.koru.mobile.R
import app.koru.mobile.services.BlobHold
import app.koru.mobile.services.BlobTransferPhase
import app.koru.mobile.services.BlobTransferReport
import app.koru.mobile.services.canClearLocal
import app.koru.mobile.services.ReleasedFiles
import androidx.compose.runtime.setValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.getValue
import androidx.compose.material3.AlertDialog
import app.koru.mobile.services.BLOB_NO_ROOM
import app.koru.mobile.ui.theme.KoruSpacing

/**
 * What this Workspace's files are doing, at the top of its own screen.
 *
 * **Not the desktop's panel.** D256 put the counts in the Cloud sync context
 * pane because that is where a desktop person looks for sync truth; a phone
 * has no context pane, and the place somebody on a phone is standing when they
 * wonder where a picture went is the Workspace they opened. So it is a card
 * above the list, it is four numbers and one sentence, and it disappears when
 * a Workspace has no files at all.
 *
 * Three rules it exists to keep:
 *
 * - **A file is not a picture** (D259). Every noun here is *file*, and the
 *   counts are the engine's own — which never consulted a media type. A
 *   surface that said "pictures" and counted only some of them is how a
 *   1.9 GiB archive moved invisibly.
 * - **A pause is not a refusal** (plan §13). Six holds, six sentences, and the
 *   two a person can act on carry the action.
 * - **"Up to date" is never shown over something in flight.** That is
 *   [BlobTransferReport.upToDate]'s business and is pinned by a test, because
 *   it is exactly the claim that was wrong on hardware.
 */
@Composable
fun BlobTransferPanel(
    report: BlobTransferReport,
    onAllowOnCellular: () -> Unit,
    modifier: Modifier = Modifier,
    onClearLocal: () -> Unit = {},
    lastRelease: ReleasedFiles? = null,
    onDismissRelease: () -> Unit = {},
) {
    if (!report.worthShowing) return
    val context = LocalContext.current
    // **Free up space asks first.** It deletes copies, however recoverable,
    // so it goes through a question with a red answer — the same shape every
    // deletion on the desktop takes.
    var askingToClear by remember { mutableStateOf(false) }
    if (askingToClear) {
        AlertDialog(
            onDismissRequest = { askingToClear = false },
            confirmButton = {
                TextButton(onClick = { askingToClear = false; onClearLocal() }) {
                    Text(stringResource(R.string.blob_clear_confirm), color = MaterialTheme.colorScheme.error)
                }
            },
            dismissButton = {
                TextButton(onClick = { askingToClear = false }) { Text(stringResource(R.string.blob_clear_cancel)) }
            },
            title = { Text(stringResource(R.string.blob_clear_title)) },
            text = { Text(stringResource(R.string.blob_clear_body)) },
        )
    }
    OutlinedCard(modifier.fillMaxWidth().padding(KoruSpacing.md)) {
        Column(
            Modifier.padding(KoruSpacing.md),
            verticalArrangement = Arrangement.spacedBy(KoruSpacing.sm),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(transferIcon(report), contentDescription = null, tint = transferTint(report))
                Spacer(Modifier.width(KoruSpacing.sm))
                Column(Modifier.weight(1f)) {
                    Text(
                        stringResource(R.string.transfers),
                        style = MaterialTheme.typography.titleMedium,
                    )
                    Text(
                        transferSentence(report),
                        style = MaterialTheme.typography.bodyMedium,
                        color = transferTint(report),
                        // A person watching a file move is told when it stops
                        // moving without having to look away and back.
                        modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite },
                    )
                }
            }
            transferExplainer(report)?.let {
                Text(it, style = MaterialTheme.typography.bodySmall)
            }
            if (report.phase == BlobTransferPhase.Working && report.bytesTotal > 0) {
                val complete = report.bytesComplete.coerceAtMost(report.bytesTotal)
                LinearProgressIndicator(
                    progress = { complete.toFloat() / report.bytesTotal.toFloat() },
                    modifier = Modifier.fillMaxWidth(),
                )
                Text(
                    stringResource(
                        R.string.blob_progress,
                        // The platform's own size formatting: units and digit
                        // grouping are a localization, not a string we choose.
                        Formatter.formatShortFileSize(context, complete),
                        Formatter.formatShortFileSize(context, report.bytesTotal),
                    ),
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            HorizontalDivider()
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                // Four numbers, four fixed labels. Nothing here pluralizes a
                // noun, which is what keeps one layout honest in eight
                // languages rather than eight approximations of one.
                //
                // Three of them are a pass's tally rather than a census, so a
                // pass that stopped at a hold before asking the Store about a
                // single file shows them as unknown. "0 on this device" over a
                // Workspace whose pictures are all sitting on the phone would
                // be a worse lie than the one D259 caught.
                val unknown = stringResource(R.string.blob_count_unknown)
                TransferCount(report.referenced.toString(), stringResource(R.string.blob_referenced))
                TransferCount(
                    if (report.countsResolved) report.onThisDevice.toString() else unknown,
                    stringResource(R.string.blob_on_device),
                )
                TransferCount(
                    if (report.countsResolved) report.onCloud.toString() else unknown,
                    stringResource(R.string.blob_on_cloud),
                )
                TransferCount(
                    if (report.countsResolved) report.moving.toString() else unknown,
                    stringResource(R.string.blob_moving),
                )
            }
            // **D50's first override, where the pause is.** The second — ask
            // for this one file now — lives on the file itself, because that
            // is the only place a person can say *which* one.
            if (report.cellularWouldHelp) {
                TextButton(onClick = onAllowOnCellular) {
                    Text(stringResource(R.string.blob_allow_cellular))
                }
            }
            // **What the last sweep did**, said until dismissed — and `kept`
            // named whenever it is not zero, because each of those is a file
            // this phone may be the only place for.
            lastRelease?.let { released ->
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        releaseSentence(context, released),
                        style = MaterialTheme.typography.bodySmall,
                        modifier = Modifier.weight(1f).semantics { liveRegion = LiveRegionMode.Polite },
                    )
                    TextButton(onClick = onDismissRelease) { Text(stringResource(R.string.blob_release_dismiss)) }
                }
            }
            if (canClearLocal(report)) {
                TextButton(onClick = { askingToClear = true }) {
                    Text(stringResource(R.string.blob_clear_local))
                }
            }
        }
    }
}

/** The sweep's answer as one sentence, with the kept count named whenever it
 *  is not zero. */
@Composable
private fun releaseSentence(context: android.content.Context, released: ReleasedFiles): String = when {
    released.released == 0 && released.kept == 0 && released.failureCode.isNotEmpty() ->
        stringResource(R.string.blob_clear_failed)
    released.kept > 0 -> stringResource(
        R.string.blob_cleared_kept,
        released.released,
        Formatter.formatShortFileSize(context, released.bytesFreed),
        released.kept,
    )
    else -> stringResource(
        R.string.blob_cleared,
        released.released,
        Formatter.formatShortFileSize(context, released.bytesFreed),
    )
}

@Composable
private fun TransferCount(value: String, label: String) {
    Column(horizontalAlignment = Alignment.CenterHorizontally) {
        Text(value, style = MaterialTheme.typography.titleLarge)
        Text(
            label,
            style = MaterialTheme.typography.labelSmall,
            textAlign = TextAlign.Center,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

/** One sentence per state, and a different one per hold (plan §13). */
@Composable
private fun transferSentence(report: BlobTransferReport): String = when {
    report.phase == BlobTransferPhase.Refused -> stringResource(R.string.blob_state_refused)
    report.hold == BlobHold.Metered -> stringResource(R.string.blob_state_paused_metered)
    report.hold == BlobHold.LowData -> stringResource(R.string.blob_state_paused_low_data)
    report.hold == BlobHold.Offline -> stringResource(R.string.blob_state_paused_offline)
    report.hold == BlobHold.Person -> stringResource(R.string.blob_state_paused_person)
    report.hold == BlobHold.Cloud -> stringResource(R.string.blob_state_no_cloud)
    report.failureCode == BLOB_NO_ROOM -> stringResource(R.string.blob_state_no_room)
    // A code a person reads back to us, verbatim: translating it would lose
    // the only thing it is for.
    report.failureCode.isNotEmpty() ->
        stringResource(R.string.blob_state_failed, report.failureCode)
    // Above `Working` because a Workspace can be both: this phone may be
    // sending one file while another it wants sits on a laptop that is off.
    // The one it cannot act on is the one worth saying.
    report.phase == BlobTransferPhase.Waiting ->
        stringResource(R.string.blob_state_waiting_holder)
    report.phase == BlobTransferPhase.Working -> stringResource(R.string.blob_state_working)
    report.upToDate -> stringResource(R.string.blob_state_up_to_date)
    else -> stringResource(R.string.blob_state_idle)
}

/** What clears this, when the answer is not obvious from the sentence. */
@Composable
private fun transferExplainer(report: BlobTransferReport): String? = when {
    report.phase == BlobTransferPhase.Refused -> stringResource(R.string.blob_refused_explainer)
    report.hold == BlobHold.Cloud -> stringResource(R.string.blob_no_cloud_explainer)
    // The sentence says what is happening; this says the part a person would
    // otherwise assume, which is that something broke.
    report.phase == BlobTransferPhase.Waiting ->
        stringResource(R.string.blob_waiting_holder_explainer)
    report.phase == BlobTransferPhase.Paused && report.hold != BlobHold.Person ->
        stringResource(R.string.blob_pause_explainer)
    else -> null
}

@Composable
private fun transferIcon(report: BlobTransferReport) = when {
    report.phase == BlobTransferPhase.Refused -> Icons.Outlined.SyncDisabled
    report.hold == BlobHold.Person -> Icons.Outlined.PauseCircleOutline
    report.hold == BlobHold.Cloud -> Icons.Outlined.CloudOff
    report.phase == BlobTransferPhase.Paused -> Icons.Outlined.CloudOff
    report.phase == BlobTransferPhase.Failed -> Icons.Outlined.ErrorOutline
    // The queue icon, not the error one: these files are coming.
    report.phase == BlobTransferPhase.Waiting -> Icons.Outlined.CloudQueue
    report.upToDate -> Icons.Outlined.CloudDone
    else -> Icons.Outlined.CloudQueue
}

@Composable
private fun transferTint(report: BlobTransferReport) = when (report.phase) {
    BlobTransferPhase.Failed -> MaterialTheme.colorScheme.error
    // A pause and a refusal are not errors and must not be painted as ones:
    // a red sentence over "waiting for Wi-Fi" is the §13 defect in a colour.
    else -> MaterialTheme.colorScheme.onSurfaceVariant
}

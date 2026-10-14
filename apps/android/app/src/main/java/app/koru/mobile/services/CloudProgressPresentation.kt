// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import app.koru.contract.api.v1.CloudSyncStatus
import app.koru.contract.api.v1.CloudWaitingDocuments
import app.koru.mobile.domain.ProfileId
import app.koru.mobile.domain.WorkspaceId
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull

/*
 * **How far a Profile's Cloud sync has got**, across every Workspace: the
 * phone's copy of the desktop's Manage Profiles Cloud sync section
 * (`apps/koru/src/manage/profiles/progress.rs`, maintainer 2026-09-24).
 *
 * The rules here are that file's, one for one — what is held, which rows
 * open, what a documents count may be read from, what a row says when it is
 * settled — so the two surfaces cannot describe one Workspace two ways. The
 * words are the screen's: every value below is a count, an id, a title, a
 * device name or a stable code, and `CloudProgressScreen.kt` picks the
 * string resource for each.
 */

/** Whether a live Cloud permit covers the Workspace, as the Store reported it. */
data class CloudPermitFacts(val enabled: Boolean, val stage: String = "", val errorCode: String = "")

/** The last committed coverage claim. */
data class CloudCoverageFacts(val state: String, val documentsRequired: Int, val documentsMissing: Int)

/** The Workspace's files, as the Store's transfer report has them. */
data class CloudFilesFacts(
    val referenced: Int = 0,
    val present: Int = 0,
    val durable: Int = 0,
    val uploading: Int = 0,
    val downloading: Int = 0,
    val missing: Int = 0,
    val pendingReason: String = "",
    val direction: String = "",
    val path: String = "",
    val unreadableReferences: Int = 0,
    val paused: Boolean = false,
    val coverage: CloudCoverageFacts? = null,
)

/** One Workspace's Cloud sync state. `null` halves mean *no fact*, never zero. */
data class CloudWorkspaceStatus(val sync: CloudPermitFacts? = null, val files: CloudFilesFacts? = null) {
    companion object {
        /** `null` when the host could not answer about this Workspace. */
        fun from(message: CloudSyncStatus): CloudWorkspaceStatus? {
            if (message.hasErrorCode()) return null
            return CloudWorkspaceStatus(
                sync = if (message.hasSync()) message.sync.let {
                    CloudPermitFacts(it.enabled, it.stage, it.errorCode)
                } else null,
                files = if (message.hasFiles()) message.files.let { files ->
                    CloudFilesFacts(
                        referenced = files.referenced,
                        present = files.present,
                        durable = files.durable,
                        uploading = files.uploading,
                        downloading = files.downloading,
                        missing = files.missing,
                        pendingReason = files.pendingReason,
                        direction = files.direction,
                        path = files.path,
                        unreadableReferences = files.unreadableReferences,
                        paused = files.paused,
                        coverage = if (files.hasCoverage()) files.coverage.let {
                            CloudCoverageFacts(it.state, it.documentsRequired, it.documentsMissing)
                        } else null,
                    )
                } else null,
            )
        }
    }
}

/** One document waiting on a file Koru Cloud does not hold yet. */
data class CloudWaitingEntry(
    val documentId: String,
    /** Empty when the manifest has no title; the screen says *Untitled*. */
    val title: String,
    val missingFiles: Int,
    /** Empty is a real answer: no device this one knows of holds the files. */
    val holders: List<String>,
) {
    companion object {
        /** `null` when the list could not be read. */
        fun listFrom(message: CloudWaitingDocuments): List<CloudWaitingEntry>? =
            if (message.hasErrorCode()) null
            else message.documentsList.map {
                CloudWaitingEntry(it.documentId, it.title, it.missingFiles, it.holdersList.toList())
            }
    }
}

/** What an open row says under itself. */
sealed interface CloudWaiting {
    /** Opened since the last read began; the next one brings the list. */
    data object Reading : CloudWaiting
    /** The host would not say. */
    data object Unavailable : CloudWaiting
    data class Documents(val documents: List<CloudWaitingEntry>) : CloudWaiting
}

/** One Workspace as a read found it. */
data class CloudWorkspaceProgress(
    val id: WorkspaceId,
    val name: String,
    /** `null` when this one Workspace's read failed; the others still draw. */
    val status: CloudWorkspaceStatus?,
    /** Read in the same pass as [status], and only for a row that was open
     *  when the pass began. `null` for every other row: nobody asked. */
    val waiting: CloudWaiting? = null,
)

// ---------------------------------------------------------------- the rules

/** Why a Workspace is held, before it is put into words. */
sealed interface CloudHold {
    data object PausedByYou : CloudHold
    /** A stable code: a transfer hold or the Cloud permit's error. */
    data class Code(val code: String) : CloudHold
}

/**
 * **Is this Workspace held**, and on what: a person's pause, a code the file
 * transfer is waiting on, or a code the Cloud permit stopped on. A Workspace
 * that is simply not on Cloud is *not* held — the summary's *n of m on Koru
 * Cloud* already says so.
 */
fun holdOf(status: CloudWorkspaceStatus): CloudHold? {
    status.files?.let { files ->
        if (files.paused) return CloudHold.PausedByYou
        if (files.pendingReason.isNotEmpty()) return CloudHold.Code(files.pendingReason)
    }
    return status.sync?.errorCode?.takeIf { it.isNotEmpty() }?.let(CloudHold::Code)
}

/** **The one rule for which rows open**: held, or files parked. */
fun cloudRowOpenable(status: CloudWorkspaceStatus?): Boolean =
    status != null && (holdOf(status) != null || (status.files?.missing ?: 0) > 0)

private fun onCloud(status: CloudWorkspaceStatus) = status.sync?.enabled == true

/** `(on Cloud, required)`, or `null` when no claim may be read — none
 *  computed, or a superseded one, which claims nothing. */
private fun documentsOf(status: CloudWorkspaceStatus): Pair<Int, Int>? {
    val coverage = status.files?.coverage ?: return null
    if (coverage.state == "superseded") return null
    val synced = (coverage.documentsRequired - coverage.documentsMissing).coerceAtLeast(0)
    return synced to coverage.documentsRequired
}

/** Nothing moving and nothing owed: the desktop's `up_to_date`. */
private fun upToDate(files: CloudFilesFacts) =
    files.referenced == files.durable && files.unreadableReferences == 0 &&
        files.uploading == 0 && files.downloading == 0

/** Which way a transfer used. */
enum class CloudPath { Cloud, Peer }

/** One part of a Workspace's line, joined with " · " by the screen. */
sealed interface CloudDetailPart {
    data class Documents(val synced: Int, val required: Int) : CloudDetailPart
    data class Files(val durable: Int, val referenced: Int) : CloudDetailPart
    data class Held(val hold: CloudHold) : CloudDetailPart
    /** On this device the Workspace is not on Koru Cloud. */
    data object SyncOff : CloudDetailPart
    data class Moving(val sending: Boolean, val path: CloudPath?) : CloudDetailPart
    /** On Cloud, nothing measured yet and nothing moving. Always alone. */
    data object SyncOn : CloudDetailPart
    /** The host would not answer about this Workspace. Always alone. */
    data object Unavailable : CloudDetailPart
}

/**
 * One Workspace's line: documents, files, then what is happening. **Every
 * part is omitted when it has no source**, and a settled Workspace adds no
 * state at all.
 */
fun cloudRowDetail(status: CloudWorkspaceStatus?): List<CloudDetailPart> {
    if (status == null) return listOf(CloudDetailPart.Unavailable)
    val parts = mutableListOf<CloudDetailPart>()
    documentsOf(status)?.let { (synced, required) -> parts += CloudDetailPart.Documents(synced, required) }
    status.files?.takeIf { it.referenced > 0 }?.let { parts += CloudDetailPart.Files(it.durable, it.referenced) }
    val state: CloudDetailPart? = holdOf(status)?.let(CloudDetailPart::Held) ?: when {
        !onCloud(status) -> CloudDetailPart.SyncOff
        else -> status.files?.let(::transferTail)
    }
    state?.let { parts += it }
    return parts.ifEmpty { listOf(CloudDetailPart.SyncOn) }
}

/** What the files are doing, after the counts — a hold has already been
 *  said, so this is only the direction and the path of a live transfer. */
private fun transferTail(files: CloudFilesFacts): CloudDetailPart? {
    if (upToDate(files) || files.direction.isEmpty()) return null
    val path = when (files.path) {
        "peer_direct" -> CloudPath.Peer
        "cloud" -> CloudPath.Cloud
        else -> null
    }
    return CloudDetailPart.Moving(sending = files.direction == "upload", path = path)
}

/** *13 of 13 workspaces on Koru Cloud · 369 of 583 files · 1 held*. Files
 *  are summed from the rows' own counts, so the two cannot disagree. */
data class CloudSummary(
    val onCloud: Int,
    val workspaces: Int,
    val durable: Long,
    val referenced: Long,
    val held: Int,
)

fun cloudSummary(rows: List<CloudWorkspaceProgress>): CloudSummary {
    val statuses = rows.mapNotNull { it.status }
    val files = statuses.mapNotNull { it.files }
    return CloudSummary(
        onCloud = statuses.count(::onCloud),
        workspaces = rows.size,
        durable = files.sumOf { it.durable.toLong() },
        referenced = files.sumOf { it.referenced.toLong() },
        held = statuses.count { holdOf(it) != null },
    )
}

/** One line under an open row. */
sealed interface CloudWaitingLine {
    data object Reading : CloudWaitingLine
    data object Unavailable : CloudWaitingLine
    /** Nothing in the Workspace waits on another device. */
    data object None : CloudWaitingLine
    data class Document(val document: CloudWaitingEntry) : CloudWaitingLine
    data class More(val count: Int) : CloudWaitingLine
}

/** At most this many lines under one open row, the last of them *and N more*. */
const val CLOUD_WAITING_CAP = 20

/** One line per document; past [limit], the last line is *and N more*. */
fun cloudWaitingLines(waiting: CloudWaiting, limit: Int = CLOUD_WAITING_CAP): List<CloudWaitingLine> {
    if (limit <= 0) return emptyList()
    val documents = when (waiting) {
        CloudWaiting.Reading -> return listOf(CloudWaitingLine.Reading)
        CloudWaiting.Unavailable -> return listOf(CloudWaitingLine.Unavailable)
        is CloudWaiting.Documents -> waiting.documents
    }
    if (documents.isEmpty()) return listOf(CloudWaitingLine.None)
    if (documents.size <= limit) return documents.map(CloudWaitingLine::Document)
    val shown = limit - 1
    return documents.take(shown).map(CloudWaitingLine::Document) +
        CloudWaitingLine.More(documents.size - shown)
}

// --------------------------------------------------------- what is drawn

data class CloudProgressRow(
    val id: WorkspaceId,
    val name: String,
    val detail: List<CloudDetailPart>,
    /** Whether a tap opens it — [cloudRowOpenable], and nothing else. */
    val openable: Boolean,
    /** What it says under itself; `null` while closed. */
    val waiting: List<CloudWaitingLine>?,
)

sealed interface CloudProgressScreenState {
    /** The Profile has no Cloud sync on this device. [unattached] when this
     *  build carries no Cloud at all, which the Cloud sync row says too. */
    data class Off(val unattached: Boolean = false) : CloudProgressScreenState
    data object Loading : CloudProgressScreenState
    data object Unavailable : CloudProgressScreenState
    data class Ready(val summary: CloudSummary, val rows: List<CloudProgressRow>) : CloudProgressScreenState
}

// ------------------------------------------------------------- the poll

/** Where the presenter reads from. Every call runs off the main thread. */
interface CloudProgressSource {
    /** The Profile's live Workspaces, `(id, name)`; `null` when the host
     *  will not list them. */
    suspend fun workspaces(profile: ProfileId): List<Pair<WorkspaceId, String>>?
    /** `null` when the host would not answer about this one Workspace. */
    suspend fun status(workspace: WorkspaceId): CloudWorkspaceStatus?
    /** `null` when the list could not be read. */
    suspend fun waiting(workspace: WorkspaceId): List<CloudWaitingEntry>?
}

/**
 * **The poll behind the screen**, in the desktop panel's shape: one read at a
 * time, each stamped with the Profile and a generation, every five seconds
 * while [poll] is being run — which the screen does only while it is
 * visible, so a hidden screen costs nothing.
 *
 * An answer about a Profile nobody is looking at any more is dropped rather
 * than drawn under the wrong name. The composable only draws [state].
 */
internal class CloudProgressPresenter(
    private val source: CloudProgressSource,
    private val io: CoroutineDispatcher = Dispatchers.IO,
    private val refreshEveryMillis: Long = REFRESH_EVERY_MILLIS,
) {
    private data class Target(val profile: ProfileId, val enabled: Boolean, val unattached: Boolean)

    private var target: Target? = null
    private var generation = 0L
    private var rows: List<CloudWorkspaceProgress>? = null
    private var unavailable = false
    /** The rows a person opened. Cleared with the Profile. */
    private val open = linkedSetOf<WorkspaceId>()
    private val wake = Channel<Unit>(Channel.CONFLATED)
    private val mutableState = MutableStateFlow<CloudProgressScreenState>(CloudProgressScreenState.Loading)
    val state: StateFlow<CloudProgressScreenState> = mutableState.asStateFlow()

    /**
     * Point the poll at [profile]. [enabled] is whether the Profile is bound
     * to the signed-in account with Cloud sync on; when it is not there is
     * nothing to read, and the screen says so.
     */
    @Synchronized
    fun show(profile: ProfileId, enabled: Boolean, unattached: Boolean = false) {
        val next = Target(profile, enabled, unattached)
        if (next == target) return
        if (next.profile != target?.profile || !enabled) {
            rows = null
            unavailable = false
            open.clear()
        }
        target = next
        generation++
        publish()
        wake.trySend(Unit)
    }

    /**
     * Open or close one row. Opening asks at once rather than at the next
     * tick — a person who tapped is looking — and closing forgets what was
     * read, so opening again never shows an old list as if it were now.
     */
    @Synchronized
    fun toggle(id: WorkspaceId) {
        if (open.remove(id)) {
            rows = rows?.map { if (it.id == id) it.copy(waiting = null) else it }
            publish()
            return
        }
        val row = rows?.firstOrNull { it.id == id } ?: return
        if (!cloudRowOpenable(row.status)) return
        open += id
        publish()
        wake.trySend(Unit)
    }

    /** Read now and every [refreshEveryMillis] after, until cancelled. */
    suspend fun poll() {
        while (true) {
            // A wake raised before this read is answered by it; one raised
            // while it is out stays queued and asks again straight after.
            wake.tryReceive()
            refresh()
            withTimeoutOrNull(refreshEveryMillis) { wake.receive() }
        }
    }

    /** One read, off the main thread, applied only if still current. */
    suspend fun refresh() {
        val (asked, stamp, openAtStart) = synchronized(this) {
            Triple(target, generation, open.toSet())
        }
        if (asked == null || !asked.enabled) return
        val answer = try {
            withContext(io) { read(asked.profile, openAtStart) }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: Exception) {
            null
        }
        apply(asked.profile, stamp, answer)
    }

    private suspend fun read(profile: ProfileId, open: Set<WorkspaceId>): List<CloudWorkspaceProgress>? {
        val workspaces = source.workspaces(profile) ?: return null
        return workspaces.map { (id, name) ->
            val status = source.status(id)
            val waiting = if (id in open && cloudRowOpenable(status)) {
                source.waiting(id)?.let(CloudWaiting::Documents) ?: CloudWaiting.Unavailable
            } else null
            CloudWorkspaceProgress(id, name, status, waiting)
        }
    }

    /** `null` [answer] means the read failed as a whole. */
    @Synchronized
    internal fun apply(profile: ProfileId, stamp: Long, answer: List<CloudWorkspaceProgress>?) {
        if (target?.profile != profile || stamp != generation) return
        if (answer == null) {
            unavailable = true
            publish()
            return
        }
        unavailable = false
        // A row that stopped being able to open closes; one closed while the
        // read was out keeps nothing it read.
        open.retainAll { id -> answer.any { it.id == id && cloudRowOpenable(it.status) } }
        rows = answer.map { if (it.id in open) it else it.copy(waiting = null) }
        publish()
    }

    private fun publish() {
        mutableState.value = screen()
    }

    private fun screen(): CloudProgressScreenState {
        val current = target ?: return CloudProgressScreenState.Loading
        if (!current.enabled) return CloudProgressScreenState.Off(current.unattached)
        val read = rows ?: return if (unavailable) CloudProgressScreenState.Unavailable
        else CloudProgressScreenState.Loading
        return CloudProgressScreenState.Ready(
            summary = cloudSummary(read),
            rows = read.map { row ->
                val openable = cloudRowOpenable(row.status)
                val isOpen = openable && row.id in open
                CloudProgressRow(
                    id = row.id,
                    name = row.name,
                    detail = cloudRowDetail(row.status),
                    openable = openable,
                    waiting = if (isOpen) cloudWaitingLines(row.waiting ?: CloudWaiting.Reading) else null,
                )
            },
        )
    }

    companion object {
        /** The desktop's cadence, for the desktop's reason: nothing announces
         *  that a pass finished, so this asks. */
        const val REFRESH_EVERY_MILLIS = 5_000L
    }
}

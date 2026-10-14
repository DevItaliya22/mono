// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import app.koru.contract.api.v1.AccountAction
import app.koru.contract.api.v1.AccountCommandReply
import app.koru.contract.api.v1.MobileAccountResult
import app.koru.mobile.domain.ProfileId
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/** One Profile's Cloud sync state, exactly as Rust reported it. Ids, booleans,
 * counts and a stable code — the app never invents or rounds any of them. */
data class CloudSyncProfileState(
    val id: ProfileId,
    val enabled: Boolean,
    val workspaces: Int,
    val syncedWorkspaces: Int,
    val error: String? = null,
)

/** What the sheet's row shows. Deliberately a value rather than four booleans
 * read off a state flow at the call site: the rules are the same on every
 * screen that ever shows this row, and they are tested once, here. */
sealed interface CloudSyncDetail {
    /** Off, with nothing further to say. */
    data object Idle : CloudSyncDetail
    /** The command is out and its answer has not arrived. */
    data object Working : CloudSyncDetail
    /** On, and Koru Cloud holds every Workspace this device has. */
    data object Complete : CloudSyncDetail
    /** On, but the Cloud has not accepted all of them yet. */
    data class Partial(val synced: Int, val total: Int) : CloudSyncDetail
    /** The stable refusal code, shown verbatim; never a message. */
    data class Failed(val code: String) : CloudSyncDetail
    /** This build carries no Cloud, so the switch cannot do anything. */
    data object Unattached : CloudSyncDetail
}

data class CloudSyncRow(
    val on: Boolean,
    val interactive: Boolean,
    val detail: CloudSyncDetail,
)

data class CloudSyncState(
    /** Whether this build carries a regional credential policy at all. */
    val attached: Boolean = false,
    val profiles: Map<String, CloudSyncProfileState> = emptyMap(),
    /** Profiles whose switch has been flipped and not yet answered, with the
     *  position that was asked for, so the gesture is visibly taken. */
    val pending: Map<String, Boolean> = emptyMap(),
) {
    /**
     * The row for one Profile, or `null` when it must not be shown at all.
     *
     * Cloud sync is an account feature: without a signed-in account, or for a
     * Profile that account does not own, the switch has nothing to be true
     * about and a disabled one would suggest it could be turned on. A build
     * with no Cloud attached is the opposite case — the row stays, disabled,
     * because "this build has no Cloud" is a fact a person cannot discover
     * anywhere else in the app.
     */
    fun row(profile: ProfileId, boundAccountId: String?, activeAccountId: String?): CloudSyncRow? {
        if (activeAccountId == null || boundAccountId != activeAccountId) return null
        if (!attached) return CloudSyncRow(on = false, interactive = false, detail = CloudSyncDetail.Unattached)
        val requested = pending[profile.value]
        if (requested != null) return CloudSyncRow(on = requested, interactive = false, detail = CloudSyncDetail.Working)
        val known = profiles[profile.value]
            ?: return CloudSyncRow(on = false, interactive = true, detail = CloudSyncDetail.Idle)
        val detail = when {
            known.error != null -> CloudSyncDetail.Failed(known.error)
            !known.enabled -> CloudSyncDetail.Idle
            known.syncedWorkspaces < known.workspaces ->
                CloudSyncDetail.Partial(known.syncedWorkspaces, known.workspaces)
            else -> CloudSyncDetail.Complete
        }
        return CloudSyncRow(on = known.enabled, interactive = true, detail = detail)
    }
}

/**
 * Presentation only, and deliberately separate from [AccountPresentation].
 *
 * That state machine has one `pending` intent for the whole account, because
 * signing in, switching and choosing Profiles genuinely exclude each other.
 * A Cloud sync change excludes nothing: renaming a Profile from the same sheet
 * has to keep working while the switch is in flight, so this keeps its own
 * per-Profile pending set and never touches the account's.
 */
internal class CloudSyncPresentation(
    private val attached: Boolean,
    private val command: suspend (Long, ProfileId?, Boolean?) -> AccountCommandReply,
) {
    private val lock = Mutex()
    private val mutableState = MutableStateFlow(CloudSyncState(attached = attached))
    val state = mutableState.asStateFlow()

    private var observedSequence = 0L
    /** Profile id by the generation whose answer settles it. */
    private var awaiting = mutableMapOf<Long, String>()
    private var requested = mutableMapOf<String, Boolean>()
    private var known = mutableMapOf<String, CloudSyncProfileState>()
    private var inspectedScope: Long? = null
    private var inspectedAccount: String? = null

    private fun publish() {
        mutableState.value = CloudSyncState(attached, known.toMap(), requested.toMap())
    }

    suspend fun observe(status: AndroidAccountStatus, results: List<MobileAccountResult>) = lock.withLock {
        // A signed-out or switched account's answers are not this account's, so
        // a change away from an account already observed drops everything
        // rather than showing one account's choice under another. The *first*
        // observation is not such a change: it has nothing to discard, and must
        // not throw away a switch already in flight.
        if (inspectedAccount != null && status.activeAccountId != inspectedAccount) {
            inspectedScope = null
            known.clear()
            requested.clear()
            awaiting.clear()
        }
        inspectedAccount = status.activeAccountId
        for (result in results) {
            if (result.sequence <= observedSequence) continue
            observedSequence = maxOf(observedSequence, result.sequence)
            when {
                result.kind == "cloud_sync" -> {
                    // The answer is the whole local picture, so it replaces
                    // rather than merges: a Profile that dropped out of the
                    // list no longer has Cloud sync state on this device.
                    known = result.cloudSyncList.associate { row ->
                        row.profileId to CloudSyncProfileState(
                            id = ProfileId(row.profileId),
                            enabled = row.enabled,
                            workspaces = row.workspaces,
                            syncedWorkspaces = row.syncedWorkspaces,
                            error = if (row.hasErrorCode()) row.errorCode else null,
                        )
                    }.toMutableMap()
                    settle(result.generation)
                }
                // A refusal answers the switch without changing what is known:
                // the code belongs to the Profile whose change was refused.
                result.kind == "failed" -> awaiting[result.generation]?.let { profile ->
                    val code = result.errorCode.takeIf { result.hasErrorCode() }
                    if (code != null) {
                        known[profile] = (known[profile] ?: CloudSyncProfileState(ProfileId(profile), false, 0, 0))
                            .copy(error = code)
                    }
                    settle(result.generation)
                }
                else -> settle(result.generation)
            }
        }
        // A result can be lost from the bounded ring. Once the shared worker is
        // idle nothing further is coming, so release the switch rather than
        // leaving it spinning for the rest of the session.
        if (status.requesterActive == false && awaiting.isNotEmpty()) {
            awaiting.keys.toList().forEach { settle(it) }
        }
        if (status.resultsLost) {
            // The ring was reset; what is on screen may be stale. Ask again.
            inspectedScope = null
        }
        publish()
    }

    private fun settle(generation: Long) {
        awaiting.remove(generation)?.let(requested::remove)
    }

    /** Read every local Profile's state once per signed-in account. The answer
     *  also activates Workspaces created since the switch was last touched. */
    suspend fun inspect(status: AndroidAccountStatus) = lock.withLock {
        if (!attached || status.activeAccountId == null || inspectedScope == status.activeScope) return@withLock
        if (status.worker.stage != "signed_in") return@withLock
        inspectedScope = status.activeScope
        try {
            command(status.activeScope, null, null)
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: Exception) {
            // Not a refusal of anything a person asked for; try again next time
            // the account settles rather than reporting a failure at the row.
            inspectedScope = null
        }
        Unit
    }

    suspend fun set(profile: ProfileId, enabled: Boolean, ownerScope: Long) = lock.withLock {
        if (!attached || requested.containsKey(profile.value)) return@withLock
        requested[profile.value] = enabled
        // Published before the call so the gesture is taken immediately; the
        // answer's `enabled` replaces this, it does not confirm it.
        publish()
        try {
            val reply = command(ownerScope, profile, enabled)
            if (reply.hasErrorCode()) {
                known[profile.value] = (known[profile.value] ?: CloudSyncProfileState(profile, false, 0, 0))
                    .copy(error = reply.errorCode)
                requested.remove(profile.value)
            } else {
                awaiting[reply.generation] = profile.value
            }
        } catch (cancelled: CancellationException) {
            requested.remove(profile.value)
            throw cancelled
        } catch (_: Exception) {
            // The JNI acknowledgement can be lost after the command reached the
            // shared worker. Releasing the switch is right; repeating it is not.
            requested.remove(profile.value)
            known[profile.value] = (known[profile.value] ?: CloudSyncProfileState(profile, false, 0, 0))
                .copy(error = "account_command_unavailable")
        }
        publish()
    }
}

internal fun cloudSyncAction(profile: ProfileId?): AccountAction =
    if (profile == null) AccountAction.ACCOUNT_ACTION_INSPECT_CLOUD_SYNC
    else AccountAction.ACCOUNT_ACTION_SET_PROFILE_CLOUD_SYNC

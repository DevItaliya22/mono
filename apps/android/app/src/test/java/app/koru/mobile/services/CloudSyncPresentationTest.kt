// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import app.koru.contract.api.v1.AccountAction
import app.koru.contract.api.v1.AccountCommandReply
import app.koru.contract.api.v1.AccountCommandRequest
import app.koru.contract.api.v1.MobileAccountResult
import app.koru.contract.api.v1.MobileCloudSyncProfile
import app.koru.mobile.domain.ProfileId
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

private const val ACCOUNT = "11111111-1111-4111-8111-111111111111"
private const val PROFILE = "22222222-2222-4222-8222-222222222222"
private const val OTHER = "33333333-3333-4333-8333-333333333333"

class CloudSyncPresentationTest {
    private fun status(
        accountId: String? = ACCOUNT,
        stage: String = "signed_in",
        requesterActive: Boolean? = true,
        resultsLost: Boolean = false,
    ) = AndroidAccountStatus(
        worker = AccountDiagnostics(stage = stage),
        activeAccountId = accountId,
        activeScope = 1,
        requesterActive = requesterActive,
        resultsLost = resultsLost,
    )

    private fun report(
        sequence: Long,
        generation: Long,
        vararg rows: MobileCloudSyncProfile,
    ): MobileAccountResult = MobileAccountResult.newBuilder()
        .setSequence(sequence).setGeneration(generation).setKind("cloud_sync")
        .addAllCloudSync(rows.toList()).build()

    private fun row(profile: String, enabled: Boolean, workspaces: Int, synced: Int, error: String? = null) =
        MobileCloudSyncProfile.newBuilder()
            .setProfileId(profile).setEnabled(enabled)
            .setWorkspaces(workspaces).setSyncedWorkspaces(synced)
            .apply { error?.let(::setErrorCode) }.build()

    private class Recorder {
        val sent = mutableListOf<Triple<Long, ProfileId?, Boolean?>>()
        var next: Long = 10
        var fail: String? = null
        var throws = false
        suspend fun handle(owner: Long, profile: ProfileId?, enabled: Boolean?): AccountCommandReply {
            sent += Triple(owner, profile, enabled)
            if (throws) throw IllegalStateException("jni")
            fail?.let { return AccountCommandReply.newBuilder().setErrorCode(it).build() }
            return AccountCommandReply.newBuilder().setGeneration(next++).build()
        }
    }

    private fun presentation(recorder: Recorder, attached: Boolean = true) =
        CloudSyncPresentation(attached) { owner, profile, enabled -> recorder.handle(owner, profile, enabled) }

    @Test fun aProfileOutsideTheSignedInAccountHasNoRowAtAll() {
        val state = CloudSyncState(attached = true)
        // Signed out: nothing about Cloud sync can be true yet.
        assertNull(state.row(ProfileId(PROFILE), ACCOUNT, null))
        // A local Profile no account owns is not a candidate for the switch.
        assertNull(state.row(ProfileId(PROFILE), null, ACCOUNT))
        // And a Profile belonging to some other account is never answered here.
        assertNull(state.row(ProfileId(PROFILE), OTHER, ACCOUNT))
        assertTrue(state.row(ProfileId(PROFILE), ACCOUNT, ACCOUNT) != null)
    }

    @Test fun aBuildWithNoCloudShowsTheRowDisabledRatherThanHidingWhyItCannot() {
        val row = CloudSyncState(attached = false).row(ProfileId(PROFILE), ACCOUNT, ACCOUNT)!!
        assertFalse(row.interactive)
        assertFalse(row.on)
        assertEquals(CloudSyncDetail.Unattached, row.detail)
    }

    @Test fun theDetailNamesWhatTheCloudIsActuallyDoingForThisProfile() {
        fun detail(enabled: Boolean, workspaces: Int, synced: Int, error: String? = null) =
            CloudSyncState(
                attached = true,
                profiles = mapOf(PROFILE to CloudSyncProfileState(ProfileId(PROFILE), enabled, workspaces, synced, error)),
            ).row(ProfileId(PROFILE), ACCOUNT, ACCOUNT)!!.detail

        assertEquals(CloudSyncDetail.Idle, detail(enabled = false, workspaces = 3, synced = 0))
        assertEquals(CloudSyncDetail.Complete, detail(enabled = true, workspaces = 3, synced = 3))
        assertEquals(CloudSyncDetail.Partial(1, 3), detail(enabled = true, workspaces = 3, synced = 1))
        // A refusal outranks the counts: it is the thing that needs reading.
        assertEquals(
            CloudSyncDetail.Failed("workspace_version_mismatch"),
            detail(enabled = true, workspaces = 3, synced = 3, error = "workspace_version_mismatch"),
        )
    }

    @Test fun flippingTheSwitchTakesTheGestureAndTheAnswerIsTheTruth() = runBlocking {
        val recorder = Recorder()
        val cloud = presentation(recorder)
        cloud.set(ProfileId(PROFILE), enabled = true, ownerScope = 1)

        // Pending: the requested position is visible, and it cannot be flipped
        // again until the answer settles it.
        val pending = cloud.state.value.row(ProfileId(PROFILE), ACCOUNT, ACCOUNT)!!
        assertTrue(pending.on)
        assertFalse(pending.interactive)
        assertEquals(CloudSyncDetail.Working, pending.detail)
        assertEquals(listOf(Triple(1L, ProfileId(PROFILE), true)), recorder.sent)

        // The handler could only apply it to two of the three Workspaces, and
        // that is what the row now says — not the optimistic "on, done".
        cloud.observe(status(), listOf(report(1, 10, row(PROFILE, enabled = true, workspaces = 3, synced = 2))))
        val settled = cloud.state.value.row(ProfileId(PROFILE), ACCOUNT, ACCOUNT)!!
        assertTrue(settled.on)
        assertTrue(settled.interactive)
        assertEquals(CloudSyncDetail.Partial(2, 3), settled.detail)
    }

    @Test fun anAnswerThatContradictsTheRequestedPositionWins() = runBlocking {
        val recorder = Recorder()
        val cloud = presentation(recorder)
        cloud.set(ProfileId(PROFILE), enabled = true, ownerScope = 1)
        assertTrue(cloud.state.value.row(ProfileId(PROFILE), ACCOUNT, ACCOUNT)!!.on)
        cloud.observe(status(), listOf(report(1, 10, row(PROFILE, enabled = false, workspaces = 1, synced = 0))))
        assertFalse(cloud.state.value.row(ProfileId(PROFILE), ACCOUNT, ACCOUNT)!!.on)
    }

    @Test fun aRefusalShowsItsStableCodeAndReleasesTheSwitch() = runBlocking {
        val recorder = Recorder()
        val cloud = presentation(recorder)
        cloud.set(ProfileId(PROFILE), enabled = true, ownerScope = 1)
        val failure = MobileAccountResult.newBuilder()
            .setSequence(1).setGeneration(10).setKind("failed")
            .setErrorCode("cloud_sync_unavailable").build()
        cloud.observe(status(), listOf(failure))
        val row = cloud.state.value.row(ProfileId(PROFILE), ACCOUNT, ACCOUNT)!!
        assertTrue("the switch is usable again", row.interactive)
        assertEquals(CloudSyncDetail.Failed("cloud_sync_unavailable"), row.detail)
    }

    @Test fun aRejectedCommandNeverLeavesTheSwitchSpinning() = runBlocking {
        val recorder = Recorder().apply { fail = "account_queue_full" }
        val cloud = presentation(recorder)
        cloud.set(ProfileId(PROFILE), enabled = true, ownerScope = 1)
        val row = cloud.state.value.row(ProfileId(PROFILE), ACCOUNT, ACCOUNT)!!
        assertTrue(row.interactive)
        assertEquals(CloudSyncDetail.Failed("account_queue_full"), row.detail)
    }

    @Test fun aLostAcknowledgementReleasesTheSwitchWithoutRepeatingTheCommand() = runBlocking {
        val recorder = Recorder().apply { throws = true }
        val cloud = presentation(recorder)
        cloud.set(ProfileId(PROFILE), enabled = true, ownerScope = 1)
        assertEquals(1, recorder.sent.size)
        val row = cloud.state.value.row(ProfileId(PROFILE), ACCOUNT, ACCOUNT)!!
        assertTrue(row.interactive)
        assertEquals(CloudSyncDetail.Failed("account_command_unavailable"), row.detail)
    }

    @Test fun anIdleWorkerWithNoResultReleasesTheSwitchRatherThanHoldingItForever() = runBlocking {
        val recorder = Recorder()
        val cloud = presentation(recorder)
        cloud.set(ProfileId(PROFILE), enabled = true, ownerScope = 1)
        assertFalse(cloud.state.value.row(ProfileId(PROFILE), ACCOUNT, ACCOUNT)!!.interactive)
        // The result fell out of the bounded ring; the worker is idle, so
        // nothing further is coming for this generation.
        cloud.observe(status(requesterActive = false), emptyList())
        assertTrue(cloud.state.value.row(ProfileId(PROFILE), ACCOUNT, ACCOUNT)!!.interactive)
    }

    @Test fun signingOutOrSwitchingAccountsDiscardsTheOtherAccountsChoices() = runBlocking {
        val recorder = Recorder()
        val cloud = presentation(recorder)
        cloud.observe(status(), listOf(report(1, 10, row(PROFILE, enabled = true, workspaces = 1, synced = 1))))
        assertEquals(CloudSyncDetail.Complete, cloud.state.value.row(ProfileId(PROFILE), ACCOUNT, ACCOUNT)!!.detail)
        cloud.observe(status(accountId = null, stage = "signed_out"), emptyList())
        assertTrue(cloud.state.value.profiles.isEmpty())
    }

    @Test fun readingEveryProfilesStateHappensOncePerSignedInAccount() = runBlocking {
        val recorder = Recorder()
        val cloud = presentation(recorder)
        cloud.inspect(status(stage = "restoring"))
        assertTrue("a worker that is not signed in is not asked", recorder.sent.isEmpty())
        cloud.inspect(status())
        cloud.inspect(status())
        assertEquals(listOf(Triple(1L, null, null)), recorder.sent)
    }

    @Test fun aBuildWithNoCloudNeitherAsksNorAccepts() = runBlocking {
        val recorder = Recorder()
        val cloud = presentation(recorder, attached = false)
        cloud.inspect(status())
        cloud.set(ProfileId(PROFILE), enabled = true, ownerScope = 1)
        assertTrue(recorder.sent.isEmpty())
    }

    @Test fun theActionMatchesWhatTheCommandIsFor() {
        assertEquals(AccountAction.ACCOUNT_ACTION_INSPECT_CLOUD_SYNC, cloudSyncAction(null))
        assertEquals(AccountAction.ACCOUNT_ACTION_SET_PROFILE_CLOUD_SYNC, cloudSyncAction(ProfileId(PROFILE)))
    }

    @Test fun theContractRefusesASwitchWithNoTargetOrNoPosition() {
        val base = AccountCommandRequest.newBuilder().setVersion(1).setRequester(2)
            .setExpectedOwnerScope(1).setAction(AccountAction.ACCOUNT_ACTION_SET_PROFILE_CLOUD_SYNC)
        // Both halves are required; neither is guessed.
        assertThrows(IllegalArgumentException::class.java) {
            MobileAccountContract.validate(base.clone().setCloudSyncEnabled(true).build())
        }
        assertThrows(IllegalArgumentException::class.java) {
            MobileAccountContract.validate(base.clone().setProfileId(PROFILE).build())
        }
        MobileAccountContract.validate(base.clone().setProfileId(PROFILE).setCloudSyncEnabled(false).build())

        // And the position never rides along on an unrelated command.
        assertThrows(IllegalArgumentException::class.java) {
            MobileAccountContract.validate(
                AccountCommandRequest.newBuilder().setVersion(1).setRequester(2).setExpectedOwnerScope(1)
                    .setAction(AccountAction.ACCOUNT_ACTION_LIST_PROFILES).setCloudSyncEnabled(true).build(),
            )
        }
        assertThrows(IllegalArgumentException::class.java) {
            MobileAccountContract.validate(
                base.clone().setProfileId("not-a-profile").setCloudSyncEnabled(true).build(),
            )
        }
    }

    @Test fun readingEveryProfilesStateCarriesNoTarget() {
        MobileAccountContract.validate(
            AccountCommandRequest.newBuilder().setVersion(1).setRequester(2).setExpectedOwnerScope(1)
                .setAction(AccountAction.ACCOUNT_ACTION_INSPECT_CLOUD_SYNC).build(),
        )
    }
}

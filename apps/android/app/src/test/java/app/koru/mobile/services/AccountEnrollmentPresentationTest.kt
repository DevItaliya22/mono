// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import app.koru.contract.api.v1.*
import app.koru.mobile.domain.ProfileId
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.runCurrent
import org.junit.Assert.*
import org.junit.Test

@OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
class AccountEnrollmentPresentationTest {
    private val a = "10000000-0000-4000-8000-000000000001"
    private val b = "10000000-0000-4000-8000-000000000002"
    private fun row(id: String, stage: String, failure: String? = null, retry: Long? = null) =
        MobileProfileEnrollment.newBuilder().setProfileId(id).setStage(stage)
            .apply { failure?.let(::setFailureCode); retry?.let(::setRetryAtMillis) }.build()

    @Test fun diagnosticsPreserveEveryIndependentFailureAndDeadline() {
        val value = MobileAccountStatus.newBuilder().setStage("signed_in")
            .addEnrollments(row(a, "failed", "no_peer", 123))
            .addEnrollments(row(b, "complete")).build()
        val result = AccountDiagnostics.from(value)
        assertEquals(listOf(a, b), result.enrollments.map { it.profileId })
        assertEquals("no_peer", result.enrollments[0].failure)
        assertEquals(123L, result.enrollments[0].retryAtMillis)
        assertNull(result.enrollments[1].failure)
        assertNull(result.enrollments[1].retryAtMillis)
    }

    @Test fun singletonChoiceAndMemberRetryKeepScopeAndSiblingProgress() = runTest {
        val calls = mutableListOf<Triple<Long, AccountIntent, String>>()
        val presentation = AccountPresentation { owner, intent, _, _, target, profiles ->
            calls += Triple(owner, intent, profiles.singleOrNull() ?: target)
            AccountCommandReply.newBuilder().setOwnerScope(owner).setGeneration(11).build()
        }
        val worker = AccountDiagnostics(stage = "signed_in", enrollments = listOf(
            ProfileEnrollmentDiagnostics(a, "failed", "no_peer", 123), ProfileEnrollmentDiagnostics(b, "complete")))
        val status = AndroidAccountStatus(worker = worker, capability = true, generation = 10, activeScope = 3, requesterActive = false)
        val profiles = MobileAccountResult.newBuilder().setOwnerScope(3).setGeneration(10).setSequence(10).setKind("profiles")
            .setAccount(MobileAccountSummary.newBuilder().setDisplayName("Account"))
            .addProfiles(MobileAccountProfile.newBuilder().setProfileId(a).setName("A"))
            .addProfiles(MobileAccountProfile.newBuilder().setProfileId(b).setName("B")).build()
        presentation.observe(status, listOf(profiles))
        presentation.act(AccountIntent.RetryEnrollment, profileId = ProfileId(b))
        assertTrue(calls.isEmpty())
        presentation.act(AccountIntent.RetryEnrollment, profileId = ProfileId(a), expectedOwnerScope = 3)
        assertEquals(Triple(3L, AccountIntent.RetryEnrollment, a), calls.single())
        assertEquals(ProfileEnrollment.Complete, presentation.state.value.enrollments[1].state)
        val completed = MobileAccountResult.newBuilder().setOwnerScope(3).setGeneration(11).setSequence(11).setKind("completed").build()
        presentation.observe(status.copy(generation = 11), listOf(completed,
            profiles.toBuilder().setGeneration(11).setSequence(12).build()))
        presentation.act(AccountIntent.SelectProfiles, profileIds = listOf(ProfileId(a)), expectedOwnerScope = 3)
        assertEquals(Triple(3L, AccountIntent.SelectProfiles, a), calls.last())
    }

    @Test fun relinkIsExplicitProfileEnrollmentAndNeverStartsProviderSignIn() = runTest {
        val calls = mutableListOf<Triple<Long, AccountIntent, List<String>>>()
        val presentation = AccountPresentation { owner, intent, _, _, _, profiles ->
            calls += Triple(owner, intent, profiles)
            AccountCommandReply.newBuilder().setOwnerScope(owner).setGeneration(11).build()
        }
        val worker = AccountDiagnostics(stage = "signed_in", enrollments = listOf(
            ProfileEnrollmentDiagnostics(a, "complete"),
            ProfileEnrollmentDiagnostics(b, "complete"),
        ))
        val status = AndroidAccountStatus(
            worker = worker,
            capability = true,
            generation = 10,
            activeScope = 3,
            requesterActive = false,
        )
        val profiles = MobileAccountResult.newBuilder()
            .setOwnerScope(3).setGeneration(10).setSequence(10).setKind("profiles")
            .setAccount(MobileAccountSummary.newBuilder().setDisplayName("Account"))
            .addProfiles(MobileAccountProfile.newBuilder().setProfileId(a).setName("A"))
            .addProfiles(MobileAccountProfile.newBuilder().setProfileId(b).setName("B"))
            .build()
        presentation.observe(status, listOf(profiles))

        assertTrue(presentation.state.value.canRelinkDevices)
        assertFalse(presentation.state.value.relinkWaiting)
        assertTrue(AccountScreenState(
            enrollments = listOf(ProfileEnrollmentProgress(ProfileId(a), ProfileEnrollment.Waiting)),
        ).relinkWaiting)
        presentation.act(
            AccountIntent.RelinkDevices,
            profileIds = listOf(ProfileId(b), ProfileId(a)),
            expectedOwnerScope = 3,
        )

        assertEquals(listOf(Triple(3L, AccountIntent.RelinkDevices, listOf(a, b))), calls)
        assertEquals(AccountAction.ACCOUNT_ACTION_SELECT_PROFILES, AccountIntent.RelinkDevices.wire())
        assertEquals(AccountIntent.RelinkDevices, presentation.state.value.pending)
        assertTrue(presentation.state.value.relinkWaiting)
        assertFalse(presentation.state.value.canRelinkDevices)
        assertFalse(calls.any { it.second == AccountIntent.SignIn })
    }

    private fun signedIn(rows: List<ProfileEnrollmentDiagnostics> = emptyList()) = AndroidAccountStatus(
        worker = AccountDiagnostics(stage = "signed_in", enrollments = rows), capability = true,
        generation = 10, activeScope = 3, requesterActive = false,
    )

    private fun choices() = MobileAccountResult.newBuilder()
        .setOwnerScope(3).setGeneration(10).setSequence(10).setKind("profiles")
        .setAccount(MobileAccountSummary.newBuilder().setDisplayName("Account"))
        .addProfiles(MobileAccountProfile.newBuilder().setProfileId(a).setName("A")).build()

    @Test fun firstTapPublishesBeforeBlockedPollAndAdmissionAndRejectsRepeatedTaps() = runTest {
        val admission = CompletableDeferred<Unit>()
        var commands = 0
        val presentation = AccountPresentation { owner, intent, _, _, _, _ ->
            assertEquals(AccountIntent.RelinkDevices, intent)
            commands++
            admission.await()
            AccountCommandReply.newBuilder().setOwnerScope(owner).setGeneration(11).build()
        }
        presentation.observe(signedIn(), listOf(choices()))
        val poll = CompletableDeferred<Unit>()
        val cycle = AccountPresentationCycle(presentation) {
            poll.await()
            AccountObservation(signedIn(), emptyList())
        }
        val polling = launch { cycle.tick() }
        runCurrent()
        assertTrue(presentation.queueRelink(3))
        assertTrue(presentation.state.value.relinkWaiting)
        assertFalse(presentation.state.value.canRelinkDevices)
        assertFalse(presentation.queueRelink(3))
        val action = launch {
            try { cycle.act(AccountIntent.RelinkDevices, "", null, expectedOwnerScope = 3, profiles = listOf(ProfileId(a))) }
            finally { presentation.releaseQueuedRelink(3) }
        }
        runCurrent()
        assertEquals(0, commands)
        poll.complete(Unit)
        polling.join()
        runCurrent()
        assertEquals(1, commands)
        assertTrue(presentation.state.value.relinkWaiting)
        assertFalse(presentation.queueRelink(3))
        admission.complete(Unit)
        action.join()
        assertTrue(presentation.state.value.relinkWaiting)
        presentation.act(AccountIntent.RelinkDevices, profileIds = listOf(ProfileId(a)))
        assertEquals(1, commands)
    }

    @Test fun recreatedPresentationUsesDurableWaitingAndScheduledRetryUntilCompletion() = runTest {
        val progress = listOf(
            ProfileEnrollmentDiagnostics(a, "enrolling"),
            ProfileEnrollmentDiagnostics(a, "waiting", "no_peer_online", 600_001),
            ProfileEnrollmentDiagnostics(a, "waiting", "enrollment_unavailable", 1_200_001),
            ProfileEnrollmentDiagnostics(a, "failed", "enrollment_unavailable", 1_200_001),
            ProfileEnrollmentDiagnostics(a, "complete", "provisioning_pending", 1_200_001),
        )
        for (row in progress) {
            val presentation = AccountPresentation { _, _, _, _, _, _ -> error("must not resubmit restored work") }
            presentation.observe(signedIn(listOf(row)), listOf(choices()))
            assertTrue(presentation.state.value.relinkWaiting)
            assertFalse(presentation.queueRelink(3))
            presentation.act(AccountIntent.RelinkDevices, profileIds = listOf(ProfileId(a)))
            presentation.foregrounded()
            assertTrue(presentation.state.value.relinkWaiting)
            presentation.observe(signedIn(listOf(ProfileEnrollmentDiagnostics(a, "complete"))), listOf(
                choices().toBuilder().setSequence(11).build(),
            ))
            assertFalse(presentation.state.value.relinkWaiting)
            assertTrue(presentation.state.value.canRelinkDevices)
        }
    }

    @Test fun terminalFailureIsHonestAndQueueFailureDoesNotStrandWaiting() = runTest {
        val presentation = AccountPresentation { _, _, _, _, _, _ -> error("no command expected") }
        presentation.observe(signedIn(listOf(ProfileEnrollmentDiagnostics(a, "failed", "permission_denied"))), listOf(choices()))
        assertFalse(presentation.state.value.relinkWaiting)
        assertEquals("permission_denied", presentation.state.value.enrollments.single().failure)
        assertTrue(presentation.queueRelink(3))
        presentation.releaseQueuedRelink(3)
        assertFalse(presentation.state.value.relinkWaiting)
        assertTrue(presentation.state.value.canRelinkDevices)
        assertFalse(presentation.queueRelink(4))
    }

    @Test fun relinkRowFollowsDurableNeedAndDisappearsAfterCompletion() {
        assertFalse(AccountScreenState(phase = AccountPhase.SignedOut)
            .showRelinkDevices(true))
        val loaded = AccountScreenState(phase = AccountPhase.SignedIn, capable = true, profilesLoaded = true,
            profiles = listOf(AuthorizedProfile(ProfileId(a), "A", true, true)))
        assertTrue(loaded.showRelinkDevices(true))
        assertFalse(loaded.relinkLoading(true))
        assertTrue(loaded.canRelinkDevices(true))
        assertTrue(loaded.showRelinkDevices(null))
        assertTrue(loaded.relinkLoading(null))
        assertFalse(loaded.canRelinkDevices(null))
        assertFalse(loaded.showRelinkDevices(false))
        assertFalse(loaded.canRelinkDevices(false))

        val loading = loaded.copy(phase = AccountPhase.Refreshing, profilesLoaded = false)
        assertTrue(loading.showRelinkDevices(true))
        assertTrue(loading.relinkLoading(true))
        assertFalse(loading.canRelinkDevices(true))

        val waiting = AccountScreenState(
            phase = AccountPhase.Restoring,
            enrollments = listOf(ProfileEnrollmentProgress(ProfileId(a), ProfileEnrollment.Waiting)),
        )
        assertTrue(waiting.showRelinkDevices(false))
        assertFalse(waiting.relinkLoading(false))
        val failed = loaded.copy(enrollments = listOf(
            ProfileEnrollmentProgress(ProfileId(a), ProfileEnrollment.Failed, "permission_denied"),
        ))
        assertTrue(failed.showRelinkDevices(false))
        assertFalse(failed.canRelinkDevices(false))
    }

    @Test fun lateSelectionAcknowledgementCannotReplaceNewerActiveOrOtherAccountProgress() = runTest {
        val presentation = AccountPresentation { _, _, _, _, _, profiles -> error("no command expected") }
        fun status(scope: Long, rows: List<ProfileEnrollmentDiagnostics>) = AndroidAccountStatus(
            worker = AccountDiagnostics(stage = "awaiting_account_creation", enrollments = rows), capability = true,
            generation = 10, activeScope = scope, requesterActive = false,
            heldAccounts = listOf(HeldAccount(1, a, null, "awaiting_account_creation", true, null, emptyList(), false),
                HeldAccount(2, b, null, "awaiting_account_creation", true, null, emptyList(), false)))
        presentation.observe(status(1, listOf(ProfileEnrollmentDiagnostics(a, "waiting"))), emptyList())
        val selectedB = status(2, listOf(ProfileEnrollmentDiagnostics(b, "complete")))
        presentation.observe(selectedB, emptyList())
        val lateA = MobileAccountResult.newBuilder().setOwnerScope(1).setGeneration(10).setSequence(11)
            .setKind("profiles_selected").addEnrollments(row(a, "failed", "no_peer", 90)).build()
        val lateB = MobileAccountResult.newBuilder().setOwnerScope(2).setGeneration(10).setSequence(12)
            .setKind("profiles_selected").addEnrollments(row(b, "waiting")).build()
        presentation.observe(selectedB, listOf(lateA, lateB))
        assertEquals(2L, presentation.state.value.activeScope)
        assertEquals(listOf(ProfileId(b)), presentation.state.value.enrollments.map { it.id })
        assertEquals(ProfileEnrollment.Complete, presentation.state.value.enrollments.single().state)
    }
}

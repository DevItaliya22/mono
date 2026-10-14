// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import app.koru.contract.api.v1.*
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.launch
import org.junit.Assert.*
import org.junit.Test

@OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
class AccountScopePresentationTest {
    private val a = "11111111-1111-4111-8111-111111111111"
    private val b = "22222222-2222-4222-8222-222222222222"
    private fun row(scope:Long, active:List<Long> = emptyList()) = HeldAccount(scope,if(scope==1L)a else b,null,"awaiting_account_creation",true,null,active,active.isNotEmpty())
    private fun status(scope:Long, aActive:List<Long> = emptyList(), bActive:List<Long> = emptyList()) = AndroidAccountStatus(
        worker=AccountDiagnostics(stage="awaiting_account_creation"), generation=10, capability=true,
        activeScope=scope, activeAccountId=if(scope==1L)a else b,
        heldAccounts=listOf(row(1,aActive),row(2,bActive)), requesterActive=(if(scope==1L)aActive else bActive).isNotEmpty())
    private fun reply(scope:Long,generation:Long) = AccountCommandReply.newBuilder().setOwnerScope(scope).setGeneration(generation).build()
    private fun result(scope:Long,generation:Long,kind:String="cancelled") = MobileAccountResult.newBuilder()
        .setSequence(generation).setGeneration(generation).setOwnerScope(scope).setKind(kind)

    @Test fun rejectedBeforeAdmissionSwitchReleasesOnlyTransitionAndKeepsAIntent() = runTest {
        val calls = mutableListOf<Pair<Long,AccountIntent>>()
        val presentation = AccountPresentation { owner,intent,_,_,_, profiles ->
            calls += owner to intent
            if(intent==AccountIntent.SwitchAccount) throw IllegalStateException("lost transport")
            reply(owner,10)
        }
        presentation.observe(status(1),emptyList())
        presentation.act(AccountIntent.ConfirmCreation)
        presentation.act(AccountIntent.SwitchAccount,accountId=b)
        assertTrue(presentation.state.value.switchPending)
        presentation.observe(status(1,aActive=listOf(10)),emptyList())
        assertFalse(presentation.state.value.switchPending)
        assertEquals(AccountIntent.ConfirmCreation,presentation.state.value.pending)
        assertEquals(1L,presentation.state.value.activeScope)
        assertEquals(listOf(1L to AccountIntent.ConfirmCreation,1L to AccountIntent.SwitchAccount),calls)
    }

    @Test fun admittedSwitchFailureReturnsFeedbackWithoutSettlingACommand() = runTest {
        val presentation = AccountPresentation { owner,intent,_,_,_, profiles -> reply(if(intent==AccountIntent.SwitchAccount)2 else owner,if(intent==AccountIntent.SwitchAccount)11 else 10) }
        presentation.observe(status(1),emptyList())
        presentation.act(AccountIntent.ConfirmCreation)
        presentation.act(AccountIntent.SwitchAccount,accountId=b)
        presentation.observe(status(1,aActive=listOf(10)),listOf(result(2,11,"failed").setErrorCode("account_credential_missing").build()))
        assertFalse(presentation.state.value.switchPending)
        assertEquals(AccountIntent.ConfirmCreation,presentation.state.value.pending)
        assertEquals("account_credential_missing",presentation.state.value.failure)
        assertEquals(1L,presentation.state.value.activeScope)
    }

    @Test fun parkedACompletionNeverEmitsAWhileBIsSelectedAndItsIntentSettles() = runTest {
        val presentation = AccountPresentation { owner,intent,_,_,_, profiles -> reply(if(intent==AccountIntent.SwitchAccount)2 else owner,if(intent==AccountIntent.SwitchAccount)11 else 10) }
        presentation.observe(status(1),emptyList())
        presentation.act(AccountIntent.ConfirmCreation)
        presentation.act(AccountIntent.SwitchAccount,accountId=b)
        presentation.observe(status(2,aActive=listOf(10)),emptyList())
        val emissions = mutableListOf<Long>()
        val collector=backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) { presentation.state.collect { emissions += it.activeScope } }
        presentation.observe(status(2),listOf(result(1,10).build()))
        assertTrue(emissions.all { it==2L })
        assertNull(presentation.state.value.pending)
        presentation.observe(status(1),emptyList())
        assertNull(presentation.state.value.pending)
        collector.cancel()
    }

    @Test fun lostAddReplyResumingSameAnonymousScopeSettlesFromOwnedSelection() = runTest {
        var calls=0
        val presentation=AccountPresentation { _,_,_,_,_, profiles -> calls++; throw IllegalStateException("lost ACK") }
        val anonymous=status(1).copy(activeAccountId=null,heldAccounts=emptyList(),requesterActive=true)
        presentation.observe(anonymous,emptyList())
        presentation.act(AccountIntent.AddAccount)
        presentation.observe(anonymous,listOf(result(1,11,"account_selected").setSelectedScope(1).build()))
        assertFalse(presentation.state.value.switchPending)
        assertEquals(AccountPhase.AwaitingCreation,presentation.state.value.phase)
        assertEquals(1,calls)
    }

    @Test fun scopePresenceIsRequiredAndZeroIsAnExplicitScope() {
        val request=AccountCommandRequest.newBuilder().setVersion(1).setRequester(1).setAction(AccountAction.ACCOUNT_ACTION_RETRY)
        assertThrows(IllegalArgumentException::class.java) { MobileAccountContract.validate(request.build()) }
        MobileAccountContract.validate(request.setExpectedOwnerScope(0).build())
    }
    @Test fun queuedTapKeepsRenderedOwnerWhenAnotherAccountIsSelectedBeforeAcquisition() = runTest {
        val calls=mutableListOf<Pair<Long,AccountIntent>>()
        val presentation=AccountPresentation { owner,intent,_,_,_, profiles -> calls += owner to intent; reply(owner,10) }
        presentation.observe(status(1),emptyList())
        val renderedScope=presentation.state.value.activeScope
        presentation.observe(status(2),emptyList())
        val cycle=AccountPresentationCycle(presentation) { AccountObservation(status(2),emptyList()) }
        cycle.act(AccountIntent.ConfirmCreation,"en",null,expectedOwnerScope=renderedScope)
        assertTrue(calls.isEmpty())
        assertEquals(2L,presentation.state.value.activeScope)
        assertTrue(presentation.state.value.canDecideCreation)
        assertNull(presentation.state.value.failure)
        cycle.act(AccountIntent.ConfirmCreation,"en",null,expectedOwnerScope=2)
        assertEquals(listOf(2L to AccountIntent.ConfirmCreation),calls)
    }
    @Test fun unknownRemovalOwnerDoesNotOfferRetryOrForceActiveAccountSignIn() {
        val signedIn=AccountScreenState(phase=AccountPhase.SignedIn,capable=true,displayName="A",failure="profile_lifecycle_owner_unproven")
        assertTrue(signedIn.signedIn)
        assertFalse(signedIn.canSignIn)
        assertFalse(signedIn.canRetry)
        assertFalse(signedIn.copy(phase=AccountPhase.Failed).canRetry)
    }

    @Test fun lostAddAdmissionAllowsOnlyExplicitIdempotentAddRetryWhileAWaitsForChoice() = runTest {
        val calls=mutableListOf<AccountIntent>()
        val presentation=AccountPresentation { owner,intent,_,_,_, profiles ->
            calls += intent
            if(intent==AccountIntent.ConfirmCreation) reply(owner,10)
            else if(calls.count { it==AccountIntent.AddAccount }==1) throw IllegalStateException("lost Add ACK")
            else reply(3,12)
        }
        presentation.observe(status(1),emptyList())
        presentation.act(AccountIntent.ConfirmCreation)
        presentation.act(AccountIntent.AddAccount)
        presentation.observe(status(1,aActive=listOf(10)),emptyList())
        assertTrue(presentation.state.value.canAddAccount)
        assertFalse(presentation.state.value.canSwitchAccounts)
        assertEquals(AccountIntent.ConfirmCreation,presentation.state.value.pending)
        presentation.act(AccountIntent.AddAccount)
        presentation.observe(status(1,aActive=listOf(10)).copy(activeScope=3,activeAccountId=null),emptyList())
        assertFalse(presentation.state.value.switchPending)
        assertNull(presentation.state.value.pending)
        presentation.observe(status(1,aActive=listOf(10)),emptyList())
        assertEquals(AccountIntent.ConfirmCreation,presentation.state.value.pending)
        assertEquals(listOf(AccountIntent.ConfirmCreation,AccountIntent.AddAccount,AccountIntent.AddAccount),calls)
    }
}

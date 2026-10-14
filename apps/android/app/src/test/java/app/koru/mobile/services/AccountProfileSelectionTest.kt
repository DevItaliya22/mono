// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import app.koru.contract.api.v1.*
import app.koru.mobile.domain.ProfileId
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test

class AccountProfileSelectionTest {
    private val a = ProfileId("10000000-0000-4000-8000-000000000001")
    private val b = ProfileId("20000000-0000-4000-8000-000000000002")
    private val c = ProfileId("30000000-0000-4000-8000-000000000003")
    private data class Call(val scope: Long, val intent: AccountIntent, val profiles: List<String>)
    private class Port {
        val calls = mutableListOf<Call>()
        var loseSelection = false
        val presentation = AccountPresentation { scope, intent, _, _, _, profiles ->
            calls += Call(scope, intent, profiles)
            if (loseSelection && intent == AccountIntent.SelectProfiles) error("lost admission reply")
            AccountCommandReply.newBuilder().setOwnerScope(scope).setGeneration(11).build()
        }
    }
    private fun status(scope: Long = 1, generation: Long = 10, active: Boolean = false, gap: Boolean = false) = AndroidAccountStatus(
        activeScope = scope, activeAccountId = "account-$scope", capability = true, generation = generation,
        requesterActive = active, resultsLost = gap, worker = AccountDiagnostics(stage = "signed_in"))
    private fun choices(scope: Long = 1, sequence: Long = 10, ids: List<ProfileId> = listOf(a, b, c)) =
        MobileAccountResult.newBuilder().setOwnerScope(scope).setGeneration(10).setSequence(sequence).setKind("profiles")
            .setAccount(MobileAccountSummary.newBuilder().setDisplayName("Account $scope"))
            .addAllProfiles(ids.map { MobileAccountProfile.newBuilder().setProfileId(it.value).setName(it.value)
                .setAvailableRemotely(it != a).build() }).build()

    @Test fun oneSubsetAndAllUseOneCanonicalSetCommandWithoutSkippingUnavailableProfiles() = runTest {
        for (selection in listOf(listOf(a), listOf(c, a), listOf(c, b, a))) {
            val port = Port()
            port.presentation.observe(status(), listOf(choices()))
            port.presentation.act(AccountIntent.SelectProfiles, profileIds = selection, expectedOwnerScope = 1)
            port.presentation.act(AccountIntent.SelectProfiles, profileIds = selection, expectedOwnerScope = 1)
            assertEquals(listOf(Call(1, AccountIntent.SelectProfiles, selection.map { it.value }.sorted())), port.calls)
            assertEquals(AccountIntent.SelectProfiles, port.presentation.state.value.pending)
        }
    }

    @Test fun freshRemovalAndStaleScopeRefuseTheWholeOldSelection() = runTest {
        val port = Port()
        port.presentation.observe(status(), listOf(choices()))
        port.presentation.observe(status(), listOf(choices(sequence = 11, ids = listOf(b, c))))
        port.presentation.act(AccountIntent.SelectProfiles, profileIds = listOf(a, b), expectedOwnerScope = 1)
        port.presentation.act(AccountIntent.SelectProfiles, profileIds = listOf(b, b), expectedOwnerScope = 1)
        assertTrue(port.calls.isEmpty())
        port.presentation.observe(status(2), listOf(choices(scope = 2, sequence = 12, ids = listOf(c))))
        port.presentation.act(AccountIntent.SelectProfiles, profileIds = listOf(c), expectedOwnerScope = 1)
        assertTrue(port.calls.isEmpty())
        port.presentation.act(AccountIntent.SelectProfiles, profileIds = listOf(c), expectedOwnerScope = 2)
        assertEquals(listOf(Call(2, AccountIntent.SelectProfiles, listOf(c.value))), port.calls)
    }

    @Test fun lostAdmissionAndResultGapNeverResubmitOrReuseOldGrants() = runTest {
        val port = Port()
        port.presentation.observe(status(), listOf(choices()))
        port.loseSelection = true
        port.presentation.act(AccountIntent.SelectProfiles, profileIds = listOf(a, b), expectedOwnerScope = 1)
        port.presentation.act(AccountIntent.SelectProfiles, profileIds = listOf(a, b), expectedOwnerScope = 1)
        assertTrue(port.presentation.state.value.commandUncertain)
        assertEquals(1, port.calls.count { it.intent == AccountIntent.SelectProfiles })
        port.presentation.observe(status(generation = 11, active = true, gap = true), emptyList())
        assertFalse(port.presentation.state.value.canChoose)
        port.presentation.observe(status(generation = 11), emptyList())
        port.presentation.act(AccountIntent.SelectProfiles, profileIds = listOf(a, b), expectedOwnerScope = 1)
        assertEquals(1, port.calls.count { it.intent == AccountIntent.SelectProfiles })
        assertFalse(port.presentation.state.value.profilesLoaded)
    }

    @Test fun malformedHostedIdsNeverBecomeSelectableAuthority() = runTest {
        val port = Port()
        port.presentation.observe(status(), listOf(choices(ids = listOf(a, ProfileId("a0000000-0000-4000-8000-000000000001".uppercase()), ProfileId("not-an-id")))))
        assertEquals(listOf(a), port.presentation.state.value.profiles.map { it.id })
    }
}

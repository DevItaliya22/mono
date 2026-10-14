// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import app.koru.contract.api.v1.*
import com.google.protobuf.ByteString
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.withContext
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class AndroidAccountAdapterTest {
    private val entry = "v1:test:${"a".repeat(64)}:${"b".repeat(64)}:10000000-0000-4000-8000-000000000001"

    private class Native : AccountNativePort {
        var stage = "signed_out"
        var activeScope = 0L
        var held = emptyList<MobileHeldAccount>()
        var signIn: NativeSignInPrompt? = null
        var vault: AccountVaultPrompt? = null
        var results = emptyList<MobileAccountResult>()
        var failPoll = false
        var consumeSignIn = true
        var consumeVault = true
        var loseSignInAck = false
        var loseVaultAck = false
        var nextSignIn: NativeSignInPrompt? = null
        var nextVault: AccountVaultPrompt? = null
        val platforms = mutableListOf<AccountPlatformRequest>()
        val polls = mutableListOf<AccountPollRequest>()
        val commands = mutableListOf<AccountCommandRequest>()
        override suspend fun command(request: AccountCommandRequest): AccountCommandReply {
            commands += request
            return AccountCommandReply.newBuilder().setGeneration(1).build()
        }
        override suspend fun poll(request: AccountPollRequest): AccountPollReply {
            polls += request
            if (failPoll) return AccountPollReply.newBuilder().setErrorCode("account_host_unavailable").build()
            return AccountPollReply.newBuilder()
                .setStatus(MobileAccountStatus.newBuilder().setStage(stage).setGeneration(1).setActiveScope(activeScope).addAllHeldAccounts(held))
                .also { builder -> signIn?.let(builder::setSignIn); vault?.let(builder::setVault) }
                .addAllResults(results).build()
        }
        override suspend fun platform(request: AccountPlatformRequest): MutationResult {
            platforms += request
            if (request.hasSignIn()) {
                if (consumeSignIn) signIn = nextSignIn.also { nextSignIn = null }
                if (loseSignInAck) {
                    loseSignInAck = false
                    error("fixture lost platform acknowledgement")
                }
            }
            if (request.hasVault()) {
                if (consumeVault) vault = nextVault.also { nextVault = null }
                if (loseVaultAck) {
                    loseVaultAck = false
                    error("fixture lost platform acknowledgement")
                }
            }
            return MutationResult.getDefaultInstance()
        }
        fun signInReplies() = platforms.filter { it.hasSignIn() }.map { it.signIn }
        fun vaultReplies() = platforms.filter { it.hasVault() }.map { it.vault }
    }

    private class Dialog : GoogleAccountDialog {
        val prompts = mutableListOf<NativeSignInPrompt>()
        val responses = mutableMapOf<Long, CompletableDeferred<GoogleAccountResult>>()
        var cancellations = 0
        var clears = 0
        var failClear = false
        var clearGate: CompletableDeferred<Unit>? = null
        val ignoreCancellation = mutableSetOf<Long>()
        override suspend fun signIn(prompt: NativeSignInPrompt): GoogleAccountResult {
            prompts += prompt
            return try {
                val response = responses.getOrPut(prompt.requestId) { CompletableDeferred() }
                if (prompt.requestId in ignoreCancellation) withContext(NonCancellable) { response.await() }
                else response.await()
            } catch (error: CancellationException) {
                cancellations += 1
                throw error
            }
        }
        override suspend fun clearSession() {
            clears += 1
            clearGate?.await()
            if (failClear) error("fixture provider clear failed")
        }
        fun complete(id: Long, result: GoogleAccountResult) {
            responses.getOrPut(id) { CompletableDeferred() }.complete(result)
        }
    }

    private class Vault : AccountCredentialVault {
        val values = mutableMapOf<String, ByteArray>()
        val loads = mutableListOf<String>()
        val stores = mutableListOf<String>()
        val clears = mutableListOf<String>()
        var failLoad = false
        var storeGate: CompletableDeferred<Unit>? = null
        override suspend fun load(entry: String): ByteArray? {
            loads += entry
            if (failLoad) throw AccountVaultException(AccountVaultFailure.Unavailable)
            return values[entry]?.copyOf()
        }
        override suspend fun store(entry: String, credential: ByteArray) {
            stores += entry
            storeGate?.await()
            values[entry] = credential.copyOf()
        }
        override suspend fun clear(entry: String) {
            clears += entry
            values.remove(entry)
        }
    }

    private fun prompt(id: Long = 1, generation: Long = 1) = NativeSignInPrompt.newBuilder()
        .setRequestId(id).setGeneration(generation).setNonce("fixture-nonce-$id")
        .setExpiresAtMillis(120_000).setServerClientId("fixture.apps.googleusercontent.com").build()

    private fun vaultPrompt(id: Long, action: AccountVaultAction, credential: ByteArray? = null) =
        AccountVaultPrompt.newBuilder().setRequestId(id).setAction(action).setEntry(entry)
            .also { builder -> credential?.let { builder.credential = ByteString.copyFrom(it) } }.build()

    private fun TestScope.adapter(native: Native, dialog: Dialog, vault: Vault = Vault()) =
        AndroidAccountAdapter(native, dialog, vault, backgroundScope) { testScheduler.currentTime }

    @Test fun selectionAndRetryUseOnlyTheirTypedFieldsAndCapturedScope() = runTest {
        val native = Native()
        val dialog = Dialog()
        val adapter = adapter(native, dialog)
        val profile = "10000000-0000-4000-8000-000000000001"
        adapter.command(AccountAction.ACCOUNT_ACTION_SELECT_PROFILES, expectedOwnerScope = 9, profileIds = listOf(profile))
        adapter.command(AccountAction.ACCOUNT_ACTION_RETRY_ENROLLMENT, expectedOwnerScope = 9, profileId = profile)
        assertEquals(listOf(profile), native.commands[0].profileIdsList)
        assertTrue(native.commands[0].profileId.isEmpty())
        assertEquals(profile, native.commands[1].profileId)
        assertEquals(0, native.commands[1].profileIdsCount)
        assertTrue(native.commands.all { it.hasExpectedOwnerScope() && it.expectedOwnerScope == 9L })
        assertTrue(dialog.prompts.isEmpty())
    }

    private suspend fun lostAck(adapter: AndroidAccountAdapter) {
        try {
            adapter.tick()
            fail("the fake transport must report the lost acknowledgement")
        } catch (error: IllegalStateException) {
            assertEquals("fixture lost platform acknowledgement", error.message)
        }
    }

    @Test fun repeatedPollsAndActivityRotationKeepOneLivingDialog() = runTest {
        val native = Native().apply { stage = "waiting_for_provider"; signIn = prompt() }
        val dialog = Dialog()
        val adapter = adapter(native, dialog)
        adapter.foreground(true)
        adapter.tick(); runCurrent()
        repeat(3) { adapter.tick(); runCurrent() }
        adapter.foreground(false)
        adapter.tick(); runCurrent()
        adapter.foreground(true)
        adapter.tick(); runCurrent()
        assertEquals(1, dialog.prompts.size)
        assertEquals(0, dialog.cancellations)
        assertEquals(listOf(true, false, true), native.platforms.filter { it.hasForeground() }.map { it.foreground })
        val result = GoogleAccountResult("fixture-id-token".toByteArray())
        dialog.complete(1, result); runCurrent()
        adapter.tick()
        assertEquals(1, native.signInReplies().size)
        assertEquals("fixture-id-token", native.signInReplies().single().idToken.toStringUtf8())
        assertTrue(result.token!!.all { it == 0.toByte() })
    }

    @Test fun aBrieflyRepeatedAcknowledgedPromptDoesNotOpenAnotherSheet() = runTest {
        val native = Native().apply { stage = "waiting_for_provider"; signIn = prompt(); consumeSignIn = false }
        val dialog = Dialog()
        val adapter = adapter(native, dialog)
        adapter.foreground(true)
        adapter.tick(); runCurrent()
        dialog.complete(1, GoogleAccountResult("fixture-id-token".toByteArray())); runCurrent()
        adapter.tick()
        repeat(3) { adapter.tick(); runCurrent() }
        assertEquals(1, dialog.prompts.size)
        assertEquals(1, native.signInReplies().size)
    }

    @Test fun aConsumedProofWithLostAcknowledgementIsReleasedWithoutSdkReplay() = runTest {
        for (next in listOf<NativeSignInPrompt?>(null, prompt(2, 2))) {
            val native = Native().apply {
                stage = "waiting_for_provider"; signIn = prompt(); loseSignInAck = true; nextSignIn = next
            }
            val dialog = Dialog()
            val adapter = adapter(native, dialog)
            adapter.foreground(true)
            adapter.tick(); runCurrent()
            val result = GoogleAccountResult("fixture-id-token".toByteArray())
            dialog.complete(1, result); runCurrent()
            lostAck(adapter)
            adapter.tick(); runCurrent()
            repeat(2) { adapter.tick(); runCurrent() }
            assertEquals(if (next == null) listOf(1L) else listOf(1L, 2L), dialog.prompts.map { it.requestId })
            assertEquals(1, native.signInReplies().size)
            assertTrue(result.token!!.all { it == 0.toByte() })
        }
    }

    @Test fun userCancellationIsAcorrelatedCancelledReplyWithoutToken() = runTest {
        val native = Native().apply { stage = "waiting_for_provider"; signIn = prompt(7, 9) }
        val dialog = Dialog()
        val adapter = adapter(native, dialog)
        adapter.foreground(true)
        adapter.tick(); runCurrent()
        dialog.complete(7, GoogleAccountResult(cancelled = true)); runCurrent()
        adapter.tick()
        val reply = native.signInReplies().single()
        assertEquals(7L, reply.requestId)
        assertEquals(9L, reply.generation)
        assertTrue(reply.cancelled)
        assertFalse(reply.hasIdToken())
    }

    @Test fun durableStoreRunsOnceAcrossLostAckAndTheFollowingVaultRequest() = runTest {
        val credential = "fixture-serialized-credential".toByteArray()
        val native = Native().apply {
            stage = "exchanging"
            vault = vaultPrompt(1, AccountVaultAction.ACCOUNT_VAULT_ACTION_STORE, credential)
            loseVaultAck = true
            nextVault = vaultPrompt(2, AccountVaultAction.ACCOUNT_VAULT_ACTION_LOAD)
        }
        val vault = Vault()
        val dialog = Dialog()
        val adapter = adapter(native, dialog, vault)
        adapter.tick(); runCurrent()
        lostAck(adapter)
        adapter.tick(); runCurrent()
        adapter.tick()
        assertEquals(listOf(entry), vault.stores)
        assertEquals(listOf(entry), vault.loads)
        assertEquals(listOf(1L, 2L), native.vaultReplies().map { it.requestId })
        assertArrayEquals(credential, native.vaultReplies().last().credential.toByteArray())
        assertEquals(0, dialog.prompts.size)
    }

    @Test fun acknowledgedStorePromptCanLingerWithoutRepeatingTheVaultWrite() = runTest {
        val native = Native().apply {
            stage = "exchanging"; consumeVault = false
            vault = vaultPrompt(1, AccountVaultAction.ACCOUNT_VAULT_ACTION_STORE, byteArrayOf(1, 2))
        }
        val vault = Vault()
        val adapter = adapter(native, Dialog(), vault)
        adapter.tick(); runCurrent()
        adapter.tick()
        repeat(3) { adapter.tick(); runCurrent() }
        assertEquals(listOf(entry), vault.stores)
        assertEquals(1, native.vaultReplies().size)
    }

    @Test fun backgroundingAndNewPromptDoNotAbandonAnInFlightDurableStore() = runTest {
        val native = Native().apply {
            stage = "exchanging"
            vault = vaultPrompt(1, AccountVaultAction.ACCOUNT_VAULT_ACTION_STORE, byteArrayOf(1, 2))
        }
        val vault = Vault().apply { storeGate = CompletableDeferred() }
        val adapter = adapter(native, Dialog(), vault)
        adapter.foreground(true)
        adapter.tick(); runCurrent()
        adapter.foreground(false)
        native.vault = vaultPrompt(2, AccountVaultAction.ACCOUNT_VAULT_ACTION_LOAD)
        adapter.tick(); runCurrent()
        assertEquals(listOf(entry), vault.stores)
        assertTrue(vault.loads.isEmpty())
        vault.storeGate!!.complete(Unit); runCurrent()
        adapter.tick(); runCurrent()
        adapter.tick()
        assertArrayEquals(byteArrayOf(1, 2), native.vaultReplies().single().credential.toByteArray())
        assertEquals(2L, native.vaultReplies().single().requestId)
    }

    @Test fun vaultUnavailableIsNotReportedAsAnEmptyVault() = runTest {
        val native = Native().apply { stage = "restoring"; vault = vaultPrompt(4, AccountVaultAction.ACCOUNT_VAULT_ACTION_LOAD) }
        val vault = Vault().apply { failLoad = true }
        val adapter = adapter(native, Dialog(), vault)
        adapter.tick(); runCurrent()
        adapter.tick()
        val reply = native.vaultReplies().single()
        assertEquals(4L, reply.requestId)
        assertFalse(reply.success)
        assertFalse(reply.hasCredential())
    }

    @Test fun signedOutClearsProviderOnInitialObservationAndAfterFailedLogoutResult() = runTest {
        val native = Native()
        val dialog = Dialog()
        val vault = Vault().apply { values[entry] = byteArrayOf(1, 2) }
        val adapter = adapter(native, dialog, vault)
        adapter.tick(); runCurrent()
        repeat(2) { adapter.tick(); runCurrent() }
        assertEquals(1, dialog.clears)
        native.stage = "signed_in"
        native.activeScope = 1
        native.held = listOf(MobileHeldAccount.newBuilder().setScopeId(1).setAccountId("10000000-0000-4000-8000-000000000001").build())
        adapter.tick()
        native.stage = "signed_out"
        native.held = emptyList()
        native.results = listOf(MobileAccountResult.newBuilder().setSequence(1)
            .setRequester(AndroidAccountAdapter.REQUESTER).setGeneration(1)
            .setKind("failed").setErrorCode("enrollment_persistence_failed").build())
        native.vault = vaultPrompt(1, AccountVaultAction.ACCOUNT_VAULT_ACTION_CLEAR)
        dialog.failClear = true
        adapter.tick(); runCurrent()
        adapter.tick()
        assertEquals(2, dialog.clears)
        assertEquals(listOf(entry), vault.clears)
        assertTrue(vault.values.isEmpty())
        assertEquals("provider_session_clear_failed", adapter.status.value.adapterFailure)
        assertEquals("failed", adapter.results.value.single().kind)
        dialog.failClear = false
        adapter.command(AccountAction.ACCOUNT_ACTION_START_GOOGLE, locale = "en", expectedOwnerScope = 0)
        adapter.tick(); runCurrent(); adapter.tick()
        assertEquals(3, dialog.clears)
        assertNull(adapter.status.value.adapterFailure)
        assertTrue(vault.values.isEmpty())
    }

    @Test fun switchingToAnonymousKeepsHeldSdkAndRetirementWaitsForItsAcknowledgement() = runTest {
        val owner = MobileHeldAccount.newBuilder().setScopeId(1)
            .setAccountId("10000000-0000-4000-8000-000000000001").setCredentialReady(true).build()
        val native = Native().apply { stage = "waiting_for_provider"; activeScope = 1; held = listOf(owner); signIn = prompt() }
        val dialog = Dialog()
        val adapter = adapter(native, dialog)
        adapter.foreground(true)
        adapter.tick(); runCurrent()
        assertEquals(1, dialog.prompts.size)
        native.activeScope = 2
        native.stage = "signed_out"
        adapter.tick(); runCurrent()
        assertEquals(0, dialog.clears)
        assertEquals(0, dialog.cancellations)
        // Whole inventory proves retirement even if its result was lost.
        native.held = emptyList()
        adapter.tick(); runCurrent()
        assertEquals(0, dialog.clears)
        dialog.complete(1, GoogleAccountResult(token = byteArrayOf(7)))
        runCurrent(); adapter.tick(); runCurrent()
        assertEquals(1, native.signInReplies().size)
        adapter.tick(); runCurrent(); adapter.tick()
        assertEquals(1, dialog.clears)
        assertEquals(1, dialog.prompts.size)
    }

    @Test fun aHungProviderClearTimesOutAndLeavesTheExplicitChooserAvailable() = runTest {
        val native = Native().apply { signIn = prompt() }
        val dialog = Dialog().apply { clearGate = CompletableDeferred() }
        val adapter = adapter(native, dialog)
        adapter.foreground(true)
        adapter.tick(); runCurrent()
        assertTrue(dialog.prompts.isEmpty())
        advanceTimeBy(30_000); runCurrent()
        adapter.tick(); runCurrent()
        assertEquals("provider_session_clear_failed", adapter.status.value.adapterFailure)
        assertEquals(1, dialog.prompts.size)
        assertEquals(1, dialog.clears)
        dialog.complete(1, GoogleAccountResult(cancelled = true))
        runCurrent(); adapter.tick()
    }

    @Test fun aNewProcessRestoresOnlyItsDurableVaultAndDoesNotRecreateAGoogleSheet() = runTest {
        for (credential in listOf<ByteArray?>(null, "fixture-durable-credential".toByteArray())) {
            val native = Native().apply {
                stage = "restoring"; vault = vaultPrompt(1, AccountVaultAction.ACCOUNT_VAULT_ACTION_LOAD)
            }
            val dialog = Dialog()
            val vault = Vault().apply { credential?.let { values[entry] = it.copyOf() } }
            val adapter = adapter(native, dialog, vault)
            adapter.foreground(true)
            adapter.tick(); runCurrent()
            adapter.tick()
            val reply = native.vaultReplies().single()
            assertTrue(reply.success)
            assertEquals(credential != null, reply.hasCredential())
            if (credential != null) assertArrayEquals(credential, reply.credential.toByteArray())
            native.stage = if (credential == null) "signed_out" else "signed_in"
            adapter.tick(); runCurrent()
            assertEquals(credential == null, adapter.status.value.signInAgain)
            assertTrue(dialog.prompts.isEmpty())
        }
    }

    @Test fun failedPollIsNotPromptRemovalAndKeepsTheOriginalDialog() = runTest {
        val native = Native().apply { stage = "waiting_for_provider"; signIn = prompt() }
        val dialog = Dialog()
        val adapter = adapter(native, dialog)
        adapter.foreground(true)
        adapter.tick(); runCurrent()
        native.failPoll = true
        repeat(3) { adapter.tick(); runCurrent() }
        assertEquals(1, dialog.prompts.size)
        assertEquals(0, dialog.cancellations)
        assertTrue(native.signInReplies().isEmpty())
        assertEquals("account_adapter_unavailable", adapter.status.value.adapterFailure)
        native.failPoll = false
        dialog.complete(1, GoogleAccountResult("fixture-id-token".toByteArray())); runCurrent()
        adapter.tick()
        assertEquals(1, native.signInReplies().size)
        assertEquals("fixture-id-token", native.signInReplies().single().idToken.toStringUtf8())
        assertEquals(1, dialog.prompts.size)
    }

    @Test fun aRetiredCancellationInsensitiveDialogWipesItsLateTokenWithoutPoisoningTheNextPrompt() = runTest {
        for (replaceImmediately in listOf(false, true)) {
            val native = Native().apply { stage = "waiting_for_provider"; signIn = prompt() }
            val dialog = Dialog().apply { ignoreCancellation += 1L }
            val adapter = adapter(native, dialog)
            adapter.foreground(true)
            adapter.tick(); runCurrent()
            native.signIn = if (replaceImmediately) prompt(2, 2) else null
            adapter.tick(); runCurrent()
            if (!replaceImmediately) {
                native.signIn = prompt(2, 2)
                adapter.tick(); runCurrent()
            }
            assertEquals(listOf(1L, 2L), dialog.prompts.map { it.requestId })
            val retired = GoogleAccountResult("fixture-late-old-token".toByteArray())
            dialog.complete(1, retired); runCurrent()
            assertTrue("late owned SDK token must be wiped", retired.token!!.all { it == 0.toByte() })
            assertTrue(native.signInReplies().isEmpty())
            dialog.complete(2, GoogleAccountResult("fixture-new-token".toByteArray())); runCurrent()
            adapter.tick()
            val reply = native.signInReplies().single()
            assertEquals(2L, reply.requestId)
            assertEquals(2L, reply.generation)
            assertEquals("fixture-new-token", reply.idToken.toStringUtf8())
        }
    }

    @Test fun nativeProofAcceptsTheExactContractLimitAndWipesAnOversizedResult() = runTest {
        for (size in listOf(16_384, 16_385)) {
            val native = Native().apply { stage = "waiting_for_provider"; signIn = prompt() }
            val dialog = Dialog()
            val adapter = adapter(native, dialog)
            adapter.foreground(true)
            adapter.tick(); runCurrent()
            val proof = GoogleAccountResult(ByteArray(size) { 65 })
            dialog.complete(1, proof); runCurrent()
            if (size > 16_384) assertTrue(proof.token!!.all { it == 0.toByte() })
            adapter.tick()
            val reply = native.signInReplies().single()
            assertEquals(size == 16_384, reply.hasIdToken())
            if (size == 16_384) assertEquals(size, reply.idToken.size())
            assertFalse(reply.cancelled)
            assertTrue(proof.token!!.all { it == 0.toByte() })
        }
    }

    @Test fun anExpiredPromptFailsWithItsCorrelationWithoutOpeningTheSdk() = runTest {
        for (expires in listOf(0L, -1L)) {
            val native = Native().apply {
                stage = "waiting_for_provider"
                signIn = prompt(8, 12).toBuilder().setExpiresAtMillis(expires).build()
            }
            val dialog = Dialog()
            val adapter = adapter(native, dialog)
            adapter.foreground(true)
            adapter.tick(); runCurrent()
            adapter.tick()
            assertTrue(dialog.prompts.isEmpty())
            val reply = native.signInReplies().single()
            assertEquals(8L, reply.requestId)
            assertEquals(12L, reply.generation)
            assertFalse(reply.hasIdToken())
            assertFalse(reply.cancelled)
        }
    }

    @Test fun changedNonceOrClientAtTheSameCorrelationFailsClosedUntilANewPrompt() = runTest {
        for (changeNonce in listOf(true, false)) {
            val original = prompt()
            val native = Native().apply { stage = "waiting_for_provider"; signIn = original }
            val dialog = Dialog()
            val adapter = adapter(native, dialog)
            adapter.foreground(true)
            adapter.tick(); runCurrent()
            native.signIn = original.toBuilder().also {
                if (changeNonce) it.nonce = "different-fixture-nonce"
                else it.serverClientId = "different.apps.googleusercontent.com"
            }.build()
            adapter.tick(); runCurrent()
            val oldProof = GoogleAccountResult("fixture-original-proof".toByteArray())
            dialog.complete(1, oldProof); runCurrent()
            repeat(2) { adapter.tick(); runCurrent() }
            assertEquals("account_prompt_changed", adapter.status.value.adapterFailure)
            assertEquals(listOf(1L), dialog.prompts.map { it.requestId })
            assertTrue(native.signInReplies().isEmpty())

            native.signIn = prompt(2, 2)
            adapter.tick(); runCurrent()
            assertTrue(oldProof.token!!.all { it == 0.toByte() })
            assertEquals(listOf(1L, 2L), dialog.prompts.map { it.requestId })
            dialog.complete(2, GoogleAccountResult("fixture-new-proof".toByteArray())); runCurrent()
            adapter.tick()
            val reply = native.signInReplies().single()
            assertEquals(2L, reply.requestId)
            assertEquals(2L, reply.generation)
            assertEquals("fixture-new-proof", reply.idToken.toStringUtf8())
        }
    }
}

// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import app.koru.contract.api.v1.*
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test

/** Sign in with Apple on Android: one browser page per prompt, and the App
 * Link that returns from it handed back for exactly that page. */
@OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
class BrowserSignInTest {
    private val apple = "https://appleid.apple.com/auth/authorize?client_id=do.koru&state=s"
    private val callback = "https://www.koru.do/v1/oauth/app/callback?state=s&outcome=o"

    private class Native : AccountNativePort {
        var browser: BrowserSignInPrompt? = null
        var consumeBrowser = true
        var loseBrowserAck = false
        var failPoll = false
        val platforms = mutableListOf<AccountPlatformRequest>()
        val commands = mutableListOf<AccountCommandRequest>()
        override suspend fun command(request: AccountCommandRequest): AccountCommandReply {
            commands += request
            return AccountCommandReply.newBuilder().setGeneration(1).build()
        }
        override suspend fun poll(request: AccountPollRequest): AccountPollReply {
            if (failPoll) return AccountPollReply.newBuilder().setErrorCode("account_host_unavailable").build()
            return AccountPollReply.newBuilder()
                .setStatus(MobileAccountStatus.newBuilder().setStage("waiting_for_provider").setGeneration(1))
                .also { builder -> browser?.let(builder::setBrowser) }.build()
        }
        override suspend fun platform(request: AccountPlatformRequest): MutationResult {
            platforms += request
            if (request.hasBrowser()) {
                if (consumeBrowser) browser = null
                if (loseBrowserAck) {
                    loseBrowserAck = false
                    error("fixture lost platform acknowledgement")
                }
            }
            return MutationResult.getDefaultInstance()
        }
        fun browserReplies() = platforms.filter { it.hasBrowser() }.map { it.browser }
    }

    private class Launcher : BrowserSignInLauncher {
        val opened = mutableListOf<String>()
        var available = true
        override suspend fun open(url: String): Boolean {
            opened += url
            return available
        }
    }

    private object NoDialog : GoogleAccountDialog {
        override suspend fun signIn(prompt: NativeSignInPrompt) = GoogleAccountResult(cancelled = true)
        override suspend fun clearSession() {}
    }

    private object NoVault : AccountCredentialVault {
        override suspend fun load(entry: String): ByteArray? = null
        override suspend fun store(entry: String, credential: ByteArray) {}
        override suspend fun clear(entry: String) {}
    }

    private fun prompt(id: Long, generation: Long = 4, url: String = apple) =
        BrowserSignInPrompt.newBuilder().setGeneration(generation).setRequestId(id).setUrl(url).build()

    private fun TestScope.adapter(native: Native, launcher: Launcher) =
        AndroidAccountAdapter(native, NoDialog, NoVault, backgroundScope, launcher) { testScheduler.currentTime }
            .also { it.foreground(true) }

    @Test fun onlyHttpsProviderPagesOnAppleOrGoogleAreOpened() {
        assertTrue(BrowserSignIn.isProviderPage(apple))
        assertTrue(BrowserSignIn.isProviderPage("https://accounts.google.com/o/oauth2/v2/auth?x=1"))
        for (refused in listOf(
            "http://appleid.apple.com/auth/authorize",
            "https://appleid.apple.com.evil.example/auth",
            "https://evil.example/?https://appleid.apple.com",
            "https://user@appleid.apple.com/auth",
            "https://appleid.apple.com:8443/auth",
            "intent://appleid.apple.com/#Intent;scheme=https;end",
            "javascript:alert(1)",
            "not a url",
            "",
            "https://appleid.apple.com/" + "a".repeat(BrowserSignIn.MAX_URL_BYTES),
        )) assertFalse(refused, BrowserSignIn.isProviderPage(refused))
    }

    @Test fun onlyTheKoruCallbackPathIsAcceptedAsTheReturn() {
        assertTrue(BrowserSignIn.isCallback(callback))
        for (refused in listOf(
            "http://www.koru.do/v1/oauth/app/callback?state=s",
            "https://koru.do/v1/oauth/app/callback?state=s",
            "https://www.koru.do/link-device?state=s",
            "https://www.koru.do/v1/oauth/app/callback/extra?state=s",
            "https://www.koru.do.evil.example/v1/oauth/app/callback?state=s",
            "https://www.koru.do/v1/oauth/app/callback#state=s",
        )) assertFalse(refused, BrowserSignIn.isCallback(refused))
    }

    @Test fun eachRequestIdOpensOnceAcrossRepeatedPolls() = runTest {
        val native = Native()
        val launcher = Launcher()
        val adapter = adapter(native, launcher)
        native.browser = prompt(1)
        repeat(4) { adapter.tick() }
        assertEquals(listOf(apple), launcher.opened)
        // The collision proof asks for a second page: a new request_id.
        native.browser = prompt(2)
        repeat(3) { adapter.tick() }
        assertEquals(listOf(apple, apple), launcher.opened)
        assertTrue(native.browserReplies().isEmpty())
    }

    @Test fun aBackgroundedAppWaitsToOpenThePage() = runTest {
        val native = Native()
        val launcher = Launcher()
        val adapter = adapter(native, launcher)
        adapter.foreground(false)
        native.browser = prompt(1)
        adapter.tick()
        assertTrue(launcher.opened.isEmpty())
        adapter.foreground(true)
        adapter.tick()
        assertEquals(1, launcher.opened.size)
    }

    @Test fun anUnrecognisedPageIsNeverOpenedAndIsAnsweredAsCancelled() = runTest {
        val native = Native()
        val launcher = Launcher()
        val adapter = adapter(native, launcher)
        native.browser = prompt(1, url = "https://evil.example/auth")
        adapter.tick()
        adapter.tick()
        assertTrue(launcher.opened.isEmpty())
        val reply = native.browserReplies().single()
        assertEquals(4, reply.generation)
        assertEquals(1, reply.requestId)
        assertTrue(reply.cancelled)
        assertFalse(reply.hasCallbackUrl())
    }

    @Test fun aPageNothingCouldShowIsAnsweredAsCancelled() = runTest {
        val native = Native()
        val launcher = Launcher().apply { available = false }
        val adapter = adapter(native, launcher)
        native.browser = prompt(1)
        adapter.tick()
        adapter.tick()
        assertEquals(1, launcher.opened.size)
        assertTrue(native.browserReplies().single().cancelled)
        assertFalse(adapter.browserReturned(callback))
    }

    @Test fun theReturnedLinkAnswersTheMostRecentlyOpenedPageOnce() = runTest {
        val native = Native()
        val launcher = Launcher()
        val adapter = adapter(native, launcher)
        native.browser = prompt(1)
        adapter.tick()
        native.browser = prompt(2)
        adapter.tick()
        assertTrue(adapter.browserReturned(callback))
        adapter.tick()
        val reply = native.browserReplies().single()
        assertEquals(4, reply.generation)
        assertEquals(2, reply.requestId)
        assertEquals(callback, reply.callbackUrl)
        assertFalse(reply.cancelled)
        // The page is answered: a second arrival of the same link is ignored.
        assertFalse(adapter.browserReturned(callback))
        adapter.tick()
        assertEquals(1, native.browserReplies().size)
    }

    @Test fun aLinkWithNoOutstandingPageIsIgnored() = runTest {
        val native = Native()
        val launcher = Launcher()
        val adapter = adapter(native, launcher)
        assertFalse(adapter.browserReturned(callback))
        adapter.tick()
        assertTrue(native.platforms.none { it.hasBrowser() })
        // The flow ends (cancelled, timed out) and its prompt leaves the poll:
        // a late link is not sent to the finished flow.
        native.browser = prompt(1)
        adapter.tick()
        native.browser = null
        adapter.tick()
        assertFalse(adapter.browserReturned(callback))
        adapter.tick()
        assertTrue(native.browserReplies().isEmpty())
    }

    @Test fun aLinkThatIsNotTheCallbackIsRefused() = runTest {
        val native = Native()
        val adapter = adapter(native, Launcher())
        native.browser = prompt(1)
        adapter.tick()
        assertFalse(adapter.browserReturned("https://www.koru.do/elsewhere?state=s"))
        adapter.tick()
        assertTrue(native.browserReplies().isEmpty())
    }

    @Test fun aLostAcknowledgementIsRetriedAndAFailedPollIsNotAbsence() = runTest {
        val native = Native()
        val adapter = adapter(native, Launcher())
        native.browser = prompt(1)
        adapter.tick()
        assertTrue(adapter.browserReturned(callback))
        native.consumeBrowser = false
        native.loseBrowserAck = true
        adapter.tick()
        assertEquals(1, native.browserReplies().size)
        native.failPoll = true
        adapter.tick()
        native.failPoll = false
        native.consumeBrowser = true
        adapter.tick()
        assertEquals(listOf(callback, callback), native.browserReplies().map { it.callbackUrl })
        adapter.tick()
        assertEquals(2, native.browserReplies().size)
    }

    @Test fun signInWithAppleDispatchesStartAppleWithTheOwnerScope() = runTest {
        assertEquals(AccountAction.ACCOUNT_ACTION_START_APPLE, AccountIntent.SignInWithApple.wire())
        val native = Native()
        val adapter = adapter(native, Launcher())
        adapter.command(AccountIntent.SignInWithApple.wire(), locale = "de", expectedOwnerScope = 7)
        val command = native.commands.single()
        assertEquals(AccountAction.ACCOUNT_ACTION_START_APPLE, command.action)
        assertEquals("de", command.locale)
        assertEquals(7, command.expectedOwnerScope)
    }

    @Test fun signInWithAppleIsAdmittedLikeGoogleAndCanBeCancelled() = runTest {
        val calls = mutableListOf<Pair<AccountIntent, Long>>()
        val presentation = AccountPresentation { owner, intent, generation, _, _, _ ->
            calls += intent to owner
            AccountCommandReply.newBuilder().setGeneration(if (intent == AccountIntent.Cancel) generation else 2).build()
        }
        presentation.observe(AndroidAccountStatus(worker = AccountDiagnostics(stage = "signed_out"),
            generation = 1, capability = true, requesterActive = false, activeScope = 3), emptyList())
        presentation.act(AccountIntent.SignInWithApple, "en", expectedOwnerScope = 3)
        presentation.act(AccountIntent.SignInWithApple, "en", expectedOwnerScope = 3)
        assertEquals(listOf(AccountIntent.SignInWithApple to 3L), calls)
        assertEquals(AccountIntent.SignInWithApple, presentation.state.value.pending)
        assertTrue(presentation.state.value.canCancel)
        presentation.act(AccountIntent.Cancel, expectedOwnerScope = 3)
        assertEquals(AccountIntent.Cancel, calls.last().first)
    }
}

// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.net.URI
import java.net.URISyntaxException

/** Sign in with Apple has no Android sheet: the host asks for a provider page
 * in the browser, and the result comes back as the verified App Link
 * `https://www.koru.do/v1/oauth/app/callback`. Rust validates both ends; these
 * checks are defence in depth, so a page the phone would not recognise is never
 * opened and a stray link is never forwarded. Neither URL is ever logged: the
 * callback carries a one-use grant. */
internal object BrowserSignIn {
    /** Rust's own ceiling on a callback URL (`MAX_CALLBACK_URL_BYTES`). */
    const val MAX_URL_BYTES = 8 * 1024
    const val CALLBACK_HOST = "www.koru.do"
    const val CALLBACK_PATH = "/v1/oauth/app/callback"
    private val PROVIDER_HOSTS = setOf("appleid.apple.com", "accounts.google.com")

    fun isProviderPage(url: String): Boolean = https(url)?.host in PROVIDER_HOSTS

    fun isCallback(url: String): Boolean =
        https(url)?.let { it.host == CALLBACK_HOST && it.rawPath == CALLBACK_PATH } == true

    private fun https(url: String): URI? {
        if (url.toByteArray(Charsets.UTF_8).size > MAX_URL_BYTES) return null
        val uri = try { URI(url) } catch (_: URISyntaxException) { return null }
        return uri.takeIf {
            it.scheme == "https" && it.rawUserInfo == null && it.host != null &&
                (it.port == -1 || it.port == 443) && it.rawFragment == null
        }
    }
}

/** Opens one provider page. False when nothing could show it. */
internal fun interface BrowserSignInLauncher {
    suspend fun open(url: String): Boolean

    companion object {
        val Unavailable = BrowserSignInLauncher { false }
    }
}

/** A plain `ACTION_VIEW` from the foreground Activity. Custom Tabs would keep
 * the page visually inside Koru, but `androidx.browser` is not a dependency and
 * the return path does not need it: the App Link reaches
 * [app.koru.mobile.BrowserSignInReturnActivity] either way. */
internal class AndroidBrowserSignInLauncher : BrowserSignInLauncher {
    @Volatile private var foregroundActivity: Activity? = null

    fun foreground(activity: Activity) { foregroundActivity = activity }

    fun background(activity: Activity) {
        if (foregroundActivity === activity) foregroundActivity = null
    }

    override suspend fun open(url: String): Boolean = withContext(Dispatchers.Main.immediate) {
        val activity = foregroundActivity?.takeUnless { it.isFinishing || it.isDestroyed }
            ?: return@withContext false
        val intent = Intent(Intent.ACTION_VIEW, Uri.parse(url)).addCategory(Intent.CATEGORY_BROWSABLE)
        try {
            activity.startActivity(intent)
            true
        } catch (_: ActivityNotFoundException) {
            false
        }
    }
}

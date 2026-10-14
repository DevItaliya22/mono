// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile

import android.app.Activity
import android.content.Intent
import android.os.Bundle
import app.koru.mobile.logging.AppLogger

/**
 * Receives the verified App Link `https://www.koru.do/v1/oauth/app/callback`
 * that a browser sign-in (Sign in with Apple) returns through, hands it to the
 * application-owned account service, and gets out of the way.
 *
 * **A trampoline, not a second entry to MainActivity.** MainActivity keeps its
 * `standard` launch mode so the launcher, `link-device` and share flows behave
 * exactly as before. The browser starts this link with `FLAG_ACTIVITY_NEW_TASK`,
 * which lands here in Koru's own task; a filter on MainActivity would instead
 * stack a second, blank MainActivity on top of the one showing the account
 * screen. From here, `CLEAR_TOP | SINGLE_TOP` brings the existing MainActivity
 * forward (and removes the browser if it was opened in this task), and the
 * account screen — a projection of the application-owned worker — shows the
 * exchange the link started. If the process had died meanwhile, the worker's
 * browser wait died with it: the link is ignored and Koru simply opens.
 *
 * The URL carries a one-use grant. It is never logged, and it is not kept on
 * this Activity's intent past `onCreate`.
 */
class BrowserSignInReturnActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (savedInstanceState == null) {
            intent?.dataString?.let { (application as KoruApplication).container.account.browserReturned(it) }
        }
        intent = Intent()
        startActivity(
            Intent(this, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP)
                // MainActivity.onNewIntent reads these; carrying the current
                // values keeps a run-local diagnostic session switched on.
                .putExtra("koru_peer_object_logs", AppLogger.peerObjectsEnabled)
                .putExtra("koru_account_logs", AppLogger.accountEnabled),
        )
        finish()
    }
}

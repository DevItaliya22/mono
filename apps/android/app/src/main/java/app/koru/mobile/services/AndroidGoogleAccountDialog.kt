// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import android.app.Activity
import android.content.Context
import android.content.MutableContextWrapper
import androidx.credentials.ClearCredentialStateRequest
import androidx.credentials.CredentialManager
import androidx.credentials.CustomCredential
import androidx.credentials.GetCredentialRequest
import androidx.credentials.exceptions.GetCredentialCancellationException
import app.koru.contract.api.v1.NativeSignInPrompt
import com.google.android.libraries.identity.googleid.GetSignInWithGoogleOption
import com.google.android.libraries.identity.googleid.GoogleIdTokenCredential
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.isActive
import kotlinx.coroutines.withContext

/** Only Credential Manager sees the foreground Activity. Application-owned
 * operation/correlation survives rotation; no token is retained by the screen. */
internal class AndroidGoogleAccountDialog(context: Context) : GoogleAccountDialog {
    private val trace = AccountTrace()
    private val application = context.applicationContext
    private val manager by lazy { CredentialManager.create(application) }
    private val activityContext = MutableContextWrapper(application)
    private var foregroundActivity: Activity? = null

    fun foreground(activity: Activity) {
        foregroundActivity = activity
        activityContext.baseContext = activity
    }

    fun background(activity: Activity) {
        if (foregroundActivity === activity) {
            foregroundActivity = null
            // Do not retain a destroyed Activity while waiting for replacement.
            activityContext.baseContext = application
        }
    }

    override suspend fun signIn(prompt: NativeSignInPrompt): GoogleAccountResult {
        var ownedToken: ByteArray? = null
        val credentialManager = withContext(Dispatchers.IO) { manager }
        try { return withContext(Dispatchers.Main.immediate) {
        val activity = foregroundActivity
        if (activity == null || activity.isFinishing || activity.isDestroyed) {
            trace.emit(AccountTrace.Stage.GoogleDialog, AccountTrace.Outcome.Failed, prompt.generation,
                AccountTrace.Reason.NoForegroundActivity)
            return@withContext GoogleAccountResult()
        }
        val option = GetSignInWithGoogleOption.Builder(prompt.serverClientId)
            .setNonce(prompt.nonce).build()
        val request = GetCredentialRequest.Builder().addCredentialOption(option).build()
        try {
            // Explicit button flow, including collision reauthentication. No
            // auto-selection or cached bottom-sheet credential is substituted.
            val credential = credentialManager.getCredential(activityContext, request).credential
            if (credential !is CustomCredential || credential.type != GoogleIdTokenCredential.TYPE_GOOGLE_ID_TOKEN_CREDENTIAL) {
                trace.emit(AccountTrace.Stage.GoogleDialog, AccountTrace.Outcome.Failed, prompt.generation,
                    AccountTrace.Reason.UnsupportedCredential)
                return@withContext GoogleAccountResult()
            }
            val token = GoogleIdTokenCredential.createFrom(credential.data).idToken.toByteArray(Charsets.UTF_8)
            ownedToken = token
            if (token.isEmpty() || token.size > MAX_GOOGLE_TOKEN_BYTES) {
                trace.emit(AccountTrace.Stage.GoogleDialog, AccountTrace.Outcome.Failed, prompt.generation,
                    AccountTrace.Reason.InvalidTokenSize)
                token.fill(0); GoogleAccountResult()
            } else GoogleAccountResult(token)
        } catch (_: GetCredentialCancellationException) {
            GoogleAccountResult(cancelled = true)
        }
        // Other SDK errors are translated by the adapter to a bounded failure
        // reply. Exception messages and generated credentials are never logged.
        } } finally {
            // withContext may cancel delivery after the main-thread SDK call
            // produced a result. Its owned bytes still have a wiping owner.
            if (!currentCoroutineContext().isActive) ownedToken?.fill(0)
        }
    }

    override suspend fun clearSession() {
        withContext(Dispatchers.IO) { manager.clearCredentialState(ClearCredentialStateRequest()) }
    }
}

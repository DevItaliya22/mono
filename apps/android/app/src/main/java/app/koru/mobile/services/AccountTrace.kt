// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import app.koru.mobile.logging.AppLogger
import app.koru.contract.api.v1.AccountAction
import app.koru.contract.api.v1.AccountVaultAction

/** Local-only support trace. Accepts categories and counters, never prompts,
 * credentials, account IDs, exception messages or provider response bundles. */
internal class AccountTrace(
    private val enabled: () -> Boolean = { AppLogger.accountEnabled },
    private val logger: AppLogger = AppLogger(),
) {
    enum class Stage { Command, ProviderClear, GoogleDialog, NativeDelivery, Vault, VaultDelivery, Browser, BrowserDelivery }
    enum class Outcome { Started, Completed, Failed, Cancelled, Retired, Stale }
    enum class Reason {
        None, NoForegroundActivity, UnsupportedCredential, InvalidTokenSize,
        DeadlineExpired, Timeout, CoroutineCancelled, ProviderConfiguration,
        NoCredential, ProviderUnsupported, ProviderInterrupted, ProviderUnknown,
        TokenParsing, KeyInvalidated, KeyAuthenticationRequired, Keystore,
        CredentialIntegrity, OtherException, NativeRejected,
        ProviderPageRefused, NoBrowser, NoOutstandingPrompt, CallbackRefused,
    }

    fun emit(stage: Stage, outcome: Outcome, generation: Long = 0, reason: Reason = Reason.None,
             elapsedMs: Long? = null, action: AccountAction? = null, vaultAction: AccountVaultAction? = null) {
        if (!enabled()) return
        logger.event("account_attempt", mapOf("stage" to stage.name,
            "outcome" to outcome.name, "generation" to generation,
            "reason" to reason.name, "elapsed_ms" to elapsedMs,
            "action" to action?.name, "vault_action" to vaultAction?.name))
    }

    companion object {
        fun reason(error: Exception): Reason = when (error.javaClass.simpleName) {
            "GetCredentialProviderConfigurationException" -> Reason.ProviderConfiguration
            "NoCredentialException" -> Reason.NoCredential
            "GetCredentialUnsupportedException" -> Reason.ProviderUnsupported
            "GetCredentialInterruptedException" -> Reason.ProviderInterrupted
            "GetCredentialUnknownException" -> Reason.ProviderUnknown
            "GoogleIdTokenParsingException" -> Reason.TokenParsing
            "KeyPermanentlyInvalidatedException" -> Reason.KeyInvalidated
            "UserNotAuthenticatedException" -> Reason.KeyAuthenticationRequired
            "KeyStoreException", "UnrecoverableKeyException" -> Reason.Keystore
            "AEADBadTagException" -> Reason.CredentialIntegrity
            else -> Reason.OtherException
        }
    }
}

// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import java.security.MessageDigest
import javax.crypto.Cipher
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.withContext
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/** The entry is supplied verbatim by Rust's CredentialEntry; this port never derives an account. */
internal interface AccountCredentialVault {
    /** Caller owns and wipes the returned plaintext. */
    suspend fun load(entry: String): ByteArray?
    /** Completion acknowledges durable storage; the caller retains ownership of its input. */
    suspend fun store(entry: String, credential: ByteArray)
    suspend fun clear(entry: String)
}

internal enum class AccountVaultFailure(val code: String) {
    Unavailable("account_vault_unavailable"),
    InvalidPayload("account_payload_invalid"),
}

/** Deliberately has no platform exception cause, entry or credential in its diagnostic. */
internal class AccountVaultException(val failure: AccountVaultFailure) : Exception(failure.code) {
    val code: String get() = failure.code
}

/** Ciphertext only. Implementations must durably replace/remove before returning. */
internal interface AccountCiphertextStorage {
    fun read(name: String): ByteArray?
    fun contains(name: String): Boolean
    fun replace(name: String, encoded: ByteArray)
    fun remove(name: String)
}

internal interface AccountWrappingKeys {
    fun load(alias: String): SecretKey?
    fun create(alias: String): SecretKey
}

internal const val ACCOUNT_VAULT_NAMESPACE = "do.koru.oauth.refresh-"
internal const val ACCOUNT_CREDENTIAL_LIMIT = 64 * 1024
internal const val ACCOUNT_CIPHERTEXT_LIMIT = ACCOUNT_CREDENTIAL_LIMIT + 29

private const val ACCOUNT_UUID_PATTERN = "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}"
private const val ACCOUNT_SCOPE_PATTERN = "[a-z0-9-]{1,40}:[0-9a-f]{64}:[0-9a-f]{64}:$ACCOUNT_UUID_PATTERN"
private val legacyAccountEntry = Regex("v1:$ACCOUNT_SCOPE_PATTERN")
private val scopedAccountEntry = Regex("v2:$ACCOUNT_SCOPE_PATTERN:$ACCOUNT_UUID_PATTERN")

/** Preserve Rust's exact logical slot in the existing credential namespace. */
internal fun accountVaultSlot(entry: String): String {
    // v1 is at most 210 ASCII bytes; v2 adds one canonical account UUID.
    if (entry.length > 247 || (!legacyAccountEntry.matches(entry) && !scopedAccountEntry.matches(entry))) {
        throw AccountVaultException(AccountVaultFailure.InvalidPayload)
    }
    return ACCOUNT_VAULT_NAMESPACE + entry
}

/** Only v2's physical filename is shortened; legacy files never move implicitly. */
internal fun accountVaultFilename(entry: String): String {
    val alias = accountVaultSlot(entry)
    if (entry.startsWith("v1:")) return "$alias.bin"
    val digest = MessageDigest.getInstance("SHA-256").digest(entry.toByteArray(Charsets.UTF_8))
    val hash = digest.joinToString("") { (it.toInt() and 0xff).toString(16).padStart(2, '0') }
    return "${ACCOUNT_VAULT_NAMESPACE}v2-$hash.bin"
}

/** Android 11 stores escaped key aliases as filenames too. Keep v1 unchanged;
 * new v2 keys use the same bounded digest as their ciphertext filename. */
internal fun accountVaultKeyAlias(entry: String): String = accountVaultFilename(entry).removeSuffix(".bin")

/** Real AES-GCM is shared with JVM tests; only key retrieval and ciphertext I/O are injected. */
internal class WrappedAccountCredentialVault(
    private val storage: AccountCiphertextStorage,
    private val keys: AccountWrappingKeys,
) : AccountCredentialVault {
    companion object {
        // A second adapter instance cannot race key creation or replace within this process.
        private val lock = Mutex()
    }

    override suspend fun load(entry: String): ByteArray? = operation {
        val slot = accountVaultSlot(entry)
        val encoded = storage.read(slot) ?: return@operation null
        try {
            if (encoded.size !in 30..ACCOUNT_CIPHERTEXT_LIMIT || encoded[0] != 1.toByte()) {
                throw AccountVaultException(AccountVaultFailure.Unavailable)
            }
            // A missing/inaccessible key is never replaced while its ciphertext survives.
            val key = loadKey(entry, slot, hasCiphertext = true)
                ?: throw AccountVaultException(AccountVaultFailure.Unavailable)
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, encoded.copyOfRange(1, 13)))
            cipher.updateAAD(entry.toByteArray(Charsets.UTF_8))
            cipher.doFinal(encoded, 13, encoded.size - 13)
        } finally {
            encoded.fill(0)
        }
    }

    override suspend fun store(entry: String, credential: ByteArray) {
        if (credential.isEmpty() || credential.size > ACCOUNT_CREDENTIAL_LIMIT) {
            throw AccountVaultException(AccountVaultFailure.InvalidPayload)
        }
        val owned = credential.copyOf()
        try {
            operation {
                val slot = accountVaultSlot(entry)
                val hasCiphertext = storage.contains(slot)
                val key = loadKey(entry, slot, hasCiphertext) ?: if (hasCiphertext) {
                    throw AccountVaultException(AccountVaultFailure.Unavailable)
                } else {
                    keys.create(accountVaultKeyAlias(entry))
                }
                val cipher = Cipher.getInstance("AES/GCM/NoPadding")
                cipher.init(Cipher.ENCRYPT_MODE, key)
                check(cipher.iv.size == 12)
                cipher.updateAAD(entry.toByteArray(Charsets.UTF_8))
                val encrypted = cipher.doFinal(owned)
                try {
                    val encoded = byteArrayOf(1) + cipher.iv + encrypted
                    try {
                        storage.replace(slot, encoded)
                    } finally {
                        encoded.fill(0)
                    }
                } finally {
                    encrypted.fill(0)
                }
            }
        } finally {
            owned.fill(0)
        }
    }

    override suspend fun clear(entry: String) = operation {
        // Erasure does not require a functioning wrapping key. Retaining the
        // non-exportable key allows retries without inventing key rotation.
        storage.remove(accountVaultSlot(entry))
    }

    private fun loadKey(entry: String, slot: String, hasCiphertext: Boolean): SecretKey? {
        val alias = accountVaultKeyAlias(entry)
        return keys.load(alias) ?: if (hasCiphertext && alias != slot) {
            // Earlier v2 builds used the full logical alias. Reuse that key
            // for surviving ciphertext; never rotate or hide a lookup failure.
            keys.load(slot)
        } else null
    }

    private suspend fun <T> operation(block: () -> T): T =
        withContext(Dispatchers.IO + NonCancellable) {
            lock.withLock {
                try {
                    block()
                } catch (error: AccountVaultException) {
                    throw error
                } catch (_: Exception) {
                    throw AccountVaultException(AccountVaultFailure.Unavailable)
                }
            }
        }
}

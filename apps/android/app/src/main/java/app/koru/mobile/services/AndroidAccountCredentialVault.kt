// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import java.io.File
import java.io.FileOutputStream
import java.security.KeyStore
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey

/** Same Keystore wrapping mechanism as device seeds, with an independent namespace and ciphertext. */
internal fun AndroidAccountCredentialVault(context: Context): AccountCredentialVault =
    WrappedAccountCredentialVault(
        AndroidAccountCiphertextStorage(context.noBackupFilesDir),
        AndroidAccountWrappingKeys(),
    )

private class AndroidAccountWrappingKeys : AccountWrappingKeys {
    override fun load(alias: String): SecretKey? {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        val key = store.getKey(alias, null)
        if (key != null && key !is SecretKey) {
            throw AccountVaultException(AccountVaultFailure.Unavailable)
        }
        return key as? SecretKey
    }

    override fun create(alias: String): SecretKey {
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        generator.init(
            KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build(),
        )
        return generator.generateKey()
    }
}

private class AndroidAccountCiphertextStorage(private val directory: File) : AccountCiphertextStorage {
    private fun file(name: String): File {
        check(name.startsWith(ACCOUNT_VAULT_NAMESPACE))
        return directory.resolve(accountVaultFilename(name.removePrefix(ACCOUNT_VAULT_NAMESPACE)))
    }

    override fun read(name: String): ByteArray? {
        val file = file(name)
        return try {
            file.inputStream().use { stream ->
                // InputStream.readNBytes is newer than the supported phones.
                val buffer = ByteArray(ACCOUNT_CIPHERTEXT_LIMIT + 1)
                try {
                    var count = 0
                    while (count < buffer.size) {
                        val read = stream.read(buffer, count, buffer.size - count)
                        if (read < 0) break
                        count += read
                    }
                    check(count <= ACCOUNT_CIPHERTEXT_LIMIT)
                    buffer.copyOf(count)
                } finally {
                    buffer.fill(0)
                }
            }
        } catch (error: java.io.FileNotFoundException) {
            // FileNotFoundException also represents permission failures; only
            // an actual absent path is the empty-vault result.
            if (!contains(name)) null else throw error
        }
    }

    override fun contains(name: String): Boolean = try {
        Os.stat(file(name).absolutePath)
        true
    } catch (error: ErrnoException) {
        if (error.errno == OsConstants.ENOENT) false else throw error
    }

    override fun replace(name: String, encoded: ByteArray) {
        val destination = file(name)
        val temporary = directory.resolve("${destination.name}.tmp")
        try {
            FileOutputStream(temporary).use { stream ->
                stream.write(encoded)
                stream.fd.sync()
            }
            // POSIX rename replaces atomically within this same directory.
            Os.rename(temporary.absolutePath, destination.absolutePath)
            syncDirectory()
        } finally {
            temporary.delete()
        }
    }

    override fun remove(name: String) {
        val destination = file(name)
        unlinkIfPresent(directory.resolve("${destination.name}.tmp"))
        unlinkIfPresent(destination)
        // A retry after an uncertain erase still acknowledges durable absence.
        syncDirectory()
    }

    private fun unlinkIfPresent(destination: File) {
        try {
            Os.remove(destination.absolutePath)
        } catch (error: ErrnoException) {
            if (error.errno != OsConstants.ENOENT) throw error
        }
    }

    private fun syncDirectory() {
        val descriptor = Os.open(directory.absolutePath, OsConstants.O_RDONLY, 0)
        try {
            Os.fsync(descriptor)
        } finally {
            Os.close(descriptor)
        }
    }
}

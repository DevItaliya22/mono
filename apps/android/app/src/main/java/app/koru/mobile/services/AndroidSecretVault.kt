// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import android.content.Context
import java.security.KeyStore
import java.security.SecureRandom
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties

/** Android Keystore wraps the exportable Ed25519 and iroh seed bytes Rust needs. */
internal class AndroidSecretVault(private val context: Context) {
    private val alias = "koru-mobile-device-seeds-v1"
    private val file = context.noBackupFilesDir.resolve("device-seeds-v1.bin")

    fun loadOrCreate(): ByteArray {
        val key = key()
        if (file.exists()) {
            val encoded = file.readBytes()
            require(encoded.size > 12) { "invalid_device_seeds" }
            return Cipher.getInstance("AES/GCM/NoPadding").run {
                init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, encoded.copyOfRange(0, 12)))
                doFinal(encoded.copyOfRange(12, encoded.size))
            }.also { require(it.size == 64) { "invalid_device_seeds" } }
        }
        val seeds = ByteArray(64).also(SecureRandom()::nextBytes)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply { init(Cipher.ENCRYPT_MODE, key) }
        file.parentFile?.mkdirs()
        val temporary = file.resolveSibling("${file.name}.tmp")
        temporary.writeBytes(cipher.iv + cipher.doFinal(seeds))
        check(temporary.renameTo(file)) { "device_seed_commit_failed" }
        return seeds
    }

    private fun key(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(alias, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        generator.init(
            KeyGenParameterSpec.Builder(
                alias,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            ).setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build()
        )
        return generator.generateKey()
    }
}

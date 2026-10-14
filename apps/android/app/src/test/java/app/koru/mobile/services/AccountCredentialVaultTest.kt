// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import javax.crypto.Cipher
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.SecretKeySpec
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Test

class AccountCredentialVaultTest {
    private val entry = "v1:test:${"a".repeat(64)}:${"b".repeat(64)}:10000000-0000-4000-8000-000000000001"
    private val accountEntry = entry.replaceFirst("v1:", "v2:") + ":20000000-0000-4000-8000-000000000002"

    private class Storage(private val physicalFilenames: Boolean = false) : AccountCiphertextStorage {
        val values = mutableMapOf<String, ByteArray>()
        var reads = 0
        var writes = 0
        var removes = 0
        var failWrite = false
        var loseWriteAcknowledgement = false
        var failRemove = false
        private fun filename(name: String): String = if (physicalFilenames) {
            check(name.startsWith(ACCOUNT_VAULT_NAMESPACE))
            accountVaultFilename(name.removePrefix(ACCOUNT_VAULT_NAMESPACE))
        } else name
        override fun read(name: String): ByteArray? { reads += 1; return values[filename(name)]?.copyOf() }
        override fun contains(name: String): Boolean { reads += 1; return values.containsKey(filename(name)) }
        override fun replace(name: String, encoded: ByteArray) {
            if (failWrite) error("must-not-escape-fixture-secret")
            values[filename(name)] = encoded.copyOf()
            writes += 1
            if (loseWriteAcknowledgement) error("must-not-escape-fixture-secret")
        }
        override fun remove(name: String) {
            if (failRemove) error("must-not-escape-fixture-secret")
            values.remove(filename(name))
            removes += 1
        }
    }

    private class Keys(private val maxAliasLength: Int = Int.MAX_VALUE) : AccountWrappingKeys {
        val values = mutableMapOf<String, SecretKey>()
        var creates = 0
        var loads = 0
        var failLoad = false
        var failCreate = false
        override fun load(alias: String): SecretKey? {
            loads += 1
            check(alias.length <= maxAliasLength)
            if (failLoad) error("must-not-escape-fixture-secret")
            return values[alias]
        }
        override fun create(alias: String): SecretKey {
            check(alias.length <= maxAliasLength)
            if (failCreate) error("must-not-escape-fixture-secret")
            creates += 1
            return SecretKeySpec(ByteArray(32) { creates.toByte() }, "AES").also { values[alias] = it }
        }
    }

    private suspend fun unavailable(block: suspend () -> Unit) {
        try {
            block()
            fail("expected typed vault failure")
        } catch (error: AccountVaultException) {
            assertEquals("account_vault_unavailable", error.code)
            assertEquals(error.code, error.message)
            assertNull(error.cause)
            assertFalse(error.toString().contains("fixture-secret"))
        }
    }

    @Test fun exactLegacyPhysicalFixtureRemainsReadableAndRetireableBesideV2() = runBlocking {
        val storage = Storage(physicalFilenames = true)
        val keys = Keys()
        val legacyAlias = ACCOUNT_VAULT_NAMESPACE + entry
        val legacyFilename = "$legacyAlias.bin"
        val key = SecretKeySpec(ByteArray(32) { 7 }, "AES")
        keys.values[legacyAlias] = key
        // Seed the released v1 envelope directly, not through today's writer.
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        val nonce = ByteArray(12) { it.toByte() }
        cipher.init(Cipher.ENCRYPT_MODE, key, GCMParameterSpec(128, nonce))
        cipher.updateAAD(entry.toByteArray(Charsets.UTF_8))
        val credential = "fixture-legacy-refresh".toByteArray()
        storage.values[legacyFilename] = byteArrayOf(1) + nonce + cipher.doFinal(credential)
        assertEquals(legacyAlias, accountVaultSlot(entry))
        assertEquals(legacyFilename, accountVaultFilename(entry))
        val vault = WrappedAccountCredentialVault(storage, keys)
        assertArrayEquals(credential, vault.load(entry))
        assertEquals(0, keys.creates)
        vault.store(accountEntry, credential)
        assertTrue(storage.values.containsKey(legacyFilename))
        assertTrue(storage.values.containsKey(accountVaultFilename(accountEntry)))
        assertEquals(setOf(legacyAlias, accountVaultKeyAlias(accountEntry)), keys.values.keys)
        vault.clear(entry)
        vault.clear(entry)
        assertNull(WrappedAccountCredentialVault(storage, keys).load(entry))
        assertArrayEquals(credential, WrappedAccountCredentialVault(storage, keys).load(accountEntry))
        assertEquals(setOf(accountVaultFilename(accountEntry)), storage.values.keys)
    }

    @Test fun v2PhysicalNameIsPinnedBoundedAndRetainsEveryLogicalScope() {
        assertEquals(ACCOUNT_VAULT_NAMESPACE + "v2-cfb932065b9f36e352ba7ca3da3db3f2f5479c1087325db30a8e77e2ef273edc.bin", accountVaultFilename(accountEntry))
        val entries = listOf(
            accountEntry,
            accountEntry.replace(":test:", ":nightly:"),
            accountEntry.replace("a".repeat(64), "c".repeat(64)),
            accountEntry.replace("b".repeat(64), "c".repeat(64)),
            accountEntry.replace("10000000-", "30000000-"),
            accountEntry.replace("20000000-", "40000000-"),
        )
        assertEquals(entries.size, entries.map(::accountVaultFilename).toSet().size)
        entries.forEach { assertEquals(ACCOUNT_VAULT_NAMESPACE + it, accountVaultSlot(it)) }
        val longestLegacy = entry.replace(":test:", ":${"c".repeat(40)}:")
        val longestV2 = accountEntry.replace(":test:", ":${"c".repeat(40)}:")
        assertEquals(210, longestLegacy.length)
        assertEquals(247, longestV2.length)
        assertTrue((accountVaultFilename(longestLegacy) + ".tmp").length <= 255)
        assertTrue((accountVaultFilename(longestV2) + ".tmp").length <= 255)
        assertTrue((accountVaultSlot(longestV2) + ".bin.tmp").length > 255)
    }

    @Test fun newV2CredentialsWorkWithBoundedKeystoreAliasesAcrossRestartAndRotation() = runBlocking {
        val storage = Storage(physicalFilenames = true)
        // Reject the old alias on lookup as well as creation, as an older
        // Keystore can fail before the writer gets to generateKey.
        val keys = Keys(maxAliasLength = 128)
        val longest = accountEntry.replace(":test:", ":${"c".repeat(40)}:")
        val first = "fixture-first".toByteArray()
        val rotated = "fixture-rotated".toByteArray()
        WrappedAccountCredentialVault(storage, keys).store(longest, first)
        val reopened = WrappedAccountCredentialVault(storage, keys)
        assertArrayEquals(first, reopened.load(longest))
        reopened.store(longest, rotated)
        assertArrayEquals(rotated, WrappedAccountCredentialVault(storage, keys).load(longest))
        assertEquals(1, keys.creates)
        assertEquals(setOf(accountVaultKeyAlias(longest)), keys.values.keys)
        assertEquals(accountVaultSlot(entry), accountVaultKeyAlias(entry))
        assertEquals(accountVaultFilename(longest).removeSuffix(".bin"), keys.values.keys.single())
    }

    @Test fun existingV2LiteralKeyRemainsReadableAndRotatesWithoutReplacingItsKey() = runBlocking {
        val storage = Storage(physicalFilenames = true)
        val keys = Keys()
        val oldAlias = accountVaultSlot(accountEntry)
        val key = SecretKeySpec(ByteArray(32) { 7 }, "AES")
        keys.values[oldAlias] = key
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        val nonce = ByteArray(12) { it.toByte() }
        cipher.init(Cipher.ENCRYPT_MODE, key, GCMParameterSpec(128, nonce))
        cipher.updateAAD(accountEntry.toByteArray(Charsets.UTF_8))
        val first = "fixture-existing-v2".toByteArray()
        val filename = accountVaultFilename(accountEntry)
        storage.values[filename] = byteArrayOf(1) + nonce + cipher.doFinal(first)
        val vault = WrappedAccountCredentialVault(storage, keys)
        assertArrayEquals(first, vault.load(accountEntry))
        val rotated = "fixture-rotated-v2".toByteArray()
        vault.store(accountEntry, rotated)
        assertArrayEquals(rotated, WrappedAccountCredentialVault(storage, keys).load(accountEntry))
        assertEquals(0, keys.creates)
        assertEquals(setOf(oldAlias), keys.values.keys)
        val preserved = storage.values.getValue(filename).copyOf()
        keys.values.remove(oldAlias)
        unavailable { vault.load(accountEntry) }
        unavailable { vault.store(accountEntry, first) }
        assertEquals(0, keys.creates)
        assertArrayEquals(preserved, storage.values[filename])
    }

    @Test fun v2CiphertextStillAuthenticatesTheLogicalEntryAndMissingKeysNeverRotate() = runBlocking {
        val storage = Storage(physicalFilenames = true)
        val keys = Keys()
        val other = accountEntry.replace("20000000-", "30000000-")
        val vault = WrappedAccountCredentialVault(storage, keys)
        vault.store(accountEntry, "fixture-v2-refresh".toByteArray())
        val alias = accountVaultKeyAlias(accountEntry)
        val original = storage.values.getValue(accountVaultFilename(accountEntry)).copyOf()
        // Use the same key deliberately: only exact logical AAD rejects substitution.
        keys.values[accountVaultKeyAlias(other)] = keys.values.getValue(alias)
        storage.values[accountVaultFilename(other)] = original.copyOf()
        unavailable { vault.load(other) }
        keys.values.remove(alias)
        unavailable { vault.load(accountEntry) }
        unavailable { vault.store(accountEntry, "fixture-replacement".toByteArray()) }
        assertEquals(1, keys.creates)
        assertArrayEquals(original, storage.values[accountVaultFilename(accountEntry)])
        vault.clear(other)
        assertArrayEquals(original, storage.values[accountVaultFilename(accountEntry)])
        vault.clear(accountEntry)
        assertTrue(storage.values.isEmpty())
    }

    @Test fun v2ReplacementAndRetirementFailuresPreserveTheExactRecoverableSlot() = runBlocking {
        val storage = Storage(physicalFilenames = true)
        val keys = Keys()
        val vault = WrappedAccountCredentialVault(storage, keys)
        val previous = "fixture-old-refresh".toByteArray()
        val replacement = "fixture-new-refresh".toByteArray()
        vault.store(accountEntry, previous)
        storage.failWrite = true
        unavailable { vault.store(accountEntry, replacement) }
        assertArrayEquals(previous, WrappedAccountCredentialVault(storage, keys).load(accountEntry))
        storage.failWrite = false
        storage.loseWriteAcknowledgement = true
        unavailable { vault.store(accountEntry, replacement) }
        assertArrayEquals(replacement, WrappedAccountCredentialVault(storage, keys).load(accountEntry))
        storage.failRemove = true
        unavailable { vault.clear(accountEntry) }
        assertArrayEquals(replacement, vault.load(accountEntry))
        assertEquals(setOf(accountVaultFilename(accountEntry)), storage.values.keys)
    }

    @Test fun rotationSurvivesNewAdapterAndClearIsIdempotent() = runBlocking {
        val storage = Storage()
        val keys = Keys()
        val vault = WrappedAccountCredentialVault(storage, keys)
        val first = "fixture-refresh-first".toByteArray()
        val rotated = "fixture-refresh-rotated".toByteArray()
        assertNull(vault.load(entry))
        assertEquals(0, keys.creates)
        vault.store(entry, first)
        assertArrayEquals("fixture-refresh-first".toByteArray(), first)
        val firstCiphertext = storage.values.getValue(accountVaultSlot(entry)).copyOf()
        assertFalse(firstCiphertext.contentEquals(first))
        assertArrayEquals(first, vault.load(entry))
        vault.store(entry, rotated)
        assertFalse(firstCiphertext.contentEquals(storage.values.getValue(accountVaultSlot(entry))))
        assertArrayEquals(rotated, WrappedAccountCredentialVault(storage, keys).load(entry))
        assertEquals(1, keys.creates)
        vault.clear(entry)
        vault.clear(entry)
        assertNull(WrappedAccountCredentialVault(storage, keys).load(entry))
        assertTrue(storage.values.isEmpty())
    }

    @Test fun noCommittedWriteMeansNoRestoredCredentialAndLostAckCanRecoverCommittedWrite() = runBlocking {
        val storage = Storage()
        val keys = Keys()
        // Before issuance and after remote issuance without a vault store,
        // a replacement process sees the same empty local durable state.
        assertNull(WrappedAccountCredentialVault(storage, keys).load(entry))
        val issued = "fixture-issued-refresh".toByteArray()
        assertNull(WrappedAccountCredentialVault(storage, keys).load(entry))
        storage.failWrite = true
        unavailable { WrappedAccountCredentialVault(storage, keys).store(entry, issued) }
        assertNull(WrappedAccountCredentialVault(storage, keys).load(entry))
        storage.failWrite = false
        storage.loseWriteAcknowledgement = true
        unavailable { WrappedAccountCredentialVault(storage, keys).store(entry, issued) }
        assertArrayEquals(issued, WrappedAccountCredentialVault(storage, keys).load(entry))
        storage.loseWriteAcknowledgement = false
        WrappedAccountCredentialVault(storage, keys).store(entry, issued)
        assertArrayEquals(issued, WrappedAccountCredentialVault(storage, keys).load(entry))
    }

    @Test fun missingOrUnavailableKeystoreNeverReplacesTheKeyForSurvivingCiphertext() = runBlocking {
        val storage = Storage()
        val keys = Keys()
        val vault = WrappedAccountCredentialVault(storage, keys)
        vault.store(entry, "fixture-first".toByteArray())
        val slot = accountVaultSlot(entry)
        val originalCiphertext = storage.values.getValue(slot).copyOf()
        val originalKey = keys.values.remove(slot)!!
        unavailable { vault.load(entry) }
        unavailable { vault.store(entry, "fixture-rotation".toByteArray()) }
        assertEquals(1, keys.creates)
        assertArrayEquals(originalCiphertext, storage.values[slot])
        keys.values[slot] = originalKey
        keys.failLoad = true
        unavailable { vault.load(entry) }
        unavailable { vault.store(entry, "fixture-rotation".toByteArray()) }
        assertEquals(1, keys.creates)
        assertArrayEquals(originalCiphertext, storage.values[slot])
        // Logout can erase unusable ciphertext without a working Keystore.
        vault.clear(entry)
        assertTrue(storage.values.isEmpty())
    }

    @Test fun encryptionAndAtomicReplacementFailurePreserveThePreviousCredential() = runBlocking {
        val storage = Storage()
        val keys = Keys()
        val vault = WrappedAccountCredentialVault(storage, keys)
        val credential = "fixture-first".toByteArray()
        vault.store(entry, credential)
        val slot = accountVaultSlot(entry)
        val originalCiphertext = storage.values.getValue(slot).copyOf()
        val originalKey = keys.values.getValue(slot)
        keys.values[slot] = SecretKeySpec(ByteArray(4), "AES")
        unavailable { vault.store(entry, "fixture-rotation".toByteArray()) }
        assertArrayEquals(originalCiphertext, storage.values[slot])
        keys.values[slot] = originalKey
        storage.failWrite = true
        unavailable { vault.store(entry, "fixture-rotation".toByteArray()) }
        assertArrayEquals(originalCiphertext, storage.values[slot])
        assertArrayEquals(credential, vault.load(entry))
        storage.failRemove = true
        unavailable { vault.clear(entry) }
        assertArrayEquals(credential, vault.load(entry))
    }

    @Test fun namespacesUseTheExactOpaqueEntryAndCiphertextCannotMoveBetweenEntries() = runBlocking {
        val other = entry.replace("v1:test:", "v1:nightly:")
        val slot = accountVaultSlot(entry)
        val otherSlot = accountVaultSlot(other)
        assertTrue(slot.startsWith("do.koru.oauth.refresh-"))
        assertEquals(ACCOUNT_VAULT_NAMESPACE + entry, slot)
        assertNotEquals(slot, otherSlot)
        assertTrue(slot.contains("10000000-0000-4000-8000-000000000001"))
        assertFalse(slot.contains("device-seeds"))
        val storage = Storage()
        val keys = Keys()
        val vault = WrappedAccountCredentialVault(storage, keys)
        vault.store(entry, "fixture-first".toByteArray())
        // Share the key too, so this specifically exercises AAD entry binding.
        storage.values[otherSlot] = storage.values.getValue(slot).copyOf()
        keys.values[otherSlot] = keys.values.getValue(slot)
        unavailable { vault.load(other) }
        assertArrayEquals("fixture-first".toByteArray(), vault.load(entry))
    }

    @Test fun malformedCiphertextAndKeyCreationFailureHaveNoPlaintextFallback() = runBlocking {
        val storage = Storage()
        val keys = Keys()
        val vault = WrappedAccountCredentialVault(storage, keys)
        keys.failCreate = true
        unavailable { vault.store(entry, "fixture-first".toByteArray()) }
        assertTrue(storage.values.isEmpty())
        keys.failCreate = false
        vault.store(entry, "fixture-first".toByteArray())
        val slot = accountVaultSlot(entry)
        val corrupted = storage.values.getValue(slot).copyOf().also { it[it.lastIndex] = (it.last().toInt() xor 1).toByte() }
        storage.values[slot] = corrupted
        unavailable { vault.load(entry) }
        assertArrayEquals(corrupted, storage.values[slot])
        storage.values[slot] = byteArrayOf(1, 2, 3)
        unavailable { vault.load(entry) }
        assertEquals(1, keys.creates)
    }

    @Test fun invalidInputsAreRejectedBeforeAnyStorageOrKeyMutation() = runBlocking {
        val storage = Storage()
        val keys = Keys()
        val vault = WrappedAccountCredentialVault(storage, keys)
        for (invalid in listOf("", "entry\n", "x".repeat(211), "非opaque", "../entry", "path/entry", "path\\entry", "entry.name",
            entry.replaceFirst("v1:", "v3:"), entry + ":20000000-0000-4000-8000-000000000002",
            accountEntry.substringBeforeLast(':'), accountEntry + ":extra",
            entry.replace(":test:", "::"), entry.replace(":test:", ":Test:"),
            accountEntry.replace(":test:", ":${"c".repeat(41)}:"),
            entry.replace("a".repeat(64), "a".repeat(63)), entry.replace("b".repeat(64), "G".repeat(64)),
            accountEntry.replace("20000000-0000", "200000000000"), accountEntry + "\n",
        )) {
            for (operation in 0..2) {
                try {
                    when (operation) {
                        0 -> vault.load(invalid)
                        1 -> vault.store(invalid, byteArrayOf(1))
                        else -> vault.clear(invalid)
                    }
                    fail("expected invalid opaque entry")
                } catch (error: AccountVaultException) {
                    assertEquals("account_payload_invalid", error.code)
                }
            }
        }
        for (invalid in listOf(byteArrayOf(), ByteArray(ACCOUNT_CREDENTIAL_LIMIT + 1))) {
            try {
                vault.store(entry, invalid)
                fail("expected invalid credential length")
            } catch (error: AccountVaultException) {
                assertEquals("account_payload_invalid", error.code)
            }
        }
        assertEquals(0, keys.creates)
        assertEquals(0, keys.loads)
        assertEquals(0, storage.reads)
        assertEquals(0, storage.writes)
        assertEquals(0, storage.removes)
        assertTrue(storage.values.isEmpty())
    }
}

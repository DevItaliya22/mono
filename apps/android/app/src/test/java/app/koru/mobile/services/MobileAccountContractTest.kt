// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import app.koru.contract.api.v1.AccountCommandRequest
import app.koru.contract.api.v1.AccountAction
import org.junit.Assert.*
import org.junit.Test
import javax.xml.parsers.DocumentBuilderFactory
import app.koru.contract.api.v1.MobileAccountResult

class MobileAccountContractTest {
    @Test fun sharedProfileSetFixturesMatchRustValidationAndPerMemberProgress() {
        listOf("account_select_profiles_empty" to 0, "account_select_profiles_one" to 1,
            "account_select_profiles_subset" to 2).forEach { (name, size) ->
            val value = fixture("valid/$name.pb")
            MobileAccountContract.validate(value)
            assertEquals(AccountAction.ACCOUNT_ACTION_SELECT_PROFILES, value.action)
            assertEquals(size, value.profileIdsCount)
            assertEquals(9L, value.expectedOwnerScope)
        }
        listOf("account_retry_enrollment_all", "account_retry_enrollment_one").forEach { name ->
            val value = fixture("valid/$name.pb")
            MobileAccountContract.validate(value)
            assertEquals(AccountAction.ACCOUNT_ACTION_RETRY_ENROLLMENT, value.action)
            assertEquals(0, value.profileIdsCount)
        }
        listOf("account_select_profiles_duplicate", "account_select_profiles_unsorted",
            "account_select_profiles_noncanonical", "account_select_profiles_uuid_version",
            "account_select_profiles_missing_scope", "account_select_profile_legacy", "account_profile_set_wrong_action").forEach { name ->
            assertThrows(name, IllegalArgumentException::class.java) { MobileAccountContract.validate(fixture("invalid/$name.pb")) }
        }
        val result = javaClass.classLoader!!.getResourceAsStream("valid/account_profiles_selected.pb")!!.use(MobileAccountResult::parseFrom)
        val status = javaClass.classLoader!!.getResourceAsStream("valid/account_enrollment_status.pb")!!.use(app.koru.contract.api.v1.MobileAccountStatus::parseFrom)
        assertEquals(9L, result.ownerScope)
        assertEquals(status.generation, result.generation)
        assertEquals(status.enrollmentsList, result.enrollmentsList)
        assertEquals(listOf("waiting", "complete", "failed"), result.enrollmentsList.map { it.stage })
        val old = javaClass.classLoader!!.getResourceAsStream("forward_compat/account_removed_selected_field.pb")!!.use(MobileAccountResult::parseFrom)
        assertEquals("selected", old.kind)
        assertEquals(0, old.enrollmentsCount)
    }
    @Test fun setSelectionRejectsLegacyActionDuplicatesOrderAndNoncanonicalIds() {
        val a = "10000000-0000-4000-8000-000000000001"
        val b = "10000000-0000-4000-8000-000000000002"
        fun request(ids: List<String>) = AccountCommandRequest.newBuilder().setVersion(1).setRequester(7)
            .setExpectedOwnerScope(3).setAction(AccountAction.ACCOUNT_ACTION_SELECT_PROFILES).addAllProfileIds(ids).build()
        listOf(emptyList(), listOf(a), listOf(a, b)).forEach { MobileAccountContract.validate(request(it)) }
        listOf(listOf(a, a), listOf(b, a), listOf(a.replace("4000", "1000")), listOf("bad")).forEach {
            assertThrows(IllegalArgumentException::class.java) { MobileAccountContract.validate(request(it)) }
        }
        assertThrows(IllegalArgumentException::class.java) {
            MobileAccountContract.validate(request(emptyList()).toBuilder().setActionValue(6).setProfileId(a).build())
        }
        val retry = request(emptyList()).toBuilder().setAction(AccountAction.ACCOUNT_ACTION_RETRY_ENROLLMENT)
        MobileAccountContract.validate(retry.build())
        MobileAccountContract.validate(retry.setProfileId(a).build())
        assertThrows(IllegalArgumentException::class.java) { MobileAccountContract.validate(retry.addProfileIds(b).build()) }
    }
    @Test fun explicitCreationCommandsShareTheRustFixtures() {
        listOf("confirm" to AccountAction.ACCOUNT_ACTION_CONFIRM_ACCOUNT_CREATION,
            "cancel" to AccountAction.ACCOUNT_ACTION_CANCEL_ACCOUNT_CREATION).forEach { (name, action) ->
            val value = fixture("valid/account_creation_$name.pb")
            MobileAccountContract.validate(value)
            assertEquals(action, value.action)
            assertTrue(value.profileId.isEmpty())
        }
    }
    @Test fun accountKeysAreCompleteUniqueAndHaveMatchingPlaceholdersInEightLocales() {
        val locales = listOf("values", "values-ar", "values-de", "values-es", "values-fr", "values-ja", "values-pt-rBR", "values-zh-rCN")
        val sets = locales.associateWith { locale ->
            val document = javaClass.classLoader!!.getResourceAsStream("$locale/strings.xml")!!.use {
                DocumentBuilderFactory.newInstance().newDocumentBuilder().parse(it)
            }
            val strings = document.getElementsByTagName("string")
            val entries = (0 until strings.length).mapNotNull { index ->
                val node = strings.item(index)
                val key = node.attributes.getNamedItem("name").nodeValue
                if (key.startsWith("account_")) key to node.textContent else null
            }
            assertEquals("duplicate account keys in $locale", entries.size, entries.toMap().size)
            entries.toMap()
        }
        val baseline = sets.getValue("values")
        assertTrue(baseline.size >= 38)
        sets.forEach { (locale, values) ->
            assertEquals(locale, baseline.keys, values.keys)
            baseline.forEach { (key, text) ->
                val placeholders = Regex("%([0-9]+\\$)?[a-zA-Z]")
                assertEquals("$locale/$key", placeholders.findAll(text).map { it.value }.sorted().toList(),
                    placeholders.findAll(values.getValue(key)).map { it.value }.sorted().toList())
                assertTrue("$locale/$key", values.getValue(key).isNotBlank())
            }
        }
    }

    @Test fun sharedEmptyAccountFixturePreservesAccountWithoutInventingAProfile() {
        val result = javaClass.classLoader!!.getResourceAsStream("valid/account_empty_profiles.pb")!!.use(MobileAccountResult::parseFrom)
        assertTrue(result.hasAccount())
        assertEquals("Existing account", result.account.displayName)
        assertEquals(0, result.profilesCount)
    }
    @Test fun accountLabelExistsInEverySupportedLanguage() {
        listOf("values", "values-ar", "values-de", "values-es", "values-fr", "values-ja", "values-pt-rBR", "values-zh-rCN").forEach {
            val xml = javaClass.classLoader!!.getResourceAsStream("$it/strings.xml")!!.bufferedReader().use { reader -> reader.readText() }
            assertTrue(it, xml.contains("name=\"account_worker\""))
        }
    }
    private fun fixture(path: String) = javaClass.classLoader!!.getResourceAsStream(path)!!.use(AccountCommandRequest::parseFrom)
    @Test fun sameRustFixturesAcceptFutureFieldsAndRejectUnknownCommands() {
        listOf("valid/account_start.pb", "forward_compat/account_start_unknown_field.pb").forEach {
            val value = fixture(it)
            MobileAccountContract.validate(value)
            assertEquals(7L, value.requester)
        }
        listOf("invalid/account_unknown_action.pb", "invalid/account_missing_requester.pb").forEach {
            assertThrows(IllegalArgumentException::class.java) { MobileAccountContract.validate(fixture(it)) }
        }
    }
}

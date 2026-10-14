// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.ui

import app.koru.mobile.services.LinkedDevice
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import javax.xml.parsers.DocumentBuilderFactory

class LinkedDevicePresentationTest {
    private fun device(id: String, name: String) = LinkedDevice(
        deviceId = id,
        displayName = name,
        linkedAtMillis = 0,
        connected = false,
        lastSeenMillis = null,
    )

    @Test fun sameNameWithDistinctIdentityIsShownAsAnotherInstallation() {
        val current = device("new-android", "Android")
        assertTrue(isAnotherInstallation(current, device("old-android", "Android")))
        assertFalse(isAnotherInstallation(current, current))
        assertFalse(isAnotherInstallation(current, device("phone", "Pixel")))
    }

    @Test fun identityAndCleanupLabelsAlignAcrossAllLocales() {
        val locales = listOf(
            "values", "values-ar", "values-de", "values-es",
            "values-fr", "values-ja", "values-pt-rBR", "values-zh-rCN",
        )
        val expected = setOf("device_another_installation", "device_via_unlink",
            "relink_devices_waiting_state", "relink_devices_loading")
        val placeholders = Regex("%([0-9]+\\$)?[a-zA-Z]")
        val values = locales.associateWith { locale ->
            val document = javaClass.classLoader!!.getResourceAsStream("$locale/strings.xml")!!.use {
                DocumentBuilderFactory.newInstance().newDocumentBuilder().parse(it)
            }
            val strings = document.getElementsByTagName("string")
            (0 until strings.length).mapNotNull { index ->
                val node = strings.item(index)
                val key = node.attributes.getNamedItem("name").nodeValue
                if (key in expected) key to node.textContent else null
            }.toMap()
        }
        val baseline = values.getValue("values")
        assertEquals("Waiting to relink", baseline.getValue("relink_devices_waiting_state"))
        values.forEach { (locale, localized) ->
            assertEquals(locale, expected, localized.keys)
            expected.forEach { key ->
                assertEquals(
                    "$locale/$key",
                    placeholders.findAll(baseline.getValue(key)).map { it.value }.sorted().toList(),
                    placeholders.findAll(localized.getValue(key)).map { it.value }.sorted().toList(),
                )
                assertTrue("$locale/$key", localized.getValue(key).isNotBlank())
            }
        }
    }
}

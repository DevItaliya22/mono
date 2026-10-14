// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import org.junit.Assert.*
import org.junit.Test

/**
 * The JSON the native host hands over is decoded field by field here, which
 * is exactly where a new field goes missing without anyone noticing — it
 * happened to `introduced_by` once. Every field the list shows is pinned.
 */
class NativeDeviceSnapshotTest {
    @Test
    fun local_account_binding_survives_both_inventory_paths_and_domain_projection() {
        for (binding in listOf("", "koru:10000000-0000-4000-8000-000000000001", " malformed ")) {
            val json = parseInventory(
                """{"profiles":[{"id":"profile","name":"Local","locale":"en","account_binding":"$binding"}],"workspaces":[],"documents":[]}"""
            )
            val wire = app.koru.contract.api.v1.Inventory.newBuilder().addProfiles(
                app.koru.contract.api.v1.InventoryProfile.newBuilder()
                    .setId("profile").setName("Local").setLocale("en")
                    .setDisplayName("Local").setAccountBinding(binding)
            ).build()
            val decoded = app.koru.contract.api.v1.Inventory.parseFrom(wire.toByteArray()).toInventory()
            for (profile in listOf(json.profiles.single(), decoded.profiles.single())) {
                assertEquals(binding, profile.accountBinding)
                assertEquals(binding, profile.toProfile().accountBinding)
                assertEquals("Local", profile.toProfile().displayName)
            }
        }
    }

    @Test
    fun profile_appearance_survives_the_native_inventory_json() {
        val inventory = parseInventory(
            """
            {
              "profiles": [
                {
                  "id": "profile",
                  "name": "Lab",
                  "locale": "en",
                  "icon": "🧪",
                  "leading_emoji": "🧪",
                  "display_name": "Lab",
                  "color": "purple"
                }
              ],
              "workspaces": [],
              "documents": []
            }
            """.trimIndent()
        )

        assertEquals("🧪", inventory.profiles.single().icon)
        assertEquals("purple", inventory.profiles.single().color)

        val legacy = parseInventory(
            """{"profiles":[{"id":"old","name":"Old","locale":"en","leading_emoji":"O","display_name":"Old","color":null}],"workspaces":[],"documents":[]}"""
        )
        assertNull(legacy.profiles.single().icon)
    }

    @Test
    fun every_field_the_device_list_shows_survives_the_json_boundary() {
        val snapshot = parseDevices(
            """
            {
              "devices": [
                {
                  "device_id": "phone",
                  "display_name": "Pixel",
                  "linked_at_millis": 5,
                  "connected": true,
                  "last_seen_millis": 9,
                  "introduced_by": "Laptop",
                  "build_label": "0.0.4 · nightly · Windows 11",
                  "build_drift": "incompatible",
                  "build_severity": "danger",
                  "update_needed_on": "peer",
                  "connection": "relay",
                  "last_sync_millis": 20,
                  "history": []
                },
                {
                  "device_id": "old",
                  "display_name": "Older build",
                  "linked_at_millis": 5,
                  "connected": false,
                  "last_seen_millis": null
                }
              ],
              "this_device": {
                "device_id": "me",
                "display_name": "Tablet",
                "linked_at_millis": 1,
                "connected": true,
                "last_seen_millis": null,
                "build_label": "0.1.0 (1) · debug · Android 14",
                "build_drift": null
              },
              "sync_event": null,
              "relink_available": true
            }
            """.trimIndent()
        )

        val pixel = snapshot.devices[0]
        assertEquals("Laptop", pixel.introducedBy)
        assertEquals("0.0.4 · nightly · Windows 11", pixel.buildLabel)
        assertEquals("incompatible", pixel.buildDrift)
        assertEquals("danger", pixel.buildSeverity)
        assertEquals("peer", pixel.updateNeededOn)
        assertEquals("relay", pixel.connection)
        assertEquals(20L, pixel.lastSyncMillis)
        assertTrue(pixel.history.isEmpty())

        // A device from before build records says nothing, and is not marked.
        val older = snapshot.devices[1]
        assertNull(older.buildLabel)
        assertNull(older.buildDrift)
        assertNull(older.buildSeverity)
        assertNull(older.updateNeededOn)
        assertNull(older.connection)
        assertNull(older.lastSeenMillis)

        assertEquals("0.1.0 (1) · debug · Android 14", snapshot.thisDevice?.buildLabel)
        assertNull(snapshot.syncEvent)
        assertTrue(snapshot.relinkAvailable)
        assertFalse(parseDevices("""{"devices":[]}""").relinkAvailable)
    }

    @Test
    fun the_relay_status_survives_the_json_boundary() {
        val connected = parseDevices(
            """{"devices":[],"relay":{"state":"connected","host":"use1-1.relay.koru.do"}}"""
        )
        assertEquals(RelayStatus(RelayState.Connected, "use1-1.relay.koru.do"), connected.relay)

        val off = parseDevices("""{"devices":[],"relay":{"state":"off","host":""}}""")
        assertEquals(RelayStatus(RelayState.Off, ""), off.relay)

        // A state word this build does not know, and a host from before the
        // field existed, both show nothing rather than a guess.
        assertNull(parseDevices("""{"devices":[],"relay":{"state":"someday-new","host":"h"}}""").relay)
        assertNull(parseDevices("""{"devices":[]}""").relay)
    }

    @Test
    fun device_history_pages_keep_the_cursor_and_document_metadata() {
        val page = parseDeviceHistory(
            """
            {
              "device_id": "phone",
              "events": [{
                "event_id": 91,
                "at_millis": 20,
                "kind": "document_updated",
                "profile_id": null,
                "workspace_id": "workspace",
                "document_id": "document",
                "document_name": "Roadmap",
                "blob_hash": null,
                "detail": null,
                "items": 3
              }],
              "next_before": 42
            }
            """.trimIndent()
        )

        assertEquals("phone", page.deviceId)
        assertEquals(42L, page.nextBefore)
        assertEquals(91L, page.events.single().eventId)
        assertEquals("Roadmap", page.events.single().documentName)
        assertEquals(3, page.events.single().items)
    }
}

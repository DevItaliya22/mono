// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * A picture load that produced no bytes: which codes are a wait and which are
 * damage. The placeholder the editor draws is chosen from this state, so a
 * peer that lacks the bytes must never reach it as [BlobLoadState.Failed] —
 * that is the "Image unavailable" a person read over a file their desktop
 * held all along.
 */
class BlobLoadFailureTest {
    @Test
    fun `a peer that lacks the bytes is a wait naming nobody`() {
        assertEquals(BlobLoadState.WaitingForPeer(emptyList()), blobLoadFailure("blob_not_found"))
    }

    @Test
    fun `a parked hydration names the devices it waits on, each once`() {
        assertEquals(
            BlobLoadState.WaitingForPeer(listOf("ThinkpadP11", "Desktop")),
            blobLoadFailure("blob_waiting_for_peer", listOf("ThinkpadP11", "Desktop", "ThinkpadP11")),
        )
        assertEquals(
            BlobLoadState.WaitingForPeer(listOf("ThinkpadP11")),
            blobLoadFailure("blob_not_found", listOf("ThinkpadP11")),
        )
    }

    @Test
    fun `every not-held code is a wait`() {
        BLOB_NOT_HELD_CODES.forEach { code ->
            assertEquals(code, BlobLoadState.WaitingForPeer(emptyList()), blobLoadFailure(code))
        }
    }

    @Test
    fun `nothing linked keeps its own remedy`() {
        assertEquals(BlobLoadState.NoLinkedDevices, blobLoadFailure("no_linked_devices"))
    }

    @Test
    fun `genuine failures stay failures`() {
        listOf(
            "blob_hash_mismatch",
            "blob_length_mismatch",
            "blob_metadata_changed",
            "blob_too_large",
            "unauthorized_workspace",
            "forbidden",
            "relay_path_refused",
            "unexpected_blob_frame",
            "blob_load_failed",
        ).forEach { code ->
            assertEquals(code, BlobLoadState.Failed(code, retryable = false), blobLoadFailure(code))
        }
    }

    @Test
    fun `a transient failure stays retryable`() {
        listOf("timeout", "runtime_closed", "store_unavailable", "internal").forEach { code ->
            assertEquals(code, BlobLoadState.Failed(code, retryable = true), blobLoadFailure(code))
        }
    }
}

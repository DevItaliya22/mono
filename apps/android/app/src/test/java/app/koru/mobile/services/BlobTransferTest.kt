// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import app.koru.contract.api.v1.BlobTransferStatus
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The phone's half of `cloud-blob-object-v1` §10, tested where it is a pure
 * function: the code a pass reports becomes the sentence a person reads, and
 * the network becomes the policy the shared engine is handed.
 *
 * No `Context`, no emulator, no shim — the Android halves of both are one
 * callback and one call each, and everything that decides anything is here.
 */
class BlobTransferTest {
    /**
     * **Free up space is offered over a real tally and nothing else.** A held
     * pass leaves the counts at zero before it asked the Store about a single
     * file, and a button over "— on this device" would promise something
     * nobody measured.
     */
    @Test
    fun free_up_space_is_offered_only_over_resolved_counts_that_hold_something() {
        val holding = blobTransferReport(snapshot(referenced = 3, present = 2))
        assertTrue(canClearLocal(holding))
        val empty = blobTransferReport(snapshot(referenced = 3, present = 0))
        assertFalse(canClearLocal(empty))
        val held = blobTransferReport(snapshot(pendingReason = "network_metered", referenced = 3, present = 2))
        assertFalse(canClearLocal(held))
    }


    /**
     * **§13, pinned on this platform too.** A metered pause and a transport-mode
     * refusal must not arrive at a person as the same thing: *"a silent pause
     * is indistinguishable from being refused, and that has already cost this
     * project a debugging session"*.
     */
    @Test
    fun a_pause_and_a_refusal_are_different_things_to_be_told() {
        val paused = snapshot(pendingReason = "network_metered", referenced = 3)
        val refused = snapshot(pendingReason = "mode_refuses_transport", referenced = 3)

        assertEquals(BlobTransferPhase.Paused, paused.phase)
        assertEquals(BlobTransferPhase.Refused, refused.phase)
        assertNotEquals(paused.phase, refused.phase)
    }

    /** Every pause the engine can report is read as one, and nothing else is. */
    @Test
    fun every_pause_code_is_read_as_a_pause() {
        for (code in BLOB_PAUSE_CODES) {
            assertEquals("$code should pause", BlobTransferPhase.Paused, phaseOf(code, 1, 0))
        }
        assertEquals(BlobTransferPhase.Refused, phaseOf(BLOB_MODE_REFUSED, 1, 0))
        assertEquals(BlobTransferPhase.Failed, phaseOf(BLOB_NO_ROOM, 1, 0))
        assertEquals(BlobTransferPhase.Failed, phaseOf("hash_mismatch", 1, 0))
    }

    /**
     * **A file no device has uploaded yet is a wait, not a failure.**
     *
     * Cloud answering `object_missing` for bytes it has never held is correct
     * while the device holding them is shut. It fell through to [Failed] on
     * the first hardware run, where a Workspace referencing six files of which
     * Cloud held two was painted in the error colour and read as damage to
     * somebody's pictures. It is not a pause either: no switch on this phone
     * clears it.
     */
    @Test
    fun bytes_no_device_has_uploaded_yet_are_a_wait_and_not_a_failure() {
        assertEquals(BlobTransferPhase.Waiting, phaseOf(BLOB_NOT_UPLOADED_YET, 3, 0))
        assertNotEquals(BlobTransferPhase.Failed, phaseOf(BLOB_NOT_UPLOADED_YET, 3, 0))
        assertNotEquals(BlobTransferPhase.Paused, phaseOf(BLOB_NOT_UPLOADED_YET, 3, 0))
        assertNull(
            "no hold, because nothing on this device clears it",
            BlobHold.of(BLOB_NOT_UPLOADED_YET),
        )
        // No verbatim code reaches the screen: `failureCode` is what the panel
        // falls back to, and a waiting Workspace has not failed.
        val report = blobTransferReport(
            snapshot(pendingReason = BLOB_NOT_UPLOADED_YET, referenced = 6),
        )
        assertEquals(BlobTransferPhase.Waiting, report.phase)
        assertEquals("", report.failureCode)
        assertTrue("the counts were resolved by the walk", report.countsResolved)
    }

    /** Nothing pending is idle **whatever is outstanding** (D260): files this
     *  phone does not hold are not on their way until something asks, so a
     *  card that said "Moving files…" over them would describe a transfer
     *  that is not happening. Work is what is in flight. */
    @Test
    fun an_empty_pending_reason_reads_as_work_only_while_something_is_in_flight() {
        assertEquals(BlobTransferPhase.Idle, phaseOf("", 0, 0))
        assertEquals(BlobTransferPhase.Idle, phaseOf("", 4, 0))
        assertEquals(BlobTransferPhase.Working, phaseOf("", 4, 1))
    }

    /** §10's counts arrive as they were sent, and `outstanding` is the one
     *  derived number: what a person is still waiting for. */
    @Test
    fun the_status_message_maps_field_for_field() {
        val status = BlobTransferStatus.newBuilder()
            .setWorkspaceId("w1")
            .setReferenced(9)
            .setPresent(4)
            .setDurable(3)
            .setUploading(1)
            .setDownloading(2)
            .setMissing(5)
            .setBytesComplete(1_024)
            .setBytesTotal(4_096)
            .setPendingReason("network_metered")
            .build()
        val snapshot = BlobTransferSnapshot.from(status)
        assertEquals("w1", snapshot.workspaceId)
        assertEquals(9, snapshot.referenced)
        assertEquals(4, snapshot.present)
        assertEquals(3, snapshot.durable)
        assertEquals(1, snapshot.uploading)
        assertEquals(2, snapshot.downloading)
        assertEquals(5, snapshot.missing)
        assertEquals(1_024L, snapshot.bytesComplete)
        assertEquals(4_096L, snapshot.bytesTotal)
        assertEquals(5, snapshot.outstanding)
        assertEquals("", snapshot.lastFailureCode)
        assertEquals(BlobTransferPhase.Paused, snapshot.phase)
    }

    /** A present count above the referenced one is somebody else's bug and
     *  must not become a negative number on a screen. */
    @Test
    fun outstanding_never_goes_below_zero() {
        assertEquals(0, snapshot(referenced = 2, present = 5).outstanding)
    }

    /**
     * **D50, as four booleans.** The engine decides what a metered connection
     * means; this only has to report the connection truthfully, including the
     * person's own allowance.
     */
    @Test
    fun the_policy_carries_the_connection_and_the_persons_answer() {
        val wifi = blobPolicyOf(connected = true, metered = false, dataSaver = false, allowOnMetered = false)
        assertFalse(wifi.offline)
        assertFalse(wifi.metered)
        assertFalse(wifi.lowData)

        val cellular = blobPolicyOf(connected = true, metered = true, dataSaver = false, allowOnMetered = false)
        assertTrue(cellular.metered)
        assertFalse(cellular.allowOnMetered)

        val allowed = blobPolicyOf(connected = true, metered = true, dataSaver = false, allowOnMetered = true)
        assertTrue(allowed.metered)
        assertTrue("the person's answer has to reach the engine", allowed.allowOnMetered)

        val saver = blobPolicyOf(connected = true, metered = false, dataSaver = true, allowOnMetered = false)
        assertTrue(saver.lowData)

        val none = blobPolicyOf(connected = false, metered = false, dataSaver = false, allowOnMetered = false)
        assertTrue("no network is reported as offline, not as unmetered", none.offline)
    }

    /**
     * **Documents keep syncing** (D50), read from the other end: the policy a
     * person's connection produces is *only* about that connection. Nothing
     * this app can set holds a document back, and the engine reads this value
     * through the blob policy port alone — the promise is structural rather
     * than a habit, and `koru-mobile-host`'s own tests pin the other half.
     */
    @Test
    fun the_worst_connection_still_leaves_every_document_field_untouched() {
        val worst = blobPolicyOf(
            connected = true,
            metered = true,
            dataSaver = true,
            allowOnMetered = false,
        )
        // Four booleans, and all four are about the connection.
        assertTrue(worst.metered)
        assertTrue(worst.lowData)
        assertFalse(worst.offline)
        assertFalse(worst.allowOnMetered)
        // The phase a Workspace reports under it is a pause — something the
        // world clears — and never a refusal, which is what a person would
        // have to act on.
        assertEquals(BlobTransferPhase.Paused, phaseOf("network_metered", 3, 0))
        assertEquals(BlobTransferPhase.Paused, phaseOf("network_low_data", 3, 0))
    }

    /**
     * **D259, as the arithmetic that produced it.** A file this device holds
     * and is uploading is `referenced == present`, so `outstanding` is zero
     * for the whole of a 1.9 GiB upload. Reading the phase from `outstanding`
     * alone therefore answered `Idle` — and the surface above it said "up to
     * date" over something mid-flight.
     */
    @Test
    fun an_upload_of_something_already_here_is_still_work() {
        assertEquals(BlobTransferPhase.Working, phaseOf("", 0, 1))
        val uploading = BlobTransferSnapshot(
            workspaceId = "w1",
            referenced = 6,
            present = 6,
            durable = 5,
            uploading = 1,
            current = true,
            coverageState = "complete",
        )
        assertEquals(0, uploading.outstanding)
        assertEquals(1, uploading.moving)
        assertEquals(BlobTransferPhase.Working, uploading.phase)
        assertFalse(
            "up to date was claimed over a file in flight",
            blobTransferReport(uploading).upToDate,
        )
    }

    /**
     * **A file is counted exactly like a picture** (D259). The engine's walk
     * never consults a media type, and neither does anything here — so the
     * only way a surface could exclude one is by inventing a filter, which is
     * what this refuses to let happen silently: an octet-stream attachment and
     * a PNG produce the same report.
     */
    @Test
    fun a_file_counts_the_same_as_a_picture() {
        val pictures = BlobTransferSnapshot(workspaceId = "w1", referenced = 5, present = 5, durable = 5)
        val withAnArchive = pictures.copy(referenced = 6, present = 6, durable = 5, uploading = 1)
        assertEquals(6, blobTransferReport(withAnArchive).referenced)
        assertEquals(1, blobTransferReport(withAnArchive).moving)
        assertEquals(
            "the report has nowhere to learn a media type, and must not grow one",
            blobTransferReport(withAnArchive).onCloud,
            5,
        )
        assertEquals(BlobTransferPhase.Working, withAnArchive.phase)
    }

    /**
     * **Up to date is the engine's claim plus two facts a claim cannot have.**
     * A coverage claim is computed per pass; a file that started moving after
     * it was taken is one it does not know about. All three must hold.
     */
    @Test
    fun up_to_date_needs_the_claim_and_a_quiet_engine() {
        val settled = BlobTransferSnapshot(
            workspaceId = "w1", referenced = 4, present = 4, durable = 4,
            coverageState = "complete", current = true,
        )
        assertTrue(blobTransferReport(settled).upToDate)
        // No claim yet is not the same as nothing being owed.
        assertFalse(blobTransferReport(settled.copy(current = false)).upToDate)
        // Files this phone does not hold are not "still to fetch" (D260): up to
        // date is about Koru Cloud, and two of four sitting only there is the
        // ordinary state of a phone that has not opened them.
        assertTrue(blobTransferReport(settled.copy(present = 2)).upToDate)
        // Something in flight, claim or no claim.
        assertFalse(blobTransferReport(settled.copy(downloading = 1)).upToDate)
        assertFalse(blobTransferReport(settled.copy(uploading = 1)).upToDate)
    }

    /**
     * **Five holds, five sentences** (plan §13). The screen chooses a resource
     * from [BlobHold], so two holds sharing an identity here would be two
     * holds sharing a sentence there — and "waiting for Wi-Fi" reading as
     * "this will never arrive" is the defect §13 exists for.
     */
    @Test
    fun every_hold_is_its_own_thing_to_be_told() {
        val codes = listOf(
            "network_metered", "network_low_data", "network_offline",
            "paused_by_person", "mode_refuses_transport",
        )
        val holds = codes.map { BlobHold.of(it) }
        assertEquals("a hold the engine reports has no identity here", codes.size, holds.filterNotNull().size)
        assertEquals("two holds share one identity", codes.size, holds.toSet().size)

        // The person's pause is a pause, not a failure: nothing went wrong.
        val paused = snapshot(pendingReason = "paused_by_person", referenced = 3)
        assertEquals(BlobTransferPhase.Paused, paused.phase)
        assertEquals("", blobTransferReport(paused).failureCode)
        // …and not the refusal, which no change of network or switch clears.
        assertEquals(
            BlobTransferPhase.Refused,
            snapshot(pendingReason = "mode_refuses_transport", referenced = 3).phase,
        )

        // D50's switch answers the two paid-connection holds and nothing else.
        for (code in listOf("network_metered", "network_low_data")) {
            assertTrue(code, blobTransferReport(snapshot(code, 3)).cellularWouldHelp)
        }
        for (code in listOf("network_offline", "paused_by_person", "mode_refuses_transport")) {
            assertFalse(
                "$code was offered a switch that cannot clear it",
                blobTransferReport(snapshot(code, 3)).cellularWouldHelp,
            )
        }
    }

    /** A Workspace with no files and nothing happening has nothing to say, and
     *  a card saying so is a card in the way. */
    @Test
    fun a_workspace_with_no_files_reports_nothing() {
        assertFalse(blobTransferReport(snapshot()).worthShowing)
        assertTrue(blobTransferReport(snapshot(referenced = 1)).worthShowing)
        // …but a hold is always worth saying, even with nothing counted yet.
        assertTrue(blobTransferReport(snapshot(pendingReason = "network_offline")).worthShowing)
    }

    private fun snapshot(
        pendingReason: String = "",
        referenced: Int = 0,
        present: Int = 0,
    ) = BlobTransferSnapshot(
        workspaceId = "w1",
        referenced = referenced,
        present = present,
        pendingReason = pendingReason,
    )
}

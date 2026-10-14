// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import app.koru.contract.api.v1.HostStatus
import app.koru.contract.api.v1.LinkDeviceResult
import org.junit.Assert.*
import org.junit.Test

/**
 * The one device state nobody can reach on purpose.
 *
 * When the seeds this phone holds stop producing the keys its stored identity
 * names, the native host keeps serving content and refuses everything that
 * needs a signature. That is `device_identity_mismatch`, and it was once
 * mistaken for data loss because the screen said the native library was
 * missing instead — a lie about the cause, in front of workspaces that were
 * all still there.
 *
 * The mismatch is pinned here rather than in a device test because there is no
 * way to arrive at it by hand: it needs a store written by one set of seeds
 * and opened with another. What is checked is the decision the app makes from
 * what the host says, which is the half that was wrong.
 */
class DeviceIdentityMismatchTest {
    private fun status(state: String, errorCode: String? = null) = HostStatus.newBuilder()
        .setState(state).apply { errorCode?.let(::setErrorCode) }.build().toState()

    private fun link(state: String, errorCode: String? = null) = LinkDeviceResult.newBuilder()
        .setState(state).apply { errorCode?.let(::setErrorCode) }.build().toState()

    /**
     * The host answers the mismatch under two different states — `status` says
     * `unavailable`, `link_device` says `failed` — and both mean the identity
     * could not be opened. Reading the *state* rather than the code is how the
     * banner becomes unreachable on the polled path, which is the only path a
     * person who has not touched anything is on.
     */
    @Test
    fun the_mismatch_is_read_from_the_code_under_either_state() {
        assertEquals(
            DeviceLinkState.IdentityMismatch,
            status("unavailable", DEVICE_IDENTITY_MISMATCH),
        )
        assertEquals(
            DeviceLinkState.IdentityMismatch,
            link("failed", DEVICE_IDENTITY_MISMATCH),
        )
    }

    /** It must never arrive as the reason string of a generic bucket: that
     *  bucket is what draws "this build does not include the native sync
     *  library", which is the sentence this whole state exists to stop. */
    @Test
    fun a_mismatch_is_never_reported_as_an_unavailable_reason() {
        val state = status("unavailable", DEVICE_IDENTITY_MISMATCH)
        assertFalse(state.toString(), state is DeviceLinkState.Unavailable)
        assertFalse(state.toString(), state is DeviceLinkState.Failed)
    }

    /** Every other code keeps the meaning it had. A missing library and an
     *  unreadable status are still unavailable; a refused link is still a
     *  failure carrying its own code. */
    @Test
    fun no_other_status_changed_meaning() {
        assertEquals(DeviceLinkState.Unlinked, status("unlinked"))
        assertEquals(DeviceLinkState.Linked(null), status("linked"))
        assertEquals(DeviceLinkState.Unavailable("native_status"), status("unavailable"))
        assertEquals(
            DeviceLinkState.Unavailable("store_worker_unavailable"),
            status("unavailable", "store_worker_unavailable"),
        )
        assertEquals(
            DeviceLinkState.Failed("invalid_link_code"),
            link("failed", "invalid_link_code"),
        )
        assertEquals(DeviceLinkState.Failed("pairing_failed"), link("failed"))
    }

    /**
     * `identity_intact` is the host refusing to reset a healthy device. It is
     * the answer to an action, never a state, so it must not be mistaken for
     * the mismatch and put the banner back on a device that is fine.
     */
    @Test
    fun a_healthy_devices_refusal_is_not_the_mismatch() {
        assertEquals(
            DeviceLinkState.Unavailable("identity_intact"),
            status("unavailable", "identity_intact"),
        )
    }

    /**
     * Linking cannot succeed in the mismatch state — the host refuses `link`
     * and `start_pairing` with the same code — so the screen does not offer
     * it. Before the banner existed the mismatch was an `Unavailable`, which
     * already disabled these; keeping them disabled is what stops the fix from
     * being a new dead end.
     */
    @Test
    fun linking_is_not_offered_while_the_identity_is_unusable() {
        assertTrue(DeviceLinkState.IdentityMismatch.linkingBlocked)
        assertTrue(DeviceLinkState.Unavailable("native_library_missing").linkingBlocked)
        assertTrue(DeviceLinkState.Linking().linkingBlocked)
        assertFalse(DeviceLinkState.Unlinked.linkingBlocked)
        assertFalse(DeviceLinkState.Linked("Desktop").linkingBlocked)
        // A failed attempt is worth another try: the code may have been typed
        // wrong, or the invitation may have expired while it was being read.
        assertFalse(DeviceLinkState.Failed("invalid_link_code").linkingBlocked)
    }
}

// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import app.koru.mobile.domain.ProfileId

enum class ProfileSelectionMode { One, Several, All }

/** Unsubmitted view state only. All captures this authorization snapshot;
 * later grants never join a selection without another explicit choice. */
data class ProfileSelectionDraft(
    val mode: ProfileSelectionMode = ProfileSelectionMode.One,
    val ids: Set<ProfileId> = emptySet(),
) {
    fun reconcile(authorized: Set<ProfileId>): ProfileSelectionDraft {
        val retained = ids intersect authorized
        return copy(ids = retained, mode = if (mode == ProfileSelectionMode.All && retained != authorized)
            ProfileSelectionMode.Several else mode)
    }

    fun choose(mode: ProfileSelectionMode, authorized: Set<ProfileId>): ProfileSelectionDraft {
        val retained = reconcile(authorized).ids
        return ProfileSelectionDraft(mode, when (mode) {
            ProfileSelectionMode.One -> retained.sortedBy { it.value }.take(1).toSet()
            ProfileSelectionMode.Several -> retained
            ProfileSelectionMode.All -> authorized.toSet()
        })
    }

    fun toggle(id: ProfileId, authorized: Set<ProfileId>): ProfileSelectionDraft {
        val current = reconcile(authorized)
        if (id !in authorized) return current
        if (mode == ProfileSelectionMode.One) return copy(ids = setOf(id))
        return ProfileSelectionDraft(ProfileSelectionMode.Several,
            if (id in current.ids) current.ids - id else current.ids + id)
    }

    fun submission(authorized: Set<ProfileId>): List<ProfileId> = (ids intersect authorized).sortedBy { it.value }
}

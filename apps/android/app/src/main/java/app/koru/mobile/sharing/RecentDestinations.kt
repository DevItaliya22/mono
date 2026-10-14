// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.sharing

import android.content.Context
import app.koru.mobile.domain.ProfileId
import app.koru.mobile.domain.WorkspaceId
import org.json.JSONArray
import org.json.JSONObject

/** One workspace a share was saved into. Ordering is insertion order. */
data class RecentDestination(
    val profileId: ProfileId,
    val workspaceId: WorkspaceId,
)

internal const val RECENT_DESTINATION_LIMIT = 3

/**
 * The last few share destinations on this device. "Save to Koru" leads with
 * them, because the workspace someone shared into is the one they will most
 * likely share into next. SharedPreferences holds the JSON, the way the
 * `appearance` and `device` stores hold theirs; the merge rules (cap, dedup,
 * order) are pure functions below, pinned by the JVM tests.
 */
class RecentDestinations(context: Context) {
    private val store = context.getSharedPreferences(StoreName, Context.MODE_PRIVATE)

    fun load(): List<RecentDestination> = decodeRecentDestinations(store.getString(Key, null))

    fun record(
        profileId: ProfileId,
        workspaceId: WorkspaceId,
    ) {
        val next = recordedRecentDestination(load(), RecentDestination(profileId, workspaceId))
        store.edit().putString(Key, encodeRecentDestinations(next)).apply()
    }

    private companion object {
        const val StoreName = "sharing"
        const val Key = "recent_destinations"
    }
}

/** The merge: the new entry leads, recording an existing (profile, workspace)
 *  pair moves it to the front, and the list never exceeds the cap. */
internal fun recordedRecentDestination(
    entries: List<RecentDestination>,
    entry: RecentDestination,
): List<RecentDestination> =
    (listOf(entry) + entries.filter {
        it.profileId != entry.profileId || it.workspaceId != entry.workspaceId
    }).take(RECENT_DESTINATION_LIMIT)

internal fun encodeRecentDestinations(entries: List<RecentDestination>): String {
    val array = JSONArray()
    entries.forEach { entry ->
        array.put(
            JSONObject()
                .put("profile_id", entry.profileId.value)
                .put("workspace_id", entry.workspaceId.value),
        )
    }
    return array.toString()
}

/** A store that does not decode reads as empty — a damaged value costs the
 *  recents list, never the share. */
internal fun decodeRecentDestinations(value: String?): List<RecentDestination> {
    if (value.isNullOrBlank()) return emptyList()
    return runCatching {
        val array = JSONArray(value)
        List(array.length()) { index ->
            array.getJSONObject(index).let {
                RecentDestination(
                    ProfileId(it.getString("profile_id")),
                    WorkspaceId(it.getString("workspace_id")),
                )
            }
        }
    }.getOrDefault(emptyList())
}

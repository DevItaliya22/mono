// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.domain

import java.util.UUID

data class VisibleProfiles(val rows: List<Profile>, val hiddenCount: Int)

/** Canonical attribution only. Nonempty malformed data is never local-only. */
fun Profile.boundAccountId(): String? {
    if (!accountBinding.startsWith("koru:")) return null
    val value = accountBinding.removePrefix("koru:")
    return runCatching { UUID.fromString(value) }.getOrNull()
        ?.takeIf { it.version() == 4 && it.variant() == 2 && it.toString() == value }
        ?.toString()
}

/** A projection of already usable local rows; no access or Store mutation. */
fun visibleProfiles(rows: List<Profile>, activeAccountId: String?): VisibleProfiles {
    val visible = if (activeAccountId == null) rows else rows.filter {
        it.accountBinding.isEmpty() || it.boundAccountId() == activeAccountId
    }
    return VisibleProfiles(visible, rows.size - visible.size)
}

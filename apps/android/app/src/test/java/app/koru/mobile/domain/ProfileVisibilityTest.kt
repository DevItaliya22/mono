// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.domain

import org.junit.Assert.*
import org.junit.Test

class ProfileVisibilityTest {
    @Test fun accountFilteringRetainsOfflineAttributionAndNeverTreatsMalformedAsUnbound() {
        val a = "10000000-0000-4000-8000-000000000001"
        val b = "10000000-0000-4000-8000-000000000002"
        val rows = listOf("", "koru:$a", "koru:$b", " ", "koru:AAAAAAAA-0000-4000-8000-000000000001").mapIndexed { index, binding ->
            Profile(ProfileId(index.toString()), "Profile $index", "en", accountBinding = binding)
        }
        assertEquals(listOf(rows[0], rows[1]), visibleProfiles(rows, a).rows)
        assertEquals(listOf(rows[0], rows[2]), visibleProfiles(rows, b).rows)
        assertEquals(3, visibleProfiles(rows, a).hiddenCount)
        assertEquals(rows, visibleProfiles(rows, null).rows)
        assertEquals(0, visibleProfiles(rows, null).hiddenCount)
        assertEquals(a, rows[1].boundAccountId())
        assertNull(rows[4].boundAccountId())
    }
}

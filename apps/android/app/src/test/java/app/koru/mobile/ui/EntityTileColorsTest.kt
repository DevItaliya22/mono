// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.ui

import androidx.compose.ui.graphics.Color
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Test

class EntityTileColorsTest {
    @Test
    fun workspace_tokens_match_the_desktop_light_and_dark_palettes() {
        assertEquals(Color(0xFF6B4FC4), workspaceTileColor("purple", dark = false))
        assertEquals(Color(0xFF9B86EA), workspaceTileColor("purple", dark = true))
        assertNull(workspaceTileColor("unknown", dark = false))
    }

    @Test
    fun explicit_profile_color_wins_over_the_deterministic_fallback() {
        val explicit = profileTileColors("teal", "profile-a", false, Color.Black)
        assertEquals(Color(0xFF1F7D75), explicit.background)
        assertEquals(Color.Black, explicit.foreground)

        val first = profileTileColors(null, "profile-a", false, Color.Black)
        val repeat = profileTileColors(null, "profile-a", false, Color.Black)
        assertEquals(first, repeat)
        assertNotEquals(first, profileTileColors(null, "profile-b", false, Color.Black))
    }
}

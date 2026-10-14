// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * Which type Android is told a file is.
 *
 * The case this exists for: a 93 MB `MVI_3212.mp4` shared in from a file
 * manager arrives labelled `application/octet-stream`, and the chooser then
 * offers Google, a wallet and a terminal instead of a video player. The bytes
 * were right and the label was not.
 */
class MimeResolutionTest {
    /** Nothing from the framework: these cases must be right on any device. */
    private val noSystemLookup: (String) -> String? = { null }

    private fun resolve(stored: String, name: String) =
        resolveMimeType(stored, name, noSystemLookup)

    @Test
    fun a_specific_stored_type_always_wins() {
        // A sender that named the type knows more than a suffix does, even
        // when the two disagree.
        assertEquals("video/mp4", resolve("video/mp4", "clip.mp4"))
        assertEquals("image/png", resolve("image/png", "screenshot.jpg"))
        assertEquals("application/pdf", resolve("application/pdf", "contract"))
    }

    @Test
    fun a_generic_stored_type_defers_to_the_extension() {
        for (generic in listOf("application/octet-stream", "APPLICATION/OCTET-STREAM", "", "*/*", "content/unknown")) {
            assertEquals(generic, "video/mp4", resolve(generic, "MVI_3212.mp4"))
        }
        assertEquals("video/x-ms-wmv", resolve("application/octet-stream", "try again.wmv"))
        assertEquals("audio/mpeg", resolve("", "song.MP3"))
        assertEquals("image/jpeg", resolve("*/*", "holiday.JPEG"))
    }

    @Test
    fun an_unknown_or_absent_extension_stays_generic_rather_than_guessing() {
        assertEquals("application/octet-stream", resolve("application/octet-stream", "README"))
        assertEquals("application/octet-stream", resolve("", "archive.wibble"))
        assertEquals("application/octet-stream", resolve("", ""))
    }

    @Test
    fun the_system_database_is_consulted_only_for_what_the_table_misses() {
        // A specific stored type never reaches it, and a known extension is
        // answered without it — so a device whose database disagrees cannot
        // change either.
        var asked: String? = null
        val lookup: (String) -> String? = { asked = it; "application/x-from-system" }
        assertEquals("video/mp4", resolveMimeType("", "clip.mp4", lookup))
        assertEquals(null, asked)
        assertEquals("application/x-from-system", resolveMimeType("", "thing.xyz", lookup))
        assertEquals("xyz", asked)
    }
}

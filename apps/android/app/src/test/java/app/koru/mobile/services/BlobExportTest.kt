// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import app.koru.mobile.domain.BlobId
import app.koru.mobile.domain.BlobReference
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The on-disk name of an exported blob.
 *
 * It is addressed by content, not by title, because two files can share a name
 * and a title can be renamed — and because the reuse check in `export` is only
 * sound if the same bytes always land under the same name.
 */
class BlobExportTest {
    private fun reference(id: String, mime: String = "video/mp4", size: Long = 1) =
        BlobReference(BlobId(id), mime, size)

    private val blob = "11111111-2222-4333-8444-555555555555"

    @Test
    fun the_name_is_the_blob_id_and_the_extension_is_the_persons() {
        assertEquals("$blob.mp4", exportName(reference(blob), "Holiday.mp4"))
        assertEquals("$blob.pdf", exportName(reference(blob), "Contract.final.PDF"))
    }

    @Test
    fun a_file_with_no_extension_keeps_the_bare_id() {
        assertEquals(blob, exportName(reference(blob), "README"))
        assertEquals(blob, exportName(reference(blob), ""))
    }

    /**
     * A title is a person's text and travels between devices, so it must not
     * be able to choose a path. Only letters and digits survive into the
     * extension, which leaves no separator, traversal or spacing trick.
     */
    @Test
    fun a_hostile_name_cannot_escape_the_exports_directory() {
        val backslash = '\\'
        val hostile = listOf(
            "video.mp4/../../etc/passwd",
            "video.mp4" + backslash + ".." + backslash + "secret",
            "video.%2e%2e%2fmp4",
            "video. mp4 ",
            "video.m p4",
        )
        for (name in hostile) {
            val exported = exportName(reference(blob), name)
            assertTrue(exported, exported.startsWith(blob))
            assertTrue(exported, exported.none { it == '/' || it == backslash })
            assertTrue(exported, !exported.contains(".."))
        }
    }

    @Test
    fun a_very_long_extension_cannot_grow_the_name_without_bound() {
        val name = exportName(reference(blob), "clip." + "x".repeat(400))
        assertEquals("$blob." + "x".repeat(8), name)
    }
}

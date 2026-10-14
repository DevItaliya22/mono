// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.ui

import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class OpenSourceLicensesTest {
    /**
     * **The screen reads the files the build writes.** Gradle's notices task
     * and its APK check name them in `koruLicenseFiles`; a rename on one side
     * would leave the screen showing "no license files" in every build.
     */
    @Test
    fun the_screen_reads_the_files_the_build_packages() {
        val script = listOf(File("build.gradle.kts"), File("app/build.gradle.kts")).first { it.isFile }.readText()
        val declared = Regex("""val koruLicenseFiles = listOf\(([^)]*)\)""").find(script)
            ?.groupValues?.get(1)?.split(',')?.map { it.trim().trim('"') }
        assertEquals(
            listOf(
                OpenSourceLicenses.LICENSE,
                OpenSourceLicenses.NOTICE,
                OpenSourceLicenses.SOURCE,
                OpenSourceLicenses.THIRD_PARTY,
            ),
            declared,
        )
        assertTrue(script.contains("resolve(\"${OpenSourceLicenses.DIRECTORY}\")"))
    }

    @Test
    fun chunks_rejoin_to_the_text_and_break_at_blank_lines() {
        val text = (1..499).joinToString("\n") { if (it % 10 == 0) "" else "line $it" }
        val chunks = OpenSourceLicenses.chunks(text, maxLines = 40)
        assertEquals(text, chunks.joinToString("\n"))
        assertTrue(chunks.size > 5)
        for (chunk in chunks.dropLast(1)) {
            assertTrue(chunk.endsWith("\n") || chunk.lines().last().isBlank())
            assertTrue(chunk.lines().size in 40..50)
        }
    }

    @Test
    fun a_text_without_blank_lines_is_one_chunk() {
        val text = (1..300).joinToString("\n") { "line $it" }
        assertEquals(listOf(text), OpenSourceLicenses.chunks(text, maxLines = 40))
        assertEquals(listOf("a\nb"), OpenSourceLicenses.chunks("a\r\nb\r\n"))
    }
}

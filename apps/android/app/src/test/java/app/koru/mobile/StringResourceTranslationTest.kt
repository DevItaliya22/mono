// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile

import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * **Every id in `values/strings.xml` exists in all seven other languages.**
 *
 * The desktop gets this from the type system: `koru_i18n::strings::Row` has
 * eight fields, so a forgotten translation does not compile. Android has no
 * such gate — a missing key falls back to `values/` silently and the screen
 * looks like it works with one English word in it — so the gate is this test.
 *
 * It also refuses `tools:ignore="MissingTranslation"`. That attribute is how
 * the last gap hid: it is written when a string is added in a hurry and it
 * outlives the reason, so the honest state of an untranslated string is a
 * failing test rather than a suppressed warning. A string that is genuinely
 * the same word everywhere says `translatable="false"` instead, which is a
 * claim rather than an omission.
 *
 * It reads the source tree rather than a built `Resources`: no `Context`, no
 * emulator, no network — the same reason `koru-i18n`'s completeness tests read
 * the table rather than the app.
 */
class StringResourceTranslationTest {
    private val res = listOf(File("src/main/res"), File("app/src/main/res")).first { it.isDirectory }

    private val languages =
        listOf("values-es", "values-pt-rBR", "values-de", "values-fr", "values-ar", "values-zh-rCN", "values-ja")

    /** Arabic selects all six CLDR categories; the others fall through to `other`. */
    private val requiredQuantities = mapOf("values-ar" to setOf("zero", "one", "two", "few", "many", "other"))

    /** Prose is allowed to name the attribute; markup is not. */
    private fun withoutComments(text: String) =
        text.replace(Regex("<!--.*?-->", setOf(RegexOption.DOT_MATCHES_ALL)), "")

    private fun ids(directory: String, tag: String): Map<String, String> {
        val text = withoutComments(File(res, "$directory/strings.xml").readText())
        return Regex("<$tag name=\"([a-z_0-9]+)\"([^>]*)>(.*?)</$tag>", setOf(RegexOption.DOT_MATCHES_ALL))
            .findAll(text)
            .filterNot { it.groupValues[2].contains("translatable=\"false\"") }
            .associate { it.groupValues[1] to it.groupValues[3] }
    }

    /** `%1$s`, `%1$d`, `%d` — the tokens a translation must carry through. */
    private fun placeholders(value: String): List<String> =
        Regex("%(?:\\d+\\\$)?[sd]").findAll(value).map { it.value.substringAfterLast('$').let { t -> "%$t" } }.toList()

    @Test
    fun no_string_is_marked_as_missing_a_translation() {
        for (directory in listOf("values") + languages) {
            val text = withoutComments(File(res, "$directory/strings.xml").readText())
            assertTrue(
                "$directory/strings.xml still suppresses MissingTranslation",
                !text.contains("tools:ignore"),
            )
        }
    }

    @Test
    fun every_translatable_string_exists_in_all_seven_languages() {
        val source = ids("values", "string")
        assertTrue("no strings were parsed — the regex or the path is wrong", source.size > 100)
        for (directory in languages) {
            val translated = ids(directory, "string")
            assertEquals(
                "$directory/strings.xml is missing ids",
                emptySet<String>(),
                source.keys - translated.keys,
            )
            assertEquals(
                "$directory/strings.xml has ids the source does not",
                emptySet<String>(),
                translated.keys - source.keys,
            )
        }
    }

    @Test
    fun every_plural_exists_in_all_seven_languages_with_the_categories_it_needs() {
        val source = ids("values", "plurals")
        assertTrue("no plurals were parsed", source.isNotEmpty())
        for (directory in languages) {
            val translated = ids(directory, "plurals")
            assertEquals("$directory is missing plurals", source.keys, translated.keys)
            val required = requiredQuantities[directory] ?: setOf("other")
            for ((name, body) in translated) {
                val quantities = Regex("quantity=\"([a-z]+)\"").findAll(body).map { it.groupValues[1] }.toSet()
                assertEquals("$directory/$name quantities", emptySet<String>(), required - quantities)
            }
        }
    }

    /**
     * A translation that drops `%1$s` throws `IllegalFormatException` at the
     * moment the screen draws, in one language, on somebody else's phone.
     */
    @Test
    fun a_translation_carries_the_same_placeholders_as_its_english() {
        val source = ids("values", "string")
        for (directory in languages) {
            for ((name, value) in ids(directory, "string")) {
                assertEquals(
                    "$directory/$name placeholders",
                    placeholders(source.getValue(name)).sorted(),
                    placeholders(value).sorted(),
                )
            }
        }
    }
}

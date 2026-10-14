// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.ui

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import app.koru.mobile.domain.*
import app.koru.mobile.ui.editor.annotatedRichText
import app.koru.mobile.ui.editor.parseMarkdownTable
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The name a reference falls back to when it carries none of its own. The app
 * resolves these from resources; a unit test has no `Context`, and naming the
 * words here keeps the assertion about *anchoring* rather than about English.
 */
private fun unlabelled(reference: EntityReference): String = when (reference) {
    is EntityReference.Url -> reference.value
    is EntityReference.Label -> reference.value
    else -> "reference"
}

private fun pillLabel(pill: InlineObject): String = pill.label ?: unlabelled(pill.reference)

class DocumentViewerTest {
    @Test
    fun rich_text_replaces_a_utf8_anchor_with_a_followable_label() {
        val rich = RichText(
            text = "A⁠B",
            marks = listOf(TextMark(TextRange(0, 1), MarkKind.Bold)),
            inlineObjects = listOf(
                InlineObject(
                    InlineObjectId("10000000-0000-4000-8000-000000000001"),
                    1,
                    "document",
                    EntityReference.Document(DocumentId("10000000-0000-4000-8000-000000000002")),
                    "Docs",
                ),
            ),
        )

        val rendered = annotatedRichText(rich, Color.Blue, ::pillLabel)

        assertEquals("ADocsB", rendered.text)
        assertEquals("0", rendered.getStringAnnotations("koru-reference", 1, 4).single().item)
        assertTrue(rendered.spanStyles.isNotEmpty())
    }

    @Test
    fun markdown_table_separator_is_not_shown_as_data() {
        assertEquals(
            listOf(listOf("Name", "Value"), listOf("Koru", "1")),
            parseMarkdownTable("| Name | Value |\n|:---|---:|\n| Koru | 1 |"),
        )
    }

    @Test
    fun bold_marks_select_a_real_bold_face() {
        val rendered = annotatedRichText(
            RichText("plain bold", listOf(TextMark(TextRange(6, 10), MarkKind.Bold))),
            Color.Blue,
            ::pillLabel,
        )

        val bold = rendered.spanStyles.single { it.start == 6 && it.end == 10 }
        assertEquals(FontWeight.Bold, bold.item.fontWeight)
        assertEquals(FontFamily.SansSerif, bold.item.fontFamily)
    }
}

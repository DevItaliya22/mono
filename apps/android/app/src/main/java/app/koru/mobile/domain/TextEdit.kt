// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.domain

data class TextSplice(val start: Int, val end: Int, val inserted: String)
data class BlockTextSplit(val blockId: BlockId, val atUtf8: Int)

fun RichText.slice(start: Int, end: Int): RichText {
    val from = text.substring(0, start).encodeToByteArray().size
    val endByte = text.substring(0, end).encodeToByteArray().size
    return RichText(text.substring(start, end), marks.mapNotNull { mark ->
        val a = maxOf(from, mark.range.startUtf8)
        val b = minOf(endByte, mark.range.endUtf8)
        if (a < b) mark.copy(range = TextRange(a - from, b - from)) else null
    }, inlineObjects.filter { it.anchorUtf8 in from until endByte }.map { it.copy(anchorUtf8 = it.anchorUtf8 - from) })
}

/** Provisional layout only; the document model supplies authoritative marks after saving. */
fun RichText.withDraftText(after: String): RichText {
    val splice = textSplice(text, after)
    val start = text.substring(0, splice.start).encodeToByteArray().size
    val end = text.substring(0, splice.end).encodeToByteArray().size
    val inserted = splice.inserted.encodeToByteArray().size
    val delta = inserted - (end - start)
    fun boundary(offset: Int) = when {
        offset <= start -> offset
        offset >= end -> offset + delta
        else -> start + inserted
    }
    return RichText(after, marks.mapNotNull { mark ->
        val a = boundary(mark.range.startUtf8)
        val b = boundary(mark.range.endUtf8)
        if (a < b) mark.copy(range = TextRange(a, b)) else null
    }, inlineObjects.filterNot { it.anchorUtf8 in start until end }.map {
        it.copy(anchorUtf8 = if (it.anchorUtf8 >= end) it.anchorUtf8 + delta else it.anchorUtf8)
    })
}

/** Native UTF-16 selection boundaries must never split a surrogate pair. */
fun textSplice(before: String, after: String): TextSplice {
    var start = 0
    while (start < minOf(before.length, after.length) && before[start] == after[start]) start++
    if (start in 1 until before.length && before[start].isLowSurrogate()) start--
    var suffix = 0
    while (before.length - suffix > start && after.length - suffix > start &&
        before[before.length - suffix - 1] == after[after.length - suffix - 1]) suffix++
    if (suffix > 0 && before[before.length - suffix].isLowSurrogate()) suffix--
    return TextSplice(start, before.length - suffix, after.substring(start, after.length - suffix))
}

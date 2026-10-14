// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.sharing

import app.koru.mobile.services.IncomingSharedFile
import app.koru.mobile.services.SharedContentSource
import java.io.ByteArrayInputStream

internal fun incomingTextFile(value: String, subject: String?, html: Boolean = false): IncomingSharedFile {
    val kind = when {
        html -> TextKind.Html
        value.trim().matches(Regex("https?://\\S+", RegexOption.IGNORE_CASE)) -> TextKind.Url
        else -> TextKind.Plain
    }
    val (fallback, suffix, mimeType, content) = when (kind) {
        TextKind.Url -> TextParts("Shared link", ".url", "text/uri-list", value.trim() + "\n")
        TextKind.Html -> TextParts("Shared page", ".html", "text/html", value)
        TextKind.Plain -> TextParts("Shared text", ".txt", "text/plain", value)
    }
    val base = safeDisplayName(subject.orEmpty()).takeUnless { it == "Shared file" } ?: fallback
    val name = safeDisplayName(if (base.endsWith(suffix, ignoreCase = true)) base else base + suffix)
    val bytes = content.toByteArray(Charsets.UTF_8)
    return IncomingSharedFile(
        name = name,
        mimeType = mimeType,
        declaredSize = bytes.size.toLong(),
        source = SharedContentSource { ByteArrayInputStream(bytes) },
    )
}

private enum class TextKind { Plain, Url, Html }

private data class TextParts(
    val fallbackName: String,
    val suffix: String,
    val mimeType: String,
    val content: String,
)

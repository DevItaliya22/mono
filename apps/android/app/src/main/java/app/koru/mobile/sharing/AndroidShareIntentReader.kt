// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.sharing

import android.content.ContentResolver
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.OpenableColumns
import android.webkit.MimeTypeMap
import app.koru.mobile.domain.AppErrorCode
import app.koru.mobile.domain.AppFailure
import app.koru.mobile.services.IncomingShare
import app.koru.mobile.services.IncomingSharedFile
import app.koru.mobile.services.SharedContentSource
import java.io.IOException

/** Converts Android's temporary share grants into sources the file service can stream. */
class AndroidShareIntentReader(private val resolver: ContentResolver) {
    fun read(intent: Intent): IncomingShare? {
        if (intent.action != Intent.ACTION_SEND && intent.action != Intent.ACTION_SEND_MULTIPLE) return null

        val files = mutableListOf<IncomingSharedFile>()
        val uris = linkedSetOf<Uri>()
        uris += streamUris(intent)
        intent.clipData?.let { clip ->
            repeat(clip.itemCount) { index -> clip.getItemAt(index).uri?.let(uris::add) }
        }
        intent.data?.let(uris::add)
        uris.forEachIndexed { index, uri -> files += uriFile(uri, intent.type, index + 1) }

        val subject = intent.getStringExtra(Intent.EXTRA_SUBJECT)
            ?: intent.getStringExtra(Intent.EXTRA_TITLE)
        val html = intent.getStringExtra(Intent.EXTRA_HTML_TEXT)
        val texts = linkedSetOf<String>()
        if (intent.action == Intent.ACTION_SEND_MULTIPLE) {
            @Suppress("DEPRECATION")
            intent.getCharSequenceArrayListExtra(Intent.EXTRA_TEXT)
                .orEmpty()
                .map(CharSequence::toString)
                .filter(String::isNotBlank)
                .forEach(texts::add)
        } else {
            intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString()?.takeIf(String::isNotBlank)?.let(texts::add)
        }
        intent.clipData?.let { clip ->
            repeat(clip.itemCount) { index ->
                clip.getItemAt(index).text?.toString()?.takeIf(String::isNotBlank)?.let(texts::add)
            }
        }
        if (!html.isNullOrBlank()) {
            files += incomingTextFile(html, subject, html = true)
        } else {
            texts.forEach { text -> files += incomingTextFile(text, subject) }
        }

        if (files.isEmpty()) {
            throw AppFailure(AppErrorCode.Unsupported, "the share intent contains no readable content")
        }
        return IncomingShare(files)
    }

    private fun uriFile(uri: Uri, intentMimeType: String?, ordinal: Int): IncomingSharedFile {
        val metadata = queryMetadata(uri)
        val mimeType = safeMimeType(runCatching { resolver.getType(uri) }.getOrNull() ?: intentMimeType)
        val fallback = buildString {
            append("Shared file")
            if (ordinal > 1) append(" ").append(ordinal)
            MimeTypeMap.getSingleton().getExtensionFromMimeType(mimeType)?.let { append('.').append(it) }
        }
        return IncomingSharedFile(
            name = safeDisplayName(metadata.name ?: fallback),
            mimeType = mimeType,
            declaredSize = metadata.size,
            source = SharedContentSource {
                resolver.openInputStream(uri) ?: throw IOException("shared URI cannot be opened")
            },
        )
    }

    private fun queryMetadata(uri: Uri): UriMetadata = runCatching {
        resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)
            ?.use { cursor ->
                if (!cursor.moveToFirst()) return@use UriMetadata()
                val nameColumn = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                val sizeColumn = cursor.getColumnIndex(OpenableColumns.SIZE)
                UriMetadata(
                    name = nameColumn.takeIf { it >= 0 && !cursor.isNull(it) }?.let(cursor::getString),
                    size = sizeColumn.takeIf { it >= 0 && !cursor.isNull(it) }
                        ?.let(cursor::getLong)
                        ?.takeIf { it >= 0 },
                )
            } ?: UriMetadata()
    }.getOrDefault(UriMetadata())

    @Suppress("DEPRECATION")
    private fun streamUris(intent: Intent): List<Uri> {
        return if (intent.action == Intent.ACTION_SEND_MULTIPLE) {
            if (Build.VERSION.SDK_INT >= 33) {
                intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java).orEmpty()
            } else {
                intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM).orEmpty()
            }
        } else {
            val value = if (Build.VERSION.SDK_INT >= 33) {
                intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
            } else {
                intent.getParcelableExtra(Intent.EXTRA_STREAM) as? Uri
            }
            listOfNotNull(value)
        }
    }

    private data class UriMetadata(val name: String? = null, val size: Long? = null)
}

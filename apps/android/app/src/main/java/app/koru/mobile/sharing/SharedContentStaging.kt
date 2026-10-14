// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.sharing

import app.koru.mobile.domain.AppErrorCode
import app.koru.mobile.domain.AppFailure
import app.koru.mobile.services.IncomingSharedFile
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.util.UUID

/**
 * Copies one shared payload to a private scratch file.
 *
 * The copy exists because two clocks disagree: Android's read grant on the
 * sender's URI is alive only for this activity, while the Store import wants a
 * path it can read at its own pace. Streaming here rather than into a byte
 * array is deliberate — a shared video must not have to fit in the JVM heap on
 * its way to a native call that will read it from disk anyway.
 */
internal fun stageSharedFile(directory: File, incoming: IncomingSharedFile): File {
    if (!directory.exists() && !directory.mkdirs()) {
        throw AppFailure(AppErrorCode.Unavailable, "shared content could not be staged")
    }
    val staged = directory.resolve("${UUID.randomUUID()}.staged")
    try {
        incoming.source.open().use { input ->
            FileOutputStream(staged).use { output -> input.copyTo(output) }
        }
    } catch (error: Throwable) {
        staged.delete()
        throw when (error) {
            is AppFailure -> error
            is SecurityException ->
                AppFailure(AppErrorCode.PermissionDenied, "shared content permission expired")
            is IOException ->
                AppFailure(AppErrorCode.Unavailable, "shared content could not be read")
            else -> error
        }
    }
    return staged
}

internal fun safeDisplayName(value: String): String {
    val cleaned = value
        .replace(Regex("""[\p{Cc}\p{Cf}\\/]+"""), " ")
        .trim()
        .trim('.')
        .trim()
        .take(180)
    return cleaned.ifBlank { "Shared file" }
}

internal fun safeMimeType(value: String?): String {
    val normalized = value?.substringBefore(';')?.trim()?.lowercase().orEmpty()
    return normalized.takeIf { it.matches(Regex("[a-z0-9!#$&^_.+-]+/[a-z0-9!#$&^_.+-]+")) && it != "*/*" }
        ?: "application/octet-stream"
}

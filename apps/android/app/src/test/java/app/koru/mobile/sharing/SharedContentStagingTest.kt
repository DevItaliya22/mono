// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.sharing

import app.koru.mobile.domain.AppErrorCode
import app.koru.mobile.domain.AppFailure
import app.koru.mobile.services.IncomingSharedFile
import app.koru.mobile.services.SharedContentSource
import java.io.ByteArrayInputStream
import java.io.IOException
import java.io.InputStream
import kotlin.io.path.createTempDirectory
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class SharedContentStagingTest {
    private val root = createTempDirectory("staging").toFile()

    @Test fun streamsThePayloadIntoItsOwnScratchFile() {
        val staged = stageSharedFile(
            root.resolve("scratch"),
            incoming("note.txt") { ByteArrayInputStream("shared".toByteArray()) },
        )
        assertTrue(staged.isFile)
        assertEquals("shared", staged.readText())
    }

    @Test fun leavesNothingBehindWhenTheGrantHasExpired() {
        val directory = root.resolve("expired")
        assertEquals(AppErrorCode.PermissionDenied, failureFrom(directory) { throw SecurityException("gone") })
        assertEquals(AppErrorCode.Unavailable, failureFrom(directory) { throw IOException("gone") })
        assertEquals(emptyList<java.io.File>(), directory.listFiles()?.toList())
    }

    @Test fun namesAndTypesAreNormalizedBeforeTheyReachTheStore() {
        assertEquals("Shared file", safeDisplayName("  ...  "))
        assertEquals("a b.txt", safeDisplayName("a\u0000/b.txt"))
        assertEquals(180, safeDisplayName("x".repeat(400)).length)
        assertEquals("text/plain", safeMimeType("Text/Plain; charset=utf-8"))
        assertEquals("application/octet-stream", safeMimeType("*/*"))
        assertEquals("application/octet-stream", safeMimeType(null))
    }

    private fun failureFrom(directory: java.io.File, open: () -> InputStream): AppErrorCode = try {
        stageSharedFile(directory, incoming("note.txt", open))
        throw AssertionError("staging was expected to fail")
    } catch (error: AppFailure) {
        error.code
    }

    private fun incoming(name: String, open: () -> InputStream) = IncomingSharedFile(
        name = name,
        mimeType = "text/plain",
        declaredSize = null,
        source = SharedContentSource { open() },
    )
}

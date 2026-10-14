// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.sharing

import org.junit.Assert.assertEquals
import org.junit.Test

class SharedTextTest {
    @Test fun webUrlsBecomeUrlFiles() {
        val file = incomingTextFile(" https://koru.app/share?q=1 ", null)
        assertEquals("Shared link.url", file.name)
        assertEquals("text/uri-list", file.mimeType)
        assertEquals("https://koru.app/share?q=1\n", file.source.open().bufferedReader().readText())
    }

    @Test fun textAndHtmlRemainFilesWithSafeNames() {
        val text = incomingTextFile("a sentence with https://koru.app in it", "Notes/Today")
        assertEquals("Notes Today.txt", text.name)
        assertEquals("text/plain", text.mimeType)

        val html = incomingTextFile("<p>Hello</p>", "Article", html = true)
        assertEquals("Article.html", html.name)
        assertEquals("text/html", html.mimeType)
    }
}

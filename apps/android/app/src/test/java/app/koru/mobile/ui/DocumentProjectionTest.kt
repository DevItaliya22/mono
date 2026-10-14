// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.ui

import app.koru.mobile.domain.DocumentId
import app.koru.mobile.domain.DocumentSummary
import app.koru.mobile.domain.WorkspaceId
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class DocumentProjectionTest {
    private val workspace = WorkspaceId("10000000-0000-4000-8000-000000000001")

    @Test
    fun documents_and_files_use_manifest_kind_not_filename() {
        val document = DocumentSummary(
            DocumentId("10000000-0000-4000-8000-000000000002"),
            workspace,
            "Looks like a file.pdf",
            kind = "document",
        )
        val file = DocumentSummary(
            DocumentId("10000000-0000-4000-8000-000000000003"),
            workspace,
            "No extension",
            kind = "file",
        )

        assertTrue(documentVisible(document, DocumentProjection.Documents))
        assertFalse(documentVisible(document, DocumentProjection.Files))
        assertTrue(documentVisible(file, DocumentProjection.Files))
        assertFalse(documentVisible(file, DocumentProjection.Documents))
    }
}

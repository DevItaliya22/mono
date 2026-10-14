// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import app.koru.contract.v1.DocumentProjection
import app.koru.contract.v1.DocumentEventEnvelope
import app.koru.mobile.domain.AppFailure
import app.koru.mobile.domain.BlockContent
import app.koru.mobile.domain.MarkKind
import org.junit.Assert.*
import org.junit.Test

class ContractMapperTest {
    @Test fun validFixturesMapToNativeState() {
        listOf("simple_document.pb", "multilingual_document.pb", "rtl_document.pb", "image_document.pb").forEach { name ->
            val contract = resource("valid/$name").use(DocumentProjection::parseFrom)
            val native = ContractMapper.document(contract)
            assertFalse(name, native.blocks.isEmpty())
        }
    }

    @Test fun invalidFixturesAreRejectedAtBoundary() {
        listOf("invalid_uuid.pb", "invalid_range.pb", "invalid_hierarchy.pb", "invalid_required_fields.pb").forEach { name ->
            val contract = resource("invalid/$name").use(DocumentProjection::parseFrom)
            assertThrows(name, AppFailure::class.java) { ContractMapper.document(contract) }
        }
    }

    @Test fun unknownBlockPayloadIsPreserved() {
        val bytes = resource("forward_compat/unknown_block_kind.pb").readBytes()
        val contract = DocumentProjection.parseFrom(bytes)
        val unsupported = ContractMapper.document(contract).blocks.last().content as BlockContent.Unsupported
        assertArrayEquals(byteArrayOf(0x08, 0x96.toByte(), 0x01), unsupported.payload)
        assertEquals(777, unsupported.kindCode)
    }

    @Test fun unknownMarkKeepsItsWireIdentityAndValue() {
        val contract = resource("forward_compat/unknown_mark.pb").use(DocumentProjection::parseFrom)
        val paragraph = ContractMapper.document(contract).blocks.single().content as BlockContent.Paragraph
        val mark = paragraph.richText.marks.single()
        assertEquals(MarkKind.Unknown, mark.kind)
        assertEquals("koru.future.sparkle", mark.wireKind)
        assertArrayEquals(byteArrayOf(0xca.toByte(), 0xfe.toByte()), mark.value)
    }

    @Test fun unknownEventWireBytesRoundTrip() {
        val bytes = resource("forward_compat/unknown_event.pb").readBytes()
        assertArrayEquals(bytes, DocumentEventEnvelope.parseFrom(bytes).toByteArray())
    }

    private fun resource(path: String) = requireNotNull(javaClass.classLoader?.getResourceAsStream(path)) { path }
}

// SPDX-License-Identifier: MPL-2.0

import XCTest
@testable import KoruMobile

final class ContractFixtureTests: XCTestCase {
    func testValidFixturesMapToNativeState() throws {
        for name in ["simple_document", "multilingual_document", "rtl_document", "image_document"] {
            let projection = try KORDocumentProjection(serializedBytes: fixture(name, folder: "valid"))
            XCTAssertFalse(try ContractMapper.document(projection).blocks.isEmpty, name)
        }
    }

    func testInvalidFixturesAreRejectedAtBoundary() throws {
        for name in ["invalid_uuid", "invalid_range", "invalid_hierarchy", "invalid_required_fields"] {
            let projection = try KORDocumentProjection(serializedBytes: fixture(name, folder: "invalid"))
            XCTAssertThrowsError(try ContractMapper.document(projection), name)
        }
    }

    func testUnknownBlockAndMarkArePreservedInNativeState() throws {
        let future = try KORDocumentProjection(serializedBytes: fixture("unknown_block_kind", folder: "forward_compat"))
        guard case let .unsupported(kind, typeURL, payload) = try ContractMapper.document(future).blocks.last?.content else { return XCTFail("missing unsupported block") }
        XCTAssertEqual(kind, 777)
        XCTAssertEqual(typeURL, "type.koru.do/koru.v9.FutureBlockPayload")
        XCTAssertEqual(payload, Data([0x08, 0x96, 0x01]))

        let marked = try KORDocumentProjection(serializedBytes: fixture("unknown_mark", folder: "forward_compat"))
        guard case let .paragraph(rich) = try ContractMapper.document(marked).blocks[0].content else { return XCTFail("missing paragraph") }
        XCTAssertEqual(rich.marks[0].kind, .unknown)
        XCTAssertEqual(rich.marks[0].wireKind, "koru.future.sparkle")
        XCTAssertEqual(rich.marks[0].value, Data([0xca, 0xfe]))
    }

    func testUnknownEventWireBytesRoundTrip() throws {
        let bytes = try fixture("unknown_event", folder: "forward_compat")
        XCTAssertEqual(try KORDocumentEventEnvelope(serializedBytes: bytes).serializedData(), bytes)
    }

    private func fixture(_ name: String, folder: String) throws -> Data {
        let bundle = Bundle(for: Self.self)
        let url = try XCTUnwrap(
            bundle.url(forResource: name, withExtension: "pb", subdirectory: folder) ?? bundle.url(forResource: name, withExtension: "pb"),
            "fixture resource not copied: \(folder)/\(name).pb"
        )
        return try Data(contentsOf: url)
    }
}

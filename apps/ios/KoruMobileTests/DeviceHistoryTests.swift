// SPDX-License-Identifier: MPL-2.0

import XCTest

@testable import KoruMobile

/// The history page as the host writes it decodes into the rows the Devices
/// screen shows: one event of each kind, the paging cursor, and a row that
/// has no durable id still has an identity of its own.
final class DeviceHistoryTests: XCTestCase {
    func testAPageDecodesEveryKindAndItsCursor() throws {
        let json = """
        {"device_id":"6cca06d3-5792-42d2-aedb-b887a6682475","events":[
          {"event_id":9,"at_millis":1787347352347,"kind":"document_updated","workspace_id":"w1","document_id":"d1","document_name":"Roadmap","items":3},
          {"event_id":8,"at_millis":1787347352000,"kind":"catalog_reconciled","detail":"2 profiles, 1 workspace"},
          {"event_id":7,"at_millis":1787347351000,"kind":"blob_stored","workspace_id":"w1","blob_hash":"abcdef0123456789"},
          {"event_id":null,"at_millis":1787347350000,"kind":"profile_updated","profile_id":"p1"}
        ],"next_before":1787347350000}
        """
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        let page = try decoder.decode(NativeDeviceHistory.self, from: Data(json.utf8)).page
        XCTAssertEqual(page.deviceId, "6cca06d3-5792-42d2-aedb-b887a6682475")
        XCTAssertEqual(page.nextBefore, 1787347350000)
        XCTAssertEqual(page.events.map(\.kind), ["document_updated", "catalog_reconciled", "blob_stored", "profile_updated"])
        XCTAssertEqual(page.events[0].documentName, "Roadmap")
        XCTAssertEqual(page.events[0].items, 3)
        XCTAssertEqual(page.events[1].detail, "2 profiles, 1 workspace")
        XCTAssertNil(page.events[3].eventId)
        XCTAssertEqual(Set(page.events.map(\.id)).count, 4, "every row identifies itself, durable or not")
    }
}

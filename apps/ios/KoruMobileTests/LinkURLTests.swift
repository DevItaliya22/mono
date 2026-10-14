// SPDX-License-Identifier: MPL-2.0

import XCTest

@testable import KoruMobile

/// The one mobile deep link, and only it — the mirror of Android's
/// `acceptLinkIntent` guard. `koru://` is desktop-scope (ADR-132) and must
/// never be accepted here.
final class LinkURLTests: XCTestCase {
    func testExactlyTheLinkDeviceURLIsAccepted() {
        XCTAssertTrue(AppModel.isLinkURL(URL(string: "https://koru.do/link-device?c=abc")!))
        XCTAssertTrue(AppModel.isLinkURL(URL(string: "https://koru.do/link-device")!))
        XCTAssertFalse(AppModel.isLinkURL(URL(string: "http://koru.do/link-device")!))
        XCTAssertFalse(AppModel.isLinkURL(URL(string: "https://koru.do/other")!))
        XCTAssertFalse(AppModel.isLinkURL(URL(string: "https://example.com/link-device")!))
        XCTAssertFalse(AppModel.isLinkURL(URL(string: "koru://doc/123")!))
    }
}

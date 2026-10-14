// SPDX-License-Identifier: MPL-2.0

import XCTest

@testable import KoruMobile

/// A picture load that produced no bytes: which codes are a wait and which are
/// damage — Android's `BlobLoadFailureTest`, on iOS (I14). The placeholder is
/// chosen from this state, so a peer that lacks the bytes must never reach it
/// as `.failed`: that is "Image unavailable" over a file a desktop holds.
final class BlobLoadFailureTests: XCTestCase {
    func testAPeerThatLacksTheBytesIsAWaitNamingNobody() {
        XCTAssertEqual(
            KoruContentServices.loadFailure(code: "blob_not_found"),
            .waitingForPeer(deviceName: nil))
    }

    func testAParkedHydrationNamesTheDeviceItWaitsOn() {
        XCTAssertEqual(
            KoruContentServices.loadFailure(code: "blob_waiting_for_peer", waitingOn: "Desktop"),
            .waitingForPeer(deviceName: "Desktop"))
    }

    func testEveryNotHeldCodeIsAWait() {
        for code in KoruContentServices.notHeldCodes {
            XCTAssertEqual(
                KoruContentServices.loadFailure(code: code), .waitingForPeer(deviceName: nil), code)
        }
    }

    func testNothingLinkedKeepsItsOwnRemedy() {
        XCTAssertEqual(KoruContentServices.loadFailure(code: "no_linked_devices"), .noLinkedDevices)
    }

    func testGenuineFailuresStayFailures() {
        for code in [
            "blob_hash_mismatch", "blob_length_mismatch", "blob_metadata_changed", "blob_too_large",
            "unauthorized_workspace", "forbidden", "relay_path_refused", "unexpected_blob_frame",
            "blob_load_failed",
        ] {
            XCTAssertEqual(
                KoruContentServices.loadFailure(code: code), .failed(code: code, retryable: false), code)
        }
    }

    func testATransientFailureStaysRetryable() {
        for code in ["timeout", "runtime_closed", "store_unavailable", "internal"] {
            XCTAssertEqual(
                KoruContentServices.loadFailure(code: code), .failed(code: code, retryable: true), code)
        }
    }
}

// SPDX-License-Identifier: MPL-2.0

import XCTest

@testable import KoruMobile

/// The permanent FFI canary: proof the Rust staticlib is linked, callable,
/// and answering generated Protobuf values through the canonical C ABI.
final class KoruNativeTests: XCTestCase {
    private func freshRoot() -> String {
        FileManager.default.temporaryDirectory
            .appendingPathComponent("koru-native-\(UUID().uuidString)").path
    }

    func testAFreshRootAnswersUnlinked() {
        // Mirrors koru-mobile-host api::tests::a_fresh_root_answers_unlinked_through_the_api.
        let value = try? KoruNative.status(
            root: freshRoot(), secrets: Data(repeating: 47, count: 64))
        XCTAssertEqual(value?.state, "unlinked")
        XCTAssertEqual(value?.hasErrorCode, false)
    }

    /// Mirrors koru-mobile-host
    /// api::tests::a_mismatched_identity_can_start_over_and_keeps_the_old_one_aside:
    /// other seeds over the same root open as a mismatch, the reset leads out
    /// of it, and a healthy device refuses the reset.
    func testAMismatchedIdentityCanStartOverThroughTheCShim() throws {
        let root = freshRoot()
        let first = Data(repeating: 47, count: 64)
        XCTAssertEqual(try KoruNative.status(root: root, secrets: first).state, "unlinked")
        let refused = try KoruNative.resetIdentity(root: root, secrets: first)
        XCTAssertEqual(refused.errorCode, "identity_intact")
        // The same files seen by an app holding other seeds: a reinstall.
        let reinstalled = freshRoot()
        try FileManager.default.copyItem(atPath: root, toPath: reinstalled)
        let second = Data(repeating: 48, count: 64)
        let mismatch = try KoruNative.status(root: reinstalled, secrets: second)
        XCTAssertEqual(mismatch.errorCode, "device_identity_mismatch")
        // **And the state here is not `failed`, which is the whole trap.**
        // `status` answers `unavailable` while `link_device` and
        // `start_pairing` answer `failed`, so a reader that switches on the
        // state and looks for the code only inside its `failed` arm cannot see
        // a mismatch on the path the Devices screen actually polls. That was
        // TECHDEBT 85, and this file could not see it because it asserted the
        // code and never the state it arrives beside.
        XCTAssertNotEqual(mismatch.state, "failed")
        XCTAssertEqual(try KoruNative.resetIdentity(root: reinstalled, secrets: second).state, "reset")
        XCTAssertEqual(try KoruNative.status(root: reinstalled, secrets: second).state, "unlinked")
        let aside = try FileManager.default.contentsOfDirectory(atPath: reinstalled)
            .filter { $0.hasPrefix("identity-set-aside-") }
        XCTAssertEqual(aside.count, 1)
        XCTAssertTrue(FileManager.default.fileExists(atPath: "\(reinstalled)/\(aside[0])/device.json"))
    }

    /// Mirrors koru-mobile-host api::tests::a_device_with_no_history_answers_an_empty_page.
    func testADeviceWithNoHistoryAnswersAnEmptyPage() {
        let root = freshRoot()
        let secrets = Data(repeating: 47, count: 64)
        let device = "6cca06d3-5792-42d2-aedb-b887a6682475"
        let empty = try? KoruNative.history(root: root, secrets: secrets, deviceID: device, before: nil)
        XCTAssertEqual(empty?.deviceID, device)
        XCTAssertEqual(empty?.events.count, 0)
        let invalid = try? KoruNative.history(root: root, secrets: secrets, deviceID: "not-a-device", before: nil)
        XCTAssertEqual(invalid?.errorCode, "invalid_device_id")
    }

    func testShortSecretsFailClosedInTheEndpointsShape() {
        // Mirrors koru-mobile-host api::tests::short_secrets_fail_closed_in_the_endpoints_shape.
        let value = try? KoruNative.status(root: freshRoot(), secrets: Data([1, 2, 3]))
        XCTAssertEqual(value?.state, "failed")
        XCTAssertEqual(value?.errorCode, "invalid_device_seeds")
    }

    func testProfileAppearanceSurvivesTheNativeInventoryJSON() throws {
        let json = #"{"profiles":[{"id":"profile","name":"Lab","locale":"en","icon":"🧪","leading_emoji":"🧪","display_name":"Lab","color":"purple"}],"workspaces":[],"documents":[]}"#
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase

        let inventory = try decoder.decode(NativeInventory.self, from: Data(json.utf8))
        XCTAssertEqual(inventory.profiles.first?.icon, "🧪")
        XCTAssertEqual(inventory.profiles.first?.color, "purple")

        let legacy = #"{"profiles":[{"id":"old","name":"Old","locale":"en","leading_emoji":"O","display_name":"Old","color":null}],"workspaces":[],"documents":[]}"#
        let oldInventory = try decoder.decode(NativeInventory.self, from: Data(legacy.utf8))
        XCTAssertNil(oldInventory.profiles.first?.icon)
    }

    func testProfileColorTokensMatchTheOtherClients() {
        XCTAssertEqual(KoruEntityColor.rgb("purple", dark: false), 0x6B4FC4)
        XCTAssertEqual(KoruEntityColor.rgb("purple", dark: true), 0x9B86EA)
        XCTAssertNil(KoruEntityColor.rgb("future-color", dark: false))
    }

    /// The whole pairing handshake through the C shim, two real hosts, real
    /// iroh, inside the simulator — the decisive test for "linking does not
    /// work on iOS": green here means the native path works in this
    /// environment and a failure in the app is the Swift layer or the network
    /// between two real machines.
    func testTwoHostsPairThroughTheCShim() throws {
        let inviterRoot = freshRoot()
        let joinerRoot = freshRoot()
        let offer = try KoruNative.startPairing(
            root: inviterRoot, secrets: Data(repeating: 61, count: 64))
        XCTAssertEqual(offer.state, "displaying", "no offer: \(offer.errorCode)")
        XCTAssertTrue(offer.hasQrPayload)

        let outcome = try KoruNative.link(
            root: joinerRoot, secrets: Data(repeating: 62, count: 64), code: offer.qrPayload)
        XCTAssertEqual(outcome.state, "linked", "join failed: \(outcome.errorCode)")
    }

    func testABlobMissEverythingAnswersAnErrorNotACrash() {
        let result = try? KoruNative.blob(
            root: freshRoot(), secrets: Data([1, 2, 3]), workspaceID: "w", hash: "h")
        XCTAssertEqual(result?.errorCode, "invalid_device_seeds")
    }
}

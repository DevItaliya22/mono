// SPDX-License-Identifier: MPL-2.0

import XCTest

@testable import KoruMobile

/// What this phone tells linked devices it runs: the keys are
/// `koru_device::BuildPayload`'s, the version is the bundle's, and a build
/// made without the git stamps advertises neither branch nor commit.
final class BuildPayloadTests: XCTestCase {
    private func keys(of payload: BuildPayload) throws -> [String: Any] {
        let data = try JSONEncoder().encode(payload)
        return try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
    }

    func testTheKeysAreTheRecordsAndTheVersionIsTheBundles() throws {
        let payload = BuildPayload.current(
            info: ["CFBundleShortVersionString": "0.1.0", "CFBundleVersion": "7"],
            gitStamp: ["branch": "mac-catch", "commit": "0123456789ab"])
        let json = try keys(of: payload)
        XCTAssertEqual(json["app_version"] as? String, "0.1.0 (7)")
        XCTAssertEqual(json["branch"] as? String, "mac-catch")
        XCTAssertEqual(json["commit"] as? String, "0123456789ab")
        XCTAssertEqual(Set(json.keys), ["app_version", "channel", "branch", "commit", "os", "os_version"])
        XCTAssertTrue((json["os"] as? String ?? "").hasPrefix("iOS "), "\(json)")
        // The protocol number is never the app's to say; the host stamps it.
        XCTAssertNil(json["peer_protocol"])
    }

    func testBlankStampsAdvertiseNoBranchAndNoCommit() throws {
        let payload = BuildPayload.current(
            info: ["CFBundleShortVersionString": "0.1.0", "CFBundleVersion": "1"],
            gitStamp: ["branch": "", "commit": ""])
        let json = try keys(of: payload)
        XCTAssertNil(json["branch"])
        XCTAssertNil(json["commit"])
        XCTAssertEqual(payload.label.components(separatedBy: " · ").count, 3)
        XCTAssertNil(BuildPayload.current(info: [:], gitStamp: nil).branch)
    }

    /// The profiles screen's quiet corner label, in Android's shape:
    /// `v{version} #{build} - {branch}`, from the bundle's two numbers and the
    /// stamp beside the sources.
    func testTheProfilesStampNamesTheVersionTheBuildAndTheBranch() {
        XCTAssertEqual(
            BuildPayload.stampLabel(
                info: ["CFBundleShortVersionString": "0.1.0", "CFBundleVersion": "7"],
                gitStamp: ["branch": "mac-catch", "commit": "0123456789ab"]),
            "v0.1.0 #7 - mac-catch")
    }

    /// Built where git could not answer, the label just ends — never a
    /// separator with nothing after it. Android's rule, mirrored.
    func testTheStampEndsWhenThereIsNoBranch() {
        let info = ["CFBundleShortVersionString": "0.1.0", "CFBundleVersion": "7"]
        XCTAssertEqual(BuildPayload.stampLabel(info: info, gitStamp: ["branch": ""]), "v0.1.0 #7")
        XCTAssertEqual(BuildPayload.stampLabel(info: info, gitStamp: nil), "v0.1.0 #7")
    }

    /// The stamp `koru.sh` wrote before this build is in the bundle — the
    /// test host is built by the same script, so this proves the resource
    /// path end to end rather than the parsing alone.
    func testTheBuildScriptsStampIsBundled() throws {
        let stamp = try XCTUnwrap(BuildPayload.bundledGitStamp(), "GitStamp.json is bundled by koru.sh")
        XCTAssertNotNil(stamp["commit"])
    }

    /// Mirrors koru-mobile-host
    /// a_fresh_root_advertises_its_build_and_the_list_says_so: the canonical
    /// Protobuf C shim takes the description and the device list carries it.
    func testAFreshRootAdvertisesThroughTheCShim() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("koru-native-\(UUID().uuidString)").path
        let secrets = Data(repeating: 47, count: 64)
        let build = BuildPayload(
            appVersion: "0.1.0 (1)", channel: "debug", branch: "mac-catch",
            commit: nil, os: "iOS 26.0", osVersion: nil)
        let outcome = try KoruNative.advertiseBuild(root: root, secrets: secrets, payload: build)
        XCTAssertTrue(outcome.ok)
        let devices = try KoruNative.devices(root: root, secrets: secrets)
        XCTAssertTrue(devices.hasThisDevice)
        XCTAssertEqual(devices.thisDevice.buildLabel, "0.1.0 (1) · debug · mac-catch · iOS 26.0")
    }
}

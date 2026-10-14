// SPDX-License-Identifier: MPL-2.0

import XCTest

@testable import KoruMobile

/// The licenses screen's text handling — the mirror of Android's
/// `OpenSourceLicensesTest`. That the screen reads the files the build
/// writes is held on the generator's side, in koru-release.
final class OpenSourceLicensesTests: XCTestCase {
    func testChunksRejoinToTheTextAndBreakAtBlankLines() {
        let text = (1...499).map { $0 % 10 == 0 ? "" : "line \($0)" }.joined(separator: "\n")
        let chunks = OpenSourceLicenses.chunks(text, maxLines: 40)
        XCTAssertEqual(chunks.joined(separator: "\n"), text)
        XCTAssertGreaterThan(chunks.count, 5)
        for chunk in chunks.dropLast() {
            let lines = chunk.components(separatedBy: "\n")
            XCTAssertTrue(lines.last?.isEmpty ?? false, chunk)
            XCTAssertTrue((40...50).contains(lines.count), "\(lines.count)")
        }
    }

    func testATextWithoutBlankLinesIsOneChunk() {
        let text = (1...300).map { "line \($0)" }.joined(separator: "\n")
        XCTAssertEqual(OpenSourceLicenses.chunks(text, maxLines: 40), [text])
        XCTAssertEqual(OpenSourceLicenses.chunks("a\r\nb\r\n"), ["a\nb"])
    }

    func testTheDocumentsAreTheThreeTextsInOrder() {
        XCTAssertEqual(
            OpenSourceLicenses.Document.allCases.map(\.rawValue),
            ["LICENSE.txt", "NOTICE.txt", "THIRD-PARTY-NOTICES.txt"])
    }
}

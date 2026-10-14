// SPDX-License-Identifier: MPL-2.0

import SwiftUI
import UIKit
import XCTest

@testable import KoruMobile

final class DocumentRenderingTests: XCTestCase {
    func testListMarkerLabelsMirrorAndroid() {
        XCTAssertEqual(listMarkerLabel(.bullet, ordinal: 3), "")
        XCTAssertEqual(listMarkerLabel(.checkbox, ordinal: 3), "")
        XCTAssertEqual(listMarkerLabel(.numbered, ordinal: 3), "3.")
        XCTAssertEqual(listMarkerLabel(.letter, ordinal: 1), "a.")
        XCTAssertEqual(listMarkerLabel(.letter, ordinal: 27), "a.")
        XCTAssertEqual(listMarkerLabel(.roman, ordinal: 4), "iv.")
        XCTAssertEqual(listMarkerLabel(.roman, ordinal: 1994), "mcmxciv.")
    }

    /// The ramp is `theme::type_step`'s, not a phone invention: TYPOGRAPHY's
    /// document heading table says 16 / 30 / 24 / 20 / 17 / 16 / 14, and these
    /// are those numbers at the default text size. Pinned by value because the
    /// point of the item was that the two platforms had each guessed a ladder.
    func testTheHeadingRampIsTheDesktops() {
        let sizes = [nil, 1, 2, 3, 4, 5, 6].map {
            documentBlockFont(headingLevel: $0.map(UInt32.init), sizeCategory: .large).pointSize
        }
        XCTAssertEqual(sizes, [16, 30, 24, 20, 17, 16, 14])
        // H5 is body size and H6 is below it — that is what those levels are
        // for — so they are told apart by weight, never by size.
        let body = documentBlockFont(headingLevel: nil, sizeCategory: .large)
        XCTAssertNotEqual(documentBlockFont(headingLevel: 5, sizeCategory: .large), body)
        XCTAssertNotEqual(documentBlockFont(headingLevel: 6, sizeCategory: .large), body)
    }

    /// One `UIFontMetrics` scales every level, so an accessibility size grows
    /// the ramp instead of reshaping it: an H1 stays above the body at both
    /// ends of the slider. Per-level text styles would not hold this.
    func testTheRampKeepsItsShapeAtAnAccessibilitySize() {
        for category in [UIContentSizeCategory.extraSmall, .large, .accessibilityExtraExtraLarge] {
            let body = documentBlockFont(headingLevel: nil, sizeCategory: category).pointSize
            let h1 = documentBlockFont(headingLevel: 1, sizeCategory: category).pointSize
            let h3 = documentBlockFont(headingLevel: 3, sizeCategory: category).pointSize
            XCTAssertGreaterThan(h1, h3, "\(category)")
            XCTAssertGreaterThan(h3, body, "\(category)")
        }
        XCTAssertGreaterThan(
            documentBlockFont(headingLevel: nil, sizeCategory: .accessibilityExtraExtraLarge).pointSize,
            documentBlockFont(headingLevel: nil, sizeCategory: .large).pointSize)
    }

    /// Space above comes from the same table and there is none below; a body
    /// row gets none at all.
    func testHeadingSpaceAboveIsTheRampsAndBodyGetsNone() {
        XCTAssertEqual(
            [nil, 1, 2, 3, 4, 5, 6].map { documentHeadingSpaceAbove($0.map(UInt32.init)) },
            [0, 28, 24, 20, 16, 14, 12])
    }

    /// `start` and `end` are writing-direction words, not left and right: an
    /// end-aligned paragraph hugs the left edge in an Arabic interface. Only
    /// `end` flips — `.natural` already does it for `start`.
    func testAlignmentFollowsTheWritingDirection() {
        XCTAssertEqual(documentTextAlignment(.start, layoutDirection: .leftToRight), .natural)
        XCTAssertEqual(documentTextAlignment(.start, layoutDirection: .rightToLeft), .natural)
        XCTAssertEqual(documentTextAlignment(.center, layoutDirection: .rightToLeft), .center)
        XCTAssertEqual(documentTextAlignment(.justify, layoutDirection: .leftToRight), .justified)
        XCTAssertEqual(documentTextAlignment(.end, layoutDirection: .leftToRight), .right)
        XCTAssertEqual(documentTextAlignment(.end, layoutDirection: .rightToLeft), .left)
    }

    /// A quote keeps the block's own face and slants it, rather than owning a
    /// second size ladder.
    func testAQuoteSlantsTheBlocksOwnFace() {
        let base = documentBlockFont(headingLevel: 2, sizeCategory: .large)
        let quoted = italicized(base)
        XCTAssertEqual(quoted.pointSize, base.pointSize)
        XCTAssertTrue(quoted.fontDescriptor.symbolicTraits.contains(.traitItalic))
    }

    /// The Swift half of Android's `parseMarkdownTable`, on the string from
    /// `markdown_table_separator_is_not_shown_as_data` — the alignment row is
    /// layout instruction, not a row of data.
    func testMarkdownTableSeparatorIsNotShownAsData() {
        XCTAssertEqual(
            parseMarkdownTable("| Name | Value |\n|:---|---:|\n| Koru | 1 |"),
            [["Name", "Value"], ["Koru", "1"]])
    }

    /// Something that is not a table parses to nothing, and the caller shows
    /// the source instead of drawing an empty grid.
    func testANonTableParsesToNothingOrToItself() {
        XCTAssertEqual(parseMarkdownTable(""), [])
        XCTAssertEqual(parseMarkdownTable("   \n\n"), [])
        // A second row of real data is kept: only a separator row is dropped.
        XCTAssertEqual(
            parseMarkdownTable("| a |\n| b |"), [["a"], ["b"]])
    }

    /// A mark whose UTF-8 offsets sit after a four-byte emoji must land on the
    /// right UTF-16 range — "a😀b" is 1+4+1 UTF-8 bytes but 1+2+1 UTF-16 units.
    func testMarkOffsetsCrossAnEmoji() {
        let text = "a😀b"
        let range = NativeRichTextEditor.Coordinator.utf16Range(UTF8Range(start: 5, end: 6), in: text)
        XCTAssertEqual(range, NSRange(location: 3, length: 1))
        let emoji = NativeRichTextEditor.Coordinator.utf16Range(UTF8Range(start: 1, end: 5), in: text)
        XCTAssertEqual(emoji, NSRange(location: 1, length: 2))
    }

    /// A date pill carries no label; its reference is a label reference holding
    /// the host's ISO `yyyy-MM-dd` string, and that string is what the pill
    /// shows — the Android `referenceLabel` fallback, mirrored.
    func testAPillWithoutALabelShowsItsReferenceValue() {
        let date = InlineObject(
            id: InlineObjectID("a"), anchorUTF8: 0, role: "date",
            reference: .label("2026-08-24"), label: nil)
        XCTAssertEqual(NativeRichTextEditor.Coordinator.pillLabel(date), "2026-08-24")
        let named = InlineObject(
            id: InlineObjectID("b"), anchorUTF8: 0, role: "document",
            reference: .label("2026-08-24"), label: "mvp docs")
        XCTAssertEqual(NativeRichTextEditor.Coordinator.pillLabel(named), "mvp docs")
        let url = InlineObject(
            id: InlineObjectID("c"), anchorUTF8: 0, role: "url",
            reference: .url("https://koru.do"), label: nil)
        XCTAssertEqual(NativeRichTextEditor.Coordinator.pillLabel(url), "https://koru.do")
    }

    /// Offsets that split an emoji's bytes answer nil and the mark is skipped,
    /// never a corrupted run.
    func testAMarkInsideAnEmojiIsRefused() {
        XCTAssertNil(NativeRichTextEditor.Coordinator.utf16Range(UTF8Range(start: 2, end: 5), in: "a😀b"))
        XCTAssertNil(NativeRichTextEditor.Coordinator.utf16Range(UTF8Range(start: 0, end: 9), in: "a😀b"))
    }
}

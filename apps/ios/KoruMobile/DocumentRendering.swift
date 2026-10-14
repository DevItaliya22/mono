// SPDX-License-Identifier: MPL-2.0

import SwiftUI
import UIKit

// Marker labels, block faces, alignment and the markdown table reader for the
// document surface. The Android mirror is `listOrdinal`/`roman`, the heading
// style switch and `parseMarkdownTable` in `DocumentEditor.kt`; these are free
// functions so `DocumentRenderingTests` can pin them.

func listMarkerLabel(_ style: KoruListStyle, ordinal: Int) -> String {
    switch style {
    case .bullet, .checkbox: return ""
    case .numbered: return "\(ordinal)."
    case .letter:
        let index = max(ordinal - 1, 0) % 26
        return "\(Character(UnicodeScalar(UInt8(97 + index))))."
    case .roman: return "\(romanNumeral(ordinal))."
    }
}

func romanNumeral(_ value: Int) -> String {
    var number = max(value, 1)
    let table: [(Int, String)] = [
        (1000, "m"), (900, "cm"), (500, "d"), (400, "cd"), (100, "c"), (90, "xc"),
        (50, "l"), (40, "xl"), (10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i"),
    ]
    var result = ""
    for (amount, glyph) in table {
        while number >= amount {
            result += glyph
            number -= amount
        }
    }
    return result
}

/// One lane for every marker, whatever its style — the desktop right-aligns a
/// numbered marker to a fixed `marker_right_x` so a wrapped line does not tuck
/// under it (DESIGN §List markers). Three different widths — which is what the
/// bullet, the ordinal and the checkbox each had — start the text column in a
/// different place on each row of the same list.
let documentMarkerLane: CGFloat = 28

/// The heading ramp, from `theme::type_step` — TYPOGRAPHY §The document heading
/// ramp is the authority, and these are its sizes: body 16, then 30/24/20/17/16/14.
/// H5 is body size and H6 is below it on purpose; those levels are told apart by
/// weight, which is why the size alone never identifies one.
///
/// Sizes are **scaled through one `UIFontMetrics`**, the body's, rather than
/// each level taking its own text style: `.largeTitle` and `.body` grow at
/// different rates, so per-level styles would reshape the ramp at every
/// accessibility size, and the ramp is the thing being matched.
func documentBlockFont(headingLevel: UInt32?, sizeCategory: UIContentSizeCategory? = nil) -> UIFont {
    // 650 (H3 on the desktop) has no San Francisco face; the named weights are
    // the vocabulary here, so it resolves to the semibold it would land on.
    let (size, weight): (CGFloat, UIFont.Weight) = switch headingLevel {
    case 1?: (30, .bold)
    case 2?: (24, .bold)
    case 3?: (20, .semibold)
    case 4?: (17, .semibold)
    case 5?: (16, .semibold)
    case 6?: (14, .semibold)
    default: (16, .regular)
    }
    let base = UIFont.systemFont(ofSize: size, weight: weight)
    let metrics = UIFontMetrics(forTextStyle: .body)
    guard let category = sizeCategory else { return metrics.scaledFont(for: base) }
    return metrics.scaledFont(
        for: base, compatibleWith: UITraitCollection(preferredContentSizeCategory: category))
}

/// The same face, slanted — a quote keeps the block's own type and adds the
/// frame, so its italic is a trait on whatever font the level chose rather than
/// a second size ladder.
func italicized(_ font: UIFont) -> UIFont {
    guard let descriptor = font.fontDescriptor.withSymbolicTraits(.traitItalic) else { return font }
    return UIFont(descriptor: descriptor, size: font.pointSize)
}

/// Space above a heading, from the same ramp table — 28/24/20/16/14/12, and
/// none below, because the row that follows brings its own line box. The value
/// is the **whole** gap the desktop puts above the heading, so a caller that
/// already lays its rows out on a uniform gap adds only the difference.
func documentHeadingSpaceAbove(_ headingLevel: UInt32?) -> CGFloat {
    switch headingLevel {
    case 1?: 28
    case 2?: 24
    case 3?: 20
    case 4?: 16
    case 5?: 14
    case 6?: 12
    default: 0
    }
}

/// `BlockProperties.alignment` as TextKit reads it. `start` is `.natural` and
/// `end` flips with the writing direction, so an Arabic document's
/// start-aligned paragraph hugs the right edge without the document storing a
/// second answer — PRINCIPLES' start/end rule, kept out of the drawing.
func documentTextAlignment(
    _ alignment: KoruAlignment, layoutDirection: LayoutDirection = .leftToRight
) -> NSTextAlignment {
    switch alignment {
    case .start: .natural
    case .center: .center
    case .end: layoutDirection == .rightToLeft ? .left : .right
    case .justify: .justified
    }
}

/// The rows of a Markdown table, separator row dropped — the Swift half of
/// Android's `parseMarkdownTable`, on the same source string, so a table drawn
/// on one phone is the table drawn on the other. An empty answer means this is
/// not a table, and the caller shows the source instead.
func parseMarkdownTable(_ source: String) -> [[String]] {
    let lines = source.split(separator: "\n", omittingEmptySubsequences: false)
        .map(String.init)
        .filter { !$0.trimmingCharacters(in: .whitespaces).isEmpty }
    if lines.isEmpty { return [] }
    let rows = lines.map { line -> [String] in
        var trimmed = Substring(line.trimmingCharacters(in: .whitespaces))
        while trimmed.first == "|" { trimmed = trimmed.dropFirst() }
        while trimmed.last == "|" { trimmed = trimmed.dropLast() }
        return trimmed.split(separator: "|", omittingEmptySubsequences: false)
            .map { $0.trimmingCharacters(in: .whitespaces) }
    }
    return rows.enumerated()
        .filter { index, cells in index != 1 || !cells.allSatisfy(isSeparatorCell) }
        .map(\.element)
}

/// Android's `:?-{3,}:?` — the alignment row under a Markdown table's header,
/// which is layout instruction rather than a row of data.
private func isSeparatorCell(_ cell: String) -> Bool {
    var body = Substring(cell)
    if body.first == ":" { body = body.dropFirst() }
    if body.last == ":" { body = body.dropLast() }
    return body.count >= 3 && body.allSatisfy { $0 == "-" }
}

// SPDX-License-Identifier: MPL-2.0

import SwiftUI
import UIKit

struct NativeRichTextEditor: UIViewRepresentable {
    let richText: RichText
    /// The block's base face — body for a plain paragraph, larger and heavier
    /// for a heading. Marks derive their fonts from this so a bold word inside
    /// a heading keeps the heading's size.
    var baseFont: UIFont = .preferredFont(forTextStyle: .body)
    /// The block's own alignment, already resolved against the writing
    /// direction by `documentTextAlignment` — the view never asks again, so a
    /// start-aligned Arabic paragraph and a start-aligned English one are the
    /// same value here.
    var alignment: NSTextAlignment = .natural
    /// A tap on a pill hands its reference here instead of placing the caret.
    var onFollow: (EntityReference) -> Void = { _ in }
    let onCommand: (RichTextEdit) -> Void

    enum RichTextEdit {
        case insert(atUTF8: Int, text: String)
        case delete(UTF8Range)
    }

    func makeCoordinator() -> Coordinator { Coordinator(onCommand: onCommand) }
    func makeUIView(context: Context) -> UITextView {
        let view = UITextView(usingTextLayoutManager: true)
        view.backgroundColor = .clear
        view.font = baseFont
        view.adjustsFontForContentSizeCategory = true
        view.isScrollEnabled = false
        view.textContainerInset = .zero
        view.textContainer.lineFragmentPadding = 0
        // Without these a UITextView refuses horizontal compression inside
        // SwiftUI, so a long paragraph grew past the screen edge instead of
        // wrapping. sizeThatFits below is the other half: it answers the
        // wrapped height for the width SwiftUI actually proposes.
        view.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        view.setContentHuggingPriority(.defaultLow, for: .vertical)
        view.delegate = context.coordinator
        // The recognizer claims a touch only when it lands on a pill (see
        // `gestureRecognizer(_:shouldReceive:)`), so ordinary taps still place
        // the caret and edit.
        let tap = UITapGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.tapped(_:)))
        tap.delegate = context.coordinator
        view.addGestureRecognizer(tap)
        context.coordinator.render(richText, in: view, baseFont: baseFont, alignment: alignment)
        return view
    }

    func updateUIView(_ view: UITextView, context: Context) {
        context.coordinator.onCommand = onCommand
        context.coordinator.onFollow = onFollow
        guard context.coordinator.model != richText || context.coordinator.baseFont != baseFont
            || context.coordinator.alignment != alignment
        else { return }
        context.coordinator.render(richText, in: view, baseFont: baseFont, alignment: alignment)
    }

    func sizeThatFits(_ proposal: ProposedViewSize, uiView: UITextView, context: Context) -> CGSize? {
        let width = proposal.width ?? UIScreen.main.bounds.width
        let fitted = uiView.sizeThatFits(CGSize(width: width, height: .greatestFiniteMagnitude))
        return CGSize(width: width, height: fitted.height)
    }

    final class Coordinator: NSObject, UITextViewDelegate, UIGestureRecognizerDelegate {
        var model = RichText(text: "")
        var baseFont: UIFont = .preferredFont(forTextStyle: .body)
        var alignment: NSTextAlignment = .natural
        var onCommand: (RichTextEdit) -> Void
        var onFollow: (EntityReference) -> Void = { _ in }
        /// UTF-16 attachment ranges of the rendered pills, in text order, so a
        /// tap can be answered with the reference it landed on.
        private var pillRanges: [(NSRange, EntityReference)] = []
        private var rendering = false
        init(onCommand: @escaping (RichTextEdit) -> Void) { self.onCommand = onCommand }

        func render(
            _ value: RichText, in view: UITextView, baseFont: UIFont, alignment: NSTextAlignment
        ) {
            rendering = true; defer { rendering = false }
            model = value
            self.baseFont = baseFont
            self.alignment = alignment
            view.font = baseFont
            // Both halves are needed: the paragraph style aligns the text that
            // is here, and `textAlignment` aligns what is typed into an empty
            // block, where there is no run to carry a style.
            view.textAlignment = alignment
            let paragraph = NSMutableParagraphStyle()
            paragraph.alignment = alignment
            let attributed = NSMutableAttributedString(
                string: value.text,
                attributes: [
                    .font: baseFont, .foregroundColor: UIColor.label,
                    .paragraphStyle: paragraph,
                ])
            for mark in value.marks {
                guard let range = Self.utf16Range(mark.range, in: value.text) else { continue }
                switch mark.kind {
                case .bold: attributed.addAttribute(.font, value: UIFont.systemFont(ofSize: baseFont.pointSize, weight: .bold), range: range)
                case .italic: attributed.addAttribute(.font, value: UIFont.italicSystemFont(ofSize: baseFont.pointSize), range: range)
                case .underline: attributed.addAttribute(.underlineStyle, value: NSUnderlineStyle.single.rawValue, range: range)
                case .strike: attributed.addAttribute(.strikethroughStyle, value: NSUnderlineStyle.single.rawValue, range: range)
                default: break
                }
            }
            pillRanges = []
            for object in value.inlineObjects {
                guard let range = Self.utf16Range(UTF8Range(start: object.anchorUTF8, end: object.anchorUTF8 + 3), in: value.text) else { continue }
                let attachment = NSTextAttachment()
                let image = pillImage(Self.pillLabel(object))
                attachment.image = image
                attachment.bounds = CGRect(x: 0, y: baseFont.descender, width: image.size.width, height: image.size.height)
                // TextKit draws an attachment only for U+FFFC, and the anchor
                // is U+2060 — adding the attribute in place rendered nothing.
                // Replace the anchor with a real attachment string; both are
                // one UTF-16 unit, so every other range keeps its offsets.
                attributed.replaceCharacters(in: range, with: NSAttributedString(attachment: attachment))
                pillRanges.append((range, object.reference))
            }
            let selection = view.selectedRange
            view.attributedText = attributed
            view.selectedRange = NSRange(location: min(selection.location, attributed.length), length: 0)
        }

        func textView(_ textView: UITextView, shouldChangeTextIn range: NSRange, replacementText text: String) -> Bool {
            guard !rendering, !text.contains("\n"), let swiftRange = Range(range, in: model.text) else { return false }
            let prefix = model.text[..<swiftRange.lowerBound]
            let start = prefix.utf8.count
            let removed = model.text[swiftRange].utf8.count
            if removed > 0 { onCommand(.delete(UTF8Range(start: start, end: start + removed))) }
            if !text.isEmpty { onCommand(.insert(atUTF8: start, text: text)) }
            return false
        }

        /// What a pill says. The stored label wins; without one the reference's
        /// own value carries — which is how a date pill shows its `yyyy-MM-dd`
        /// (the host sends a date as a label reference holding the ISO string,
        /// and Android's `referenceLabel` falls back the same way).
        static func pillLabel(_ object: InlineObject) -> String {
            if let label = object.label, !label.isEmpty { return label }
            switch object.reference {
            case let .url(value): return value
            case let .label(value): return value
            case .document, .block, .user: return String(localized: "reference")
            }
        }

        func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer, shouldReceive touch: UITouch) -> Bool {
            guard let view = gestureRecognizer.view as? UITextView else { return false }
            return pill(at: touch.location(in: view), in: view) != nil
        }

        @objc func tapped(_ gesture: UITapGestureRecognizer) {
            guard let view = gesture.view as? UITextView,
                  let reference = pill(at: gesture.location(in: view), in: view) else { return }
            onFollow(reference)
        }

        private func pill(at point: CGPoint, in view: UITextView) -> EntityReference? {
            guard let position = view.closestPosition(to: point) else { return nil }
            let index = view.offset(from: view.beginningOfDocument, to: position)
            // `closestPosition` answers either side of the attachment glyph,
            // so both boundaries count as landing on it.
            return pillRanges.first { index >= $0.0.location && index <= $0.0.location + $0.0.length }?.1
        }

        /// UTF-8 mark offsets → a UTF-16 `NSRange`, or nil when an offset does
        /// not land on a character boundary — which is what keeps a mark that
        /// would split an emoji's four bytes from corrupting the run. Static
        /// and internal so the boundary math is unit-tested.
        static func utf16Range(_ range: UTF8Range, in text: String) -> NSRange? {
            guard range.start >= 0, range.end >= range.start, range.end <= text.utf8.count else { return nil }
            let lowerUTF8 = text.utf8.index(text.utf8.startIndex, offsetBy: range.start)
            let upperUTF8 = text.utf8.index(text.utf8.startIndex, offsetBy: range.end)
            guard let lower = String.Index(lowerUTF8, within: text), let upper = String.Index(upperUTF8, within: text) else { return nil }
            return NSRange(lower..<upper, in: text)
        }

        private func pillImage(_ text: String) -> UIImage {
            let label = UILabel(); label.text = "  \(text)  "; label.font = .preferredFont(forTextStyle: .caption1); label.textColor = .white; label.backgroundColor = UIColor(KoruColor.accent); label.layer.cornerRadius = 6; label.clipsToBounds = true; label.sizeToFit()
            return UIGraphicsImageRenderer(size: label.bounds.size).image { context in label.layer.render(in: context.cgContext) }
        }
    }
}

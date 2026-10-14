// SPDX-License-Identifier: MPL-2.0

import XCTest

/// **Every key in both string catalogues exists in all eight languages.**
///
/// The desktop gets this from the type system — `koru_i18n::strings::Row` has
/// eight fields, so a forgotten translation does not compile. A string catalog
/// has no such gate, and the failure mode is worse than Android's: Xcode
/// compiles one `.strings` table per language holding **only** that language's
/// translated entries, and `NSLocalizedString` on a key the chosen table does
/// not have returns *the key*. There is no fall-back to the development
/// language. So a missing German translation does not draw the English word —
/// it draws `add_image`. Android's silent fall-back to `values/` is the
/// friendlier bug and neither is acceptable; this is the gate for the iOS half
/// and `StringResourceTranslationTest` is the gate for the other.
///
/// It reads the catalogues from the source tree through `#filePath` rather
/// than from a bundle, so it needs no host application, no simulator
/// localization and no network.
final class LocalizationCatalogTests: XCTestCase {
    /// The seven beside English, spelled as the catalogues spell them.
    private let languages = ["es", "pt-BR", "de", "fr", "ar", "zh-Hans", "ja"]

    private var catalogs: [String] {
        let ios = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        return [
            ios.appendingPathComponent("KoruMobile/Localizable.xcstrings").path,
            ios.appendingPathComponent("KoruShare/Localizable.xcstrings").path,
        ]
    }

    private func strings(at path: String) throws -> [String: [String: Any]] {
        let data = try Data(contentsOf: URL(fileURLWithPath: path))
        let root = try XCTUnwrap(
            try JSONSerialization.jsonObject(with: data) as? [String: Any])
        return try XCTUnwrap(root["strings"] as? [String: [String: Any]])
    }

    /// The value for one language, whether it is a plain unit or a plural.
    /// A plural's forms are returned together; the placeholder check treats
    /// them as one body, which is what it wants.
    private func value(_ entry: [String: Any], _ language: String) -> String? {
        guard let localizations = entry["localizations"] as? [String: [String: Any]],
            let one = localizations[language]
        else { return nil }
        if let unit = one["stringUnit"] as? [String: Any] {
            return unit["value"] as? String
        }
        guard let variations = one["variations"] as? [String: Any],
            let plural = variations["plural"] as? [String: [String: Any]]
        else { return nil }
        return plural.values
            .compactMap { ($0["stringUnit"] as? [String: Any])?["value"] as? String }
            .sorted()
            .joined(separator: " ")
    }

    /// `%@`, `%d`, `%1$@` — what a translation must carry through, or the
    /// screen throws where it draws, in one language, on somebody else's
    /// phone.
    private func placeholders(_ value: String) -> [String] {
        let pattern = try! NSRegularExpression(pattern: "%(?:[0-9]+\\$)?[@d]")
        let range = NSRange(value.startIndex..., in: value)
        return pattern.matches(in: value, range: range)
            .compactMap { Range($0.range, in: value).map { r in String(value[r]) } }
            .sorted()
    }

    func testEveryKeyIsTranslatedInEveryLanguage() throws {
        for path in catalogs {
            let name = URL(fileURLWithPath: path).deletingLastPathComponent().lastPathComponent
            let entries = try strings(at: path)
            XCTAssertFalse(entries.isEmpty, "\(name) parsed to nothing")
            for (key, entry) in entries {
                // `shouldTranslate: false` is a claim that the word is the
                // same everywhere — a product name — and is the only way to
                // be absent from a language.
                if entry["shouldTranslate"] as? Bool == false { continue }
                XCTAssertNotNil(value(entry, "en"), "\(name)/\(key) has no English")
                for language in languages {
                    XCTAssertNotNil(
                        value(entry, language), "\(name)/\(key) is missing \(language)")
                }
            }
        }
    }

    func testATranslationCarriesTheSamePlaceholdersAsItsEnglish() throws {
        for path in catalogs {
            let name = URL(fileURLWithPath: path).deletingLastPathComponent().lastPathComponent
            for (key, entry) in try strings(at: path) {
                guard let english = value(entry, "en") else { continue }
                let expected = placeholders(english)
                for language in languages {
                    guard let translated = value(entry, language) else { continue }
                    XCTAssertEqual(
                        placeholders(translated), expected,
                        "\(name)/\(key) in \(language) does not carry English's placeholders")
                }
            }
        }
    }

    /// Not a style rule: `String(format:)` with no locale formats `%d` as
    /// ASCII regardless of language, so a number in an Arabic sentence comes
    /// out in Western digits. Every manual format in this app passes
    /// `locale: .current`, and this is what says so after somebody deletes it.
    func testEveryManualStringFormatNamesALocale() throws {
        let sources = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let files = FileManager.default.enumerator(atPath: sources.path)?
            .compactMap { $0 as? String }
            // Not DerivedData, and not this file — which names the pattern it
            // is looking for and would otherwise fail on its own predicate.
            .filter {
                $0.hasSuffix(".swift") && !$0.contains("DerivedData")
                    && !$0.hasSuffix("LocalizationCatalogTests.swift")
            } ?? []
        for file in files {
            let text = try String(contentsOf: sources.appendingPathComponent(file), encoding: .utf8)
            for line in text.components(separatedBy: .newlines)
            where line.contains("String(format:") && line.contains("String(localized:") {
                // A `%02x` over hash bytes is machinery and must *not* take a
                // locale; a localized format string is drawn to a person and
                // must. Naming `String(localized:` is what tells them apart.
                XCTAssertTrue(
                    line.contains("locale:"),
                    "\(file) formats a localized string without a locale: \(line.trimmingCharacters(in: .whitespaces))")
            }
        }
    }
}

// SPDX-License-Identifier: MPL-2.0

import SwiftUI

/// **What the build bundles in `licenses/`**, named once — the iOS mirror of
/// Android's `OpenSourceLicenses`. The "Generate open-source notices" build
/// phase writes these files (`koru.sh notices`, which runs `koru-release
/// notices ios`), and koru-release's
/// `the_ios_screen_reads_the_files_the_generator_writes` holds the names here
/// to the names it writes.
enum OpenSourceLicenses {
    static let directory = "licenses"
    static let source = "SOURCE.txt"

    /// The documents the screen lists, in order.
    enum Document: String, CaseIterable, Identifiable, Hashable {
        case license = "LICENSE.txt"
        case notice = "NOTICE.txt"
        case thirdParty = "THIRD-PARTY-NOTICES.txt"

        var id: Self { self }

        var title: LocalizedStringKey {
            switch self {
            case .license: "open_source_license_koru"
            case .notice: "open_source_license_notice"
            case .thirdParty: "open_source_license_third_party"
            }
        }
    }

    /// One bundled file's text, or `nil` when the build carries none.
    static func read(_ file: String, in bundle: Bundle = .main) -> String? {
        let name = (file as NSString).deletingPathExtension
        let ext = (file as NSString).pathExtension
        guard let url = bundle.url(forResource: name, withExtension: ext, subdirectory: directory)
        else { return nil }
        return try? String(contentsOf: url, encoding: .utf8)
    }

    /// **A long text in pieces a lazy stack can draw.** The third-party
    /// notices are hundreds of kilobytes; one `Text` holding all of it lays
    /// out every line before the first frame. Pieces break at blank lines,
    /// are at most `maxLines` lines unless one paragraph is longer, and join
    /// back to the original text with "\n" — Android's `chunks`, line for line.
    static func chunks(_ text: String, maxLines: Int = 80) -> [String] {
        var lines = text.replacingOccurrences(of: "\r\n", with: "\n").components(separatedBy: "\n")
        while lines.last == "" { lines.removeLast() }
        var out: [String] = []
        var current: [String] = []
        for line in lines {
            current.append(line)
            if line.trimmingCharacters(in: .whitespaces).isEmpty && current.count >= maxLines {
                out.append(current.joined(separator: "\n"))
                current = []
            }
        }
        if !current.isEmpty { out.append(current.joined(separator: "\n")) }
        return out
    }
}

/// Settings → Open-source licenses: the source of this build, and the three
/// documents.
struct OpenSourceLicensesScreen: View {
    @State private var source: URL?

    var body: some View {
        List {
            Section {
                Text("open_source_licenses_intro")
                if let source {
                    Link(destination: source) {
                        VStack(alignment: .leading, spacing: KoruSpace.xSmall) {
                            Text("open_source_source_code")
                            Text(verbatim: source.absoluteString)
                                .font(.caption)
                                .foregroundStyle(KoruColor.subtle)
                        }
                    }
                }
            }
            Section {
                ForEach(OpenSourceLicenses.Document.allCases) { document in
                    NavigationLink(document.title) { OpenSourceLicenseDocumentScreen(document: document) }
                }
            }
        }
        .navigationTitle("open_source_licenses")
        .task {
            source = OpenSourceLicenses.read(OpenSourceLicenses.source)
                .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
                .flatMap { $0.hasPrefix("https://") ? URL(string: $0) : nil }
        }
    }
}

/// One of the documents, read from the bundle off the main thread.
struct OpenSourceLicenseDocumentScreen: View {
    let document: OpenSourceLicenses.Document
    @State private var chunks: [String]?

    var body: some View {
        Group {
            if let chunks {
                if chunks.isEmpty {
                    Text("open_source_licenses_missing")
                        .foregroundStyle(KoruColor.subtle)
                        .padding(KoruSpace.medium)
                } else {
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 0) {
                            ForEach(chunks.indices, id: \.self) { index in
                                Text(verbatim: chunks[index])
                                    .font(.system(.caption, design: .monospaced))
                                    .textSelection(.enabled)
                                    .frame(maxWidth: .infinity, alignment: .leading)
                            }
                        }
                        .padding(KoruSpace.medium)
                    }
                }
            } else {
                ProgressView()
            }
        }
        .navigationTitle(document.title)
        .navigationBarTitleDisplayMode(.inline)
        .task {
            let file = document.rawValue
            chunks = await Task.detached(priority: .userInitiated) {
                OpenSourceLicenses.read(file).map { OpenSourceLicenses.chunks($0) } ?? []
            }.value
        }
    }
}

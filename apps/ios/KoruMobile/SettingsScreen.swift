// SPDX-License-Identifier: MPL-2.0

import SwiftUI

/// Appearance, language and the open-source licenses — the iOS mirror of
/// Android's `SettingsScreen`.
///
/// The language row links to the system's per-app language screen rather than
/// mimicking Android's activity-recreate: once an app ships more than one
/// localization, iOS owns that choice natively and a second in-app picker
/// would fight it.
struct SettingsScreen: View {
    @AppStorage("appearance") private var appearance: Appearance = .system

    enum Appearance: String, CaseIterable, Identifiable {
        case system, light, dark
        var id: Self { self }

        var scheme: ColorScheme? {
            switch self {
            case .system: nil
            case .light: .light
            case .dark: .dark
            }
        }

        var label: LocalizedStringKey {
            switch self {
            case .system: "appearance_system"
            case .light: "appearance_light"
            case .dark: "appearance_dark"
            }
        }
    }

    var body: some View {
        List {
            Section("appearance") {
                Picker("appearance", selection: $appearance) {
                    ForEach(Appearance.allCases) { choice in Text(choice.label).tag(choice) }
                }
                .pickerStyle(.segmented)
            }
            Section("language") {
                if let url = URL(string: UIApplication.openSettingsURLString) {
                    Link("open_language_settings", destination: url)
                }
                Text("language_explainer").font(.caption).foregroundStyle(KoruColor.subtle)
            }
            Section {
                NavigationLink("open_source_licenses") { OpenSourceLicensesScreen() }
                Text("open_source_licenses_subtitle").font(.caption).foregroundStyle(KoruColor.subtle)
            }
        }
        .navigationTitle("settings")
    }
}

extension View {
    /// Applied at the root so the whole scene follows the override.
    func koruAppearance() -> some View {
        modifier(KoruAppearanceModifier())
    }
}

private struct KoruAppearanceModifier: ViewModifier {
    @AppStorage("appearance") private var appearance: SettingsScreen.Appearance = .system
    func body(content: Content) -> some View {
        content.preferredColorScheme(appearance.scheme)
    }
}

// SPDX-License-Identifier: MPL-2.0

import SwiftUI

enum KoruSpace {
    static let xSmall: CGFloat = 4
    static let small: CGFloat = 8
    static let medium: CGFloat = 16
    static let large: CGFloat = 24
    static let xLarge: CGFloat = 32
}

enum KoruColor {
    static let accent = Color(red: 0.22, green: 0.42, blue: 0.36)
    static let surface = Color(uiColor: .secondarySystemBackground)
    static let subtle = Color(uiColor: .secondaryLabel)
    static let danger = Color(red: 0.72, green: 0.18, blue: 0.18)
    /// A warning that is not yet a failure — a linked device a minor version
    /// apart. Amber, as the desktop's notice tone; danger is the red Unlink wears.
    static let notice = Color(red: 0.72, green: 0.48, blue: 0.08)
}

/// Named entity colours shared with the desktop and Android switchers.
enum KoruEntityColor {
    static func rgb(_ token: String?, dark: Bool) -> UInt32? {
        let light: [String: UInt32] = [
            "blue": 0x2F6FB5, "purple": 0x6B4FC4, "teal": 0x1F7D75,
            "green": 0x3B7D44, "olive": 0x746A2C, "orange": 0xC06A1E,
            "red": 0xB33D35, "magenta": 0xA83C77, "slate": 0x555F6B,
            "stone": 0x6E6660,
        ]
        let darkValues: [String: UInt32] = [
            "blue": 0x5E9CE0, "purple": 0x9B86EA, "teal": 0x45ADA2,
            "green": 0x66AE70, "olive": 0xA69B4F, "orange": 0xE0964A,
            "red": 0xE06C63, "magenta": 0xD46EA6, "slate": 0x8B96A4,
            "stone": 0x9E948C,
        ]
        guard let key = token?.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        else { return nil }
        return (dark ? darkValues : light)[key]
    }

    static func color(_ token: String?, scheme: ColorScheme) -> Color? {
        guard let value = rgb(token, dark: scheme == .dark) else { return nil }
        return Color(
            red: Double((value >> 16) & 0xff) / 255,
            green: Double((value >> 8) & 0xff) / 255,
            blue: Double(value & 0xff) / 255)
    }
}

enum KoruType {
    static let title = Font.system(.title2, design: .rounded, weight: .semibold)
    static let body = Font.system(.body, design: .default)
    static let editorial = Font.system(.body, design: .serif)
    static let label = Font.system(.subheadline, design: .rounded, weight: .medium)
}

struct KoruPrimaryButton: View {
    let title: LocalizedStringKey
    let action: () -> Void
    var body: some View {
        Button(action: action) { Text(title).font(KoruType.label).frame(maxWidth: .infinity).padding(.vertical, KoruSpace.small) }
            .buttonStyle(.borderedProminent).tint(KoruColor.accent)
    }
}

struct KoruEmptyState: View {
    let title: LocalizedStringKey
    var body: some View { ContentUnavailableView(title, systemImage: "tray") }
}

struct KoruRow<Content: View>: View {
    let content: Content
    init(@ViewBuilder content: () -> Content) { self.content = content() }
    var body: some View { content.padding(.vertical, KoruSpace.xSmall) }
}

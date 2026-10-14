// SPDX-License-Identifier: MPL-2.0

import SwiftUI

@main
struct KoruMobileApp: App {
    @StateObject private var model = AppModel(services: .native())
    var body: some Scene { WindowGroup { RootView().environmentObject(model).tint(KoruColor.accent).koruAppearance()
            .onOpenURL { model.acceptLink($0) }
            .onContinueUserActivity(NSUserActivityTypeBrowsingWeb) { activity in
                if let url = activity.webpageURL { model.acceptLink(url) }
            } } }
}

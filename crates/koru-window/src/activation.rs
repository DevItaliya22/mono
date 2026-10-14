// SPDX-License-Identifier: MPL-2.0

//! Bringing this application to the front, on the platform that needs asking.

/// Bring this application to the front.
///
/// **macOS needs this and no other platform does.** `Window::focus_window` is
/// `makeKeyAndOrderFront:`, which orders a window within *its own*
/// application — it cannot pull a background app forward. So restoring from the
/// status item made every window visible and key, behind whatever the user was
/// looking at, which reads exactly like nothing happening. The agent's overlay
/// has the same problem in a sharper form: a global hotkey by definition fires
/// while another application is frontmost.
///
/// `activateIgnoringOtherApps:` is deprecated in macOS 14 in favour of
/// `activate`, but it is the one that works back to the versions Koru still
/// runs on, and Apple has not removed it. Ignoring other apps is the point: the
/// user asked for Koru from a surface that is by definition not in Koru.
///
/// Lives here rather than in `koru-workspace-host` because two binaries need it and
/// only one of them has a workspace — the same argument as every other module
/// in this crate. Must be called on the main thread; anywhere else it quietly
/// does nothing, which is the only safe answer AppKit leaves.
#[cfg(target_os = "macos")]
pub fn activate_app() {
    use objc2_app_kit::NSApplication;
    use objc2_foundation::MainThreadMarker;

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    #[allow(deprecated)]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
}

#[cfg(not(target_os = "macos"))]
pub fn activate_app() {}

/// Make this process an *accessory*: no Dock tile, no ⌘-Tab entry, windows and
/// a status item only — `LSUIElement`, set at runtime because there is no
/// bundle to carry the plist key yet.
///
/// This is the Windows tray-only shape spoken in AppKit: a background agent's
/// surface is its status item, and a Dock tile that only ever hosts an overlay
/// reads as an app somebody left open. An accessory can still show windows and
/// still become active — [`activate_app`] works on one — which is what the
/// hotkey overlay needs.
///
/// Must be called on the main thread; anywhere else it quietly does nothing.
#[cfg(target_os = "macos")]
pub fn become_accessory() {
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    use objc2_foundation::MainThreadMarker;

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    NSApplication::sharedApplication(mtm)
        .setActivationPolicy(NSApplicationActivationPolicy::Accessory);
}

#[cfg(not(target_os = "macos"))]
pub fn become_accessory() {}

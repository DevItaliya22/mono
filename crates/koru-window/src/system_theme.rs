// SPDX-License-Identifier: MPL-2.0

//! OS light/dark appearance, read before any window exists.
//!
//! Live changes arrive as winit `WindowEvent::ThemeChanged`. This module is
//! the cold-start path and the re-read used when that event fires: brushes bake
//! into parley layouts, so the process global must be right before the first
//! layout is built.
//!
//! Debug: `KORU_THEME_DEBUG=1` logs every read/apply to stderr and
//! `koru-theme-debug.log` in the platform's temp directory.

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::OnceLock;

/// Snapshot of what Windows reports right now.
#[derive(Clone, Debug)]
pub struct SystemThemeProbe {
    /// Resolved: apps should use dark chrome/content.
    pub apps_dark: bool,
    /// Raw `AppsUseLightTheme` DWORD, if the key was readable.
    pub apps_use_light: Option<u32>,
    /// Raw `SystemUsesLightTheme` DWORD, if present (taskbar/chrome; often
    /// desynced from apps).
    pub system_use_light: Option<u32>,
    /// How `apps_dark` was decided.
    pub source: &'static str,
}

/// Whether the OS wants apps in dark mode.
pub fn system_is_dark() -> bool {
    probe().apps_dark
}

/// Full probe for apply paths and debug logging.
pub fn probe() -> SystemThemeProbe {
    #[cfg(windows)]
    {
        windows_probe()
    }
    #[cfg(target_os = "macos")]
    {
        macos_probe()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        SystemThemeProbe {
            apps_dark: false,
            apps_use_light: None,
            system_use_light: None,
            source: "unknown-platform-default-light",
        }
    }
}

/// macOS reports dark mode as the **presence** of a string, not a boolean.
///
/// `AppleInterfaceStyle` is `"Dark"` in dark mode and **absent entirely** in
/// light mode — there is no `"Light"` value to read, which is why this asks
/// whether the key exists rather than what it says. A probe that compared
/// against `"Light"` would report dark for every Mac on earth.
///
/// **`NSUserDefaults`, not `NSApp.effectiveAppearance`.** This runs on the
/// cold-start path, before winit has created an `NSApplication` — and brushes
/// bake into parley layouts, so the answer has to be right before the first
/// layout is built. `NSUserDefaults` needs no application object. It is also the
/// exact counterpart of the Windows path, which reads the registry for the same
/// reason.
///
/// Live changes still arrive as winit's `ThemeChanged` and re-enter here.
///
/// Auto-switching ("Appearance: Auto") simply writes and clears the same key at
/// sunrise and sunset, so it needs no special case.
#[cfg(target_os = "macos")]
fn macos_probe() -> SystemThemeProbe {
    use objc2_foundation::{NSString, NSUserDefaults};

    let defaults = NSUserDefaults::standardUserDefaults();
    let style = defaults.stringForKey(&NSString::from_str("AppleInterfaceStyle"));
    let style = style.map(|s| s.to_string());
    let apps_dark = interface_style_means_dark(style.as_deref());
    SystemThemeProbe {
        apps_dark,
        apps_use_light: None,
        system_use_light: None,
        source: if style.is_some() {
            "AppleInterfaceStyle"
        } else {
            "AppleInterfaceStyle-absent-means-light"
        },
    }
}

/// Map macOS `AppleInterfaceStyle` to dark mode.
///
/// **Absent means light.** There is no `"Light"` value — the key simply does not
/// exist in light mode — so this is a presence test, not a comparison. Any value
/// beginning `Dark` counts: it reads `Dark` today, and `DarkAqua` has been
/// reported on some releases.
///
/// Exposed and pure so the rule is testable without a Mac's real settings, the
/// same way [`apps_use_light_theme_means_dark`] is on Windows.
pub fn interface_style_means_dark(value: Option<&str>) -> bool {
    value.is_some_and(|v| v.starts_with("Dark"))
}

/// Map Windows `AppsUseLightTheme` (DWORD) to dark mode.
///
/// `0` → dark, anything else (including the usual `1`) → light. Exposed for
/// tests so CI never needs a live registry.
pub fn apps_use_light_theme_means_dark(value: u32) -> bool {
    value == 0
}

/// Log a line when `KORU_THEME_DEBUG` is set.
pub fn debug_log(line: impl AsRef<str>) {
    if !debug_on() {
        return;
    }
    let line = line.as_ref();
    eprintln!("[koru:theme] {line}");
    // `std::env::temp_dir` rather than `%TEMP%`: the variable is Windows-only,
    // so the log silently went nowhere on macOS and Linux.
    let path = std::env::temp_dir().join("koru-theme-debug.log");
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{line}");
    }
}

fn debug_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| koru_env::switch(koru_env::name::KORU_THEME_DEBUG))
}

#[cfg(windows)]
fn windows_probe() -> SystemThemeProbe {
    let apps = read_personalize_dword("AppsUseLightTheme");
    let system = read_personalize_dword("SystemUsesLightTheme");
    let (apps_dark, source) = match apps {
        Some(v) => (apps_use_light_theme_means_dark(v), "AppsUseLightTheme"),
        None => (false, "AppsUseLightTheme-missing-default-light"),
    };
    SystemThemeProbe {
        apps_dark,
        apps_use_light: apps,
        system_use_light: system,
        source,
    }
}

#[cfg(windows)]
fn read_personalize_dword(value_name: &str) -> Option<u32> {
    use std::ptr;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, KEY_READ, REG_DWORD, RegCloseKey, RegOpenKeyExW, RegQueryValueExW,
    };

    let subkey: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize\0"
        .encode_utf16()
        .collect();
    let mut name: Vec<u16> = value_name.encode_utf16().collect();
    name.push(0);

    unsafe {
        let mut hkey = core::mem::zeroed();
        let open = RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_READ, &mut hkey);
        if open != ERROR_SUCCESS {
            return None;
        }
        let mut data: u32 = 0xFFFF_FFFF;
        let mut data_size = std::mem::size_of::<u32>() as u32;
        let mut ty = 0u32;
        let query = RegQueryValueExW(
            hkey,
            name.as_ptr(),
            ptr::null_mut(),
            &mut ty,
            (&mut data as *mut u32).cast(),
            &mut data_size,
        );
        let _ = RegCloseKey(hkey);
        if query != ERROR_SUCCESS || ty != REG_DWORD {
            return None;
        }
        Some(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The presence test, in both directions. A probe that compared against
    /// `"Light"` would report dark for every Mac on earth, because that value
    /// never appears.
    #[test]
    fn an_absent_interface_style_is_light() {
        assert!(!interface_style_means_dark(None));
        assert!(interface_style_means_dark(Some("Dark")));
        assert!(interface_style_means_dark(Some("DarkAqua")));
        assert!(!interface_style_means_dark(Some("")));
    }

    #[test]
    fn apps_use_light_theme_zero_is_dark() {
        assert!(apps_use_light_theme_means_dark(0));
        assert!(!apps_use_light_theme_means_dark(1));
        assert!(!apps_use_light_theme_means_dark(2));
    }
}

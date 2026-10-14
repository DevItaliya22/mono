// SPDX-License-Identifier: MPL-2.0

use koru_i18n::{Key, Lang, t};
use koru_native_notifications::MacActionTitles;

fn action_titles(lang: Lang) -> MacActionTitles {
    MacActionTitles {
        open: t(lang, Key::ReminderNotificationOpen).into(),
        snooze: t(lang, Key::ReminderSnooze).into(),
        handled: t(lang, Key::ReminderNotificationHandled).into(),
    }
}

#[cfg(target_os = "macos")]
fn system_lang() -> Lang {
    use objc2_foundation::NSLocale;
    let languages = unsafe { NSLocale::preferredLanguages() };
    Lang::for_system(
        unsafe { languages.firstObject() }
            .map(|value| value.to_string())
            .as_deref(),
    )
}

#[cfg(target_os = "macos")]
fn main() {
    use std::io::{Read as _, Write as _};

    let titles = action_titles(system_lang());
    let mut args = std::env::args_os().skip(1);
    match (args.next(), args.next()) {
        (None, None) => {
            if koru_native_notifications::run_notification_owner_app(&titles).is_err() {
                std::process::exit(1);
            }
        }
        (Some(mode), None) if mode == std::ffi::OsStr::new("--request") => {
            let mut input = Vec::new();
            let read = std::io::stdin()
                .take((koru_native_notifications::MAX_MAC_OWNER_MESSAGE + 1) as u64)
                .read_to_end(&mut input);
            let response = if read.is_err()
                || input.len() > koru_native_notifications::MAX_MAC_OWNER_MESSAGE
            {
                Err(koru_native_notifications::Failure {
                    code: koru_native_notifications::FailureCode::InvalidRequest,
                    detail: "notification-owner request exceeds its limit".into(),
                })
            } else {
                serde_json::from_slice(&input)
                    .map_err(|_| koru_native_notifications::Failure {
                        code: koru_native_notifications::FailureCode::InvalidRequest,
                        detail: "notification-owner request is invalid".into(),
                    })
                    .and_then(|request| {
                        koru_native_notifications::run_notification_owner(request, &titles)
                    })
            };
            let output = serde_json::to_vec(&response)
                .expect("the bounded notification-owner response is serializable");
            if output.len() > koru_native_notifications::MAX_MAC_OWNER_MESSAGE
                || std::io::stdout().write_all(&output).is_err()
            {
                std::process::exit(1);
            }
            // A typed platform/permission error is a successful protocol
            // exchange. The Agent must be allowed to decode its exact category.
        }
        _ => std::process::exit(2),
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    let _ = action_titles(Lang::En);
    eprintln!("koru-notification-owner is available only on macOS");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_has_three_native_action_titles() {
        for lang in Lang::ALL {
            let titles = action_titles(lang);
            assert!(!titles.open.is_empty());
            assert!(!titles.snooze.is_empty());
            assert!(!titles.handled.is_empty());
        }
    }
}

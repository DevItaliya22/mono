// SPDX-License-Identifier: MPL-2.0

use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::DateTime;
use windows::UI::Notifications::{
    NotificationSetting, ScheduledToastNotification, ToastNotification, ToastNotificationManager,
};
use windows::core::HSTRING;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{
    Authorization, Capability, Content, DeliverySupport, Failure, FailureCode, NativeHandle,
    NativeNotifications, NotificationNamespace, RegistrationId, Schedule, Sound,
};

const WINDOWS_EPOCH_TICKS: i64 = 116_444_736_000_000_000;

pub(crate) struct WindowsNotifications {
    namespace: NotificationNamespace,
    activation_ready: Arc<AtomicBool>,
}

impl WindowsNotifications {
    pub(crate) fn new(namespace: NotificationNamespace, activation_ready: Arc<AtomicBool>) -> Self {
        Self {
            namespace,
            activation_ready,
        }
    }

    fn group(&self) -> HSTRING {
        HSTRING::from(self.namespace.as_str())
    }

    fn tag(registration: &RegistrationId) -> HSTRING {
        HSTRING::from(crate::stable_native_token(registration.as_str()))
    }

    fn application_id() -> Result<HSTRING, Failure> {
        let family = windows::ApplicationModel::Package::Current()
            .and_then(|package| package.Id())
            .and_then(|id| id.FamilyName())
            .map_err(platform)?;
        Ok(HSTRING::from(format!(
            "{}!{}",
            family,
            crate::WINDOWS_NOTIFICATION_APPLICATION_ID
        )))
    }

    fn notifier(
        application_id: &HSTRING,
    ) -> Result<windows::UI::Notifications::ToastNotifier, Failure> {
        // All calls live on the Agent worker. Re-initializing the same MTA is
        // harmless and keeps this adapter independent of process startup.
        unsafe {
            windows::Win32::System::WinRT::RoInitialize(
                windows::Win32::System::WinRT::RO_INIT_MULTITHREADED,
            )
            .map_err(platform)?;
        }
        ToastNotificationManager::CreateToastNotifierWithId(application_id).map_err(platform)
    }

    fn identity_ready(&self) -> bool {
        self.activation_ready.load(Ordering::Acquire)
            && windows::ApplicationModel::Package::Current().is_ok()
    }
}

impl NativeNotifications for WindowsNotifications {
    fn delivery_support(&self) -> DeliverySupport {
        let ready = self.identity_ready();
        delivery_support_for(ready, crate::sounds::packaged_windows_sounds_available())
    }

    fn capability(&mut self) -> Result<Capability, Failure> {
        if !self.identity_ready() {
            return Err(Failure {
                code: FailureCode::Unsupported,
                detail: "Windows reminder delivery requires the registered Koru identity package and activation host".into(),
            });
        }
        let application_id = Self::application_id()?;
        let authorization = if Self::notifier(&application_id)?
            .Setting()
            .map_err(platform)?
            == NotificationSetting::Enabled
        {
            Authorization::Allowed
        } else {
            Authorization::Denied
        };
        let support = self.delivery_support();
        Ok(Capability {
            authorization,
            actions: support.actions,
            stable_replacement: support.stable_replacement,
            withdrawal: support.withdrawal,
            custom_sound: support.custom_sound,
            activation_while_stopped: support.activation_while_stopped,
        })
    }

    fn schedule(&mut self, request: Schedule) -> Result<NativeHandle, Failure> {
        if !self.identity_ready() {
            return Err(unsupported_identity());
        }
        if matches!(request.sound, Sound::Bundled(_))
            && !crate::sounds::packaged_windows_sounds_available()
        {
            return Err(Failure {
                code: FailureCode::AssetMissing,
                detail: "the packaged Windows reminder tones are incomplete".into(),
            });
        }
        let document = xml(
            &request.content,
            Some((
                &self.namespace,
                &request.registration,
                &request.action_labels,
                request.actions,
            )),
            request.sound,
        )?;
        let tag = Self::tag(&request.registration);
        let group = self.group();
        let application_id = Self::application_id()?;
        let notifier = Self::notifier(&application_id)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or_default();
        match delivery_mode(request.deliver_at_unix_ms, now) {
            DeliveryMode::Immediate => {
                withdraw_pending(&notifier, &tag, &group)?;
                withdraw_delivered(&tag, &group, &application_id)?;
                let toast =
                    ToastNotification::CreateToastNotification(&document).map_err(platform)?;
                toast.SetTag(&tag).map_err(platform)?;
                toast.SetGroup(&group).map_err(platform)?;
                notifier.Show(&toast).map_err(platform)?;
            }
            DeliveryMode::Scheduled => {
                let delivery = unix_ms_to_datetime(request.deliver_at_unix_ms)?;
                let toast = ScheduledToastNotification::CreateScheduledToastNotification(
                    &document, delivery,
                )
                .map_err(platform)?;
                toast.SetTag(&tag).map_err(platform)?;
                toast.SetGroup(&group).map_err(platform)?;
                replace_registration(&mut RealReplacement {
                    notifier: &notifier,
                    toast: &toast,
                    tag: &tag,
                    group: &group,
                    application_id: &application_id,
                })?;
            }
        }
        Ok(NativeHandle(
            self.namespace.native_id(&request.registration),
        ))
    }

    fn withdraw(&mut self, registration: &RegistrationId) -> Result<(), Failure> {
        let application_id = Self::application_id()?;
        let notifier = Self::notifier(&application_id)?;
        let tag = Self::tag(registration);
        let group = self.group();
        withdraw_pending(&notifier, &tag, &group)?;
        withdraw_delivered(&tag, &group, &application_id)
    }

    fn reset_namespace(&mut self) -> Result<(), Failure> {
        let application_id = Self::application_id()?;
        let notifier = Self::notifier(&application_id)?;
        let group = self.group();
        let pending = notifier
            .GetScheduledToastNotifications()
            .map_err(platform)?;
        for index in 0..pending.Size().map_err(platform)? {
            let candidate = pending.GetAt(index).map_err(platform)?;
            if candidate.Group().map_err(platform)? == group {
                notifier.RemoveFromSchedule(&candidate).map_err(platform)?;
            }
        }
        ToastNotificationManager::History()
            .and_then(|history| history.RemoveGroupWithId(&group, &application_id))
            .map_err(platform)
    }

    fn open_settings(&mut self) -> Result<(), Failure> {
        std::process::Command::new("explorer.exe")
            .arg("ms-settings:notifications")
            .spawn()
            .map(|_| ())
            .map_err(|error| Failure {
                code: FailureCode::Platform,
                detail: format!("could not open Windows notification settings: {error}"),
            })
    }

    fn send_test(&mut self, content: Content, sound: Sound) -> Result<(), Failure> {
        if !self.identity_ready() {
            return Err(unsupported_identity());
        }
        if matches!(sound, Sound::Bundled(_)) && !crate::sounds::packaged_windows_sounds_available()
        {
            return Err(Failure {
                code: FailureCode::AssetMissing,
                detail: "the packaged Windows reminder tones are incomplete".into(),
            });
        }
        let document = xml(&content, None, sound)?;
        let toast = ToastNotification::CreateToastNotification(&document).map_err(platform)?;
        let registration = RegistrationId::new("koru-notification-test").unwrap();
        let tag = Self::tag(&registration);
        let group = self.group();
        toast.SetTag(&tag).map_err(platform)?;
        toast.SetGroup(&group).map_err(platform)?;
        let application_id = Self::application_id()?;
        let notifier = Self::notifier(&application_id)?;
        withdraw_pending(&notifier, &tag, &group)?;
        withdraw_delivered(&tag, &group, &application_id)?;
        notifier.Show(&toast).map_err(platform)
    }
}

fn delivery_support_for(identity_and_broker_ready: bool, packaged_sounds: bool) -> DeliverySupport {
    DeliverySupport {
        // A sparse package alone is not enough: action delivery is exposed
        // only after this process's COM class factory is registered.
        actions: identity_and_broker_ready,
        stable_replacement: true,
        withdrawal: true,
        custom_sound: identity_and_broker_ready && packaged_sounds,
        activation_while_stopped: identity_and_broker_ready,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeliveryMode {
    Immediate,
    Scheduled,
}

fn delivery_mode(deliver_at_ms: i64, now_ms: i64) -> DeliveryMode {
    if deliver_at_ms <= now_ms.saturating_add(1_000) {
        DeliveryMode::Immediate
    } else {
        DeliveryMode::Scheduled
    }
}

trait ReplacementOps {
    fn clear_pending(&mut self) -> Result<(), Failure>;
    fn clear_delivered(&mut self) -> Result<(), Failure>;
    fn add(&mut self) -> Result<(), Failure>;
}

fn replace_registration(ops: &mut impl ReplacementOps) -> Result<(), Failure> {
    ops.clear_pending()?;
    ops.clear_delivered()?;
    ops.add()
}

struct RealReplacement<'a> {
    notifier: &'a windows::UI::Notifications::ToastNotifier,
    toast: &'a ScheduledToastNotification,
    tag: &'a HSTRING,
    group: &'a HSTRING,
    application_id: &'a HSTRING,
}

impl ReplacementOps for RealReplacement<'_> {
    fn clear_pending(&mut self) -> Result<(), Failure> {
        withdraw_pending(self.notifier, self.tag, self.group)
    }

    fn clear_delivered(&mut self) -> Result<(), Failure> {
        withdraw_delivered(self.tag, self.group, self.application_id)
    }

    fn add(&mut self) -> Result<(), Failure> {
        self.notifier.AddToSchedule(self.toast).map_err(platform)
    }
}

fn withdraw_pending(
    notifier: &windows::UI::Notifications::ToastNotifier,
    tag: &HSTRING,
    group: &HSTRING,
) -> Result<(), Failure> {
    let pending = notifier
        .GetScheduledToastNotifications()
        .map_err(platform)?;
    for index in 0..pending.Size().map_err(platform)? {
        let candidate = pending.GetAt(index).map_err(platform)?;
        if candidate.Tag().map_err(platform)? == *tag
            && candidate.Group().map_err(platform)? == *group
        {
            notifier.RemoveFromSchedule(&candidate).map_err(platform)?;
        }
    }
    Ok(())
}

fn withdraw_delivered(
    tag: &HSTRING,
    group: &HSTRING,
    application_id: &HSTRING,
) -> Result<(), Failure> {
    ToastNotificationManager::History()
        .and_then(|history| history.RemoveGroupedTagWithId(tag, group, application_id))
        .map_err(platform)
}

fn xml(
    content: &Content,
    activation: Option<(
        &NotificationNamespace,
        &RegistrationId,
        &crate::ActionLabels,
        crate::ActionSet,
    )>,
    sound: Sound,
) -> Result<XmlDocument, Failure> {
    let title = escape(&content.title);
    let body = content
        .body
        .as_deref()
        .map(escape)
        .map(|body| format!("<text>{body}</text>"))
        .unwrap_or_default();
    let launch = activation
        .map(|(namespace, registration, _, _)| {
            escape(&windows_envelope(
                namespace,
                registration,
                crate::Action::Open,
            ))
        })
        .unwrap_or_default();
    let actions = activation.map_or_else(String::new, |(namespace, registration, labels, set)| {
        let action = |content: &str, action| {
            let arguments = escape(&windows_envelope(namespace, registration, action));
            format!(
                r#"<action content="{}" arguments="{arguments}" activationType="foreground"/>"#,
                escape(content),
            )
        };
        let buttons = match set {
            crate::ActionSet::Reminder => format!(
                "{}{}{}",
                action(&labels.open, crate::Action::Open),
                action(&labels.snooze, crate::Action::Snooze),
                action(&labels.handled, crate::Action::Handled),
            ),
            crate::ActionSet::OpenOnly => action(&labels.open, crate::Action::Open),
        };
        format!("<actions>{buttons}</actions>")
    });
    let audio = match sound {
        Sound::Silent => r#"<audio silent="true"/>"#.to_string(),
        Sound::System => r#"<audio src="ms-winsoundevent:Notification.Default"/>"#.to_string(),
        Sound::Bundled(tone) => {
            let asset = crate::sounds::sound_asset(tone).ok_or_else(|| Failure {
                code: FailureCode::InvalidRequest,
                detail: "unknown bundled reminder tone".into(),
            })?;
            format!(
                r#"<audio src="ms-appx:///assets/reminders/{}"/>"#,
                asset.file
            )
        }
    };
    let source = format!(
        r#"<toast launch="{launch}"><visual><binding template="ToastGeneric"><text>{title}</text>{body}</binding></visual>{actions}{audio}</toast>"#
    );
    let document = XmlDocument::new().map_err(platform)?;
    document.LoadXml(&HSTRING::from(source)).map_err(platform)?;
    Ok(document)
}

fn windows_envelope(
    namespace: &NotificationNamespace,
    registration: &RegistrationId,
    action: crate::Action,
) -> String {
    crate::WindowsActivationEnvelope::new(
        namespace.clone(),
        crate::Activation::new(registration.clone(), action),
    )
    .encode_opaque()
}

fn unsupported_identity() -> Failure {
    Failure {
        code: FailureCode::Unsupported,
        detail: "Windows reminder delivery requires the registered Koru identity package and activation host".into(),
    }
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn unix_ms_to_datetime(unix_ms: i64) -> Result<DateTime, Failure> {
    unix_ms
        .checked_mul(10_000)
        .and_then(|ticks| ticks.checked_add(WINDOWS_EPOCH_TICKS))
        .map(|universal_time| DateTime {
            UniversalTime: universal_time,
        })
        .ok_or_else(|| Failure {
            code: FailureCode::InvalidRequest,
            detail: "notification delivery instant is outside the Windows range".into(),
        })
}

fn platform(error: windows::core::Error) -> Failure {
    Failure {
        code: FailureCode::Platform,
        detail: format!("Windows notification API failed: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_escapes_content_and_carries_only_the_opaque_activation() {
        let content = Content {
            title: "<title & private>".into(),
            body: Some("body \"quoted\"".into()),
        };
        let registration = RegistrationId::new("registration-1").unwrap();
        let namespace = NotificationNamespace::from_stable_key("root");
        let labels = crate::ActionLabels {
            open: "Open & inspect".into(),
            snooze: "Snooze".into(),
            handled: "Handled".into(),
        };
        let document = xml(
            &content,
            Some((
                &namespace,
                &registration,
                &labels,
                crate::ActionSet::Reminder,
            )),
            Sound::System,
        )
        .unwrap();
        let rendered = document.GetXml().unwrap().to_string();
        assert!(rendered.contains("&lt;title &amp; private&gt;"));
        assert!(rendered.contains("content=\"Open &amp; inspect\""));
        assert_eq!(rendered.matches("activationType=\"foreground\"").count(), 3);
        for opaque in [
            "eyJzY2hlbWFfdmVyc2lvbiI6MSwibmFtZXNwYWNlIjoiYTM1NGZkMWZmMGM0NjdjNSIsInJlZ2lzdHJhdGlvbiI6InJlZ2lzdHJhdGlvbi0xIiwiYWN0aW9uIjoib3BlbiJ9",
            "eyJzY2hlbWFfdmVyc2lvbiI6MSwibmFtZXNwYWNlIjoiYTM1NGZkMWZmMGM0NjdjNSIsInJlZ2lzdHJhdGlvbiI6InJlZ2lzdHJhdGlvbi0xIiwiYWN0aW9uIjoic25vb3plIn0",
            "eyJzY2hlbWFfdmVyc2lvbiI6MSwibmFtZXNwYWNlIjoiYTM1NGZkMWZmMGM0NjdjNSIsInJlZ2lzdHJhdGlvbiI6InJlZ2lzdHJhdGlvbi0xIiwiYWN0aW9uIjoiaGFuZGxlZCJ9",
        ] {
            assert!(rendered.contains(&format!("arguments=\"{opaque}\"")));
        }
        assert!(!rendered.contains("activationType=\"background\""));
        assert!(!rendered.contains("profile"));
        assert!(!rendered.contains("workspace"));
    }

    #[test]
    fn silent_xml_explicitly_suppresses_audio() {
        let content = Content {
            title: "2".into(),
            body: None,
        };
        let document = xml(&content, None, Sound::Silent).unwrap();
        assert!(
            document
                .GetXml()
                .unwrap()
                .to_string()
                .contains("silent=\"true\"")
        );
    }

    #[test]
    fn open_only_xml_has_one_foreground_open_action_and_no_audio() {
        let content = Content {
            title: "2".into(),
            body: None,
        };
        let namespace = NotificationNamespace::from_stable_key("root");
        let registration = RegistrationId::new("quiet-summary").unwrap();
        let labels = crate::ActionLabels {
            open: "Open".into(),
            snooze: "Snooze".into(),
            handled: "Handled".into(),
        };
        let document = xml(
            &content,
            Some((
                &namespace,
                &registration,
                &labels,
                crate::ActionSet::OpenOnly,
            )),
            Sound::Silent,
        )
        .unwrap();
        let rendered = document.GetXml().unwrap().to_string();
        assert_eq!(rendered.matches("activationType=\"foreground\"").count(), 1);
        assert!(rendered.contains("content=\"Open\""));
        assert!(!rendered.contains("content=\"Snooze\""));
        assert!(!rendered.contains("content=\"Handled\""));
        assert!(rendered.contains("silent=\"true\""));
        assert!(!rendered.contains("<audio src="));
    }

    #[test]
    fn windows_datetime_conversion_is_checked() {
        assert_eq!(
            unix_ms_to_datetime(0).unwrap().UniversalTime,
            WINDOWS_EPOCH_TICKS
        );
        assert!(unix_ms_to_datetime(i64::MAX).is_err());
    }

    #[test]
    fn unresolved_package_capabilities_are_not_advertised() {
        let adapter = WindowsNotifications::new(
            NotificationNamespace::from_stable_key("root"),
            Arc::new(AtomicBool::new(true)),
        );
        let support = adapter.delivery_support();
        assert!(!support.actions);
        assert!(!support.custom_sound);
        assert!(!support.activation_while_stopped);
    }

    #[test]
    fn registered_identity_and_broker_unlock_the_scheduler_requirements() {
        let support = delivery_support_for(true, true);
        assert!(support.actions);
        assert!(support.stable_replacement);
        assert!(support.withdrawal);
        assert!(support.custom_sound);
        assert!(support.activation_while_stopped);
    }

    #[test]
    fn bundled_tones_are_package_relative() {
        let content = Content {
            title: "Reminder".into(),
            body: None,
        };
        let document = xml(
            &content,
            None,
            Sound::Bundled(crate::BundledTone::KoruDefault),
        )
        .unwrap();
        let rendered = document.GetXml().unwrap().to_string();
        assert!(rendered.contains("src=\"ms-appx:///assets/reminders/01_Koru_Default.wav\""));
        assert!(!rendered.contains(env!("CARGO_MANIFEST_DIR")));
    }

    #[test]
    fn replacement_clears_pending_and_delivered_before_add() {
        #[derive(Default)]
        struct Fake(Vec<&'static str>);
        impl ReplacementOps for Fake {
            fn clear_pending(&mut self) -> Result<(), Failure> {
                self.0.push("pending");
                Ok(())
            }
            fn clear_delivered(&mut self) -> Result<(), Failure> {
                self.0.push("delivered");
                Ok(())
            }
            fn add(&mut self) -> Result<(), Failure> {
                self.0.push("add");
                Ok(())
            }
        }
        let mut fake = Fake::default();
        replace_registration(&mut fake).unwrap();
        assert_eq!(fake.0, ["pending", "delivered", "add"]);
    }

    #[test]
    fn due_now_or_nearly_due_uses_immediate_delivery() {
        assert_eq!(delivery_mode(10_000, 10_000), DeliveryMode::Immediate);
        assert_eq!(delivery_mode(10_999, 10_000), DeliveryMode::Immediate);
        assert_eq!(delivery_mode(11_001, 10_000), DeliveryMode::Scheduled);
    }

    #[test]
    fn windows_groups_are_isolated_by_selected_root_namespace() {
        let first = WindowsNotifications::new(
            NotificationNamespace::from_stable_key("root-a"),
            Arc::new(AtomicBool::new(false)),
        );
        let second = WindowsNotifications::new(
            NotificationNamespace::from_stable_key("root-b"),
            Arc::new(AtomicBool::new(false)),
        );
        assert_ne!(first.group(), second.group());
        assert_eq!(first.group().to_string().len(), 16);
    }

    #[test]
    fn windows_tags_and_groups_honor_the_strict_native_limit() {
        let adapter = WindowsNotifications::new(
            NotificationNamespace::from_stable_key("root"),
            Arc::new(AtomicBool::new(false)),
        );
        let longest = RegistrationId::new("a".repeat(64)).unwrap();
        let other = RegistrationId::new("b".repeat(64)).unwrap();

        assert_eq!(WindowsNotifications::tag(&longest).to_string().len(), 16);
        assert_eq!(adapter.group().to_string().len(), 16);
        assert_eq!(
            WindowsNotifications::tag(&longest),
            WindowsNotifications::tag(&longest)
        );
        assert_ne!(
            WindowsNotifications::tag(&longest),
            WindowsNotifications::tag(&other)
        );
    }
}

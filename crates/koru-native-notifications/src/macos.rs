#![allow(unsafe_op_in_unsafe_fn)]
// SPDX-License-Identifier: MPL-2.0

use std::io::{Read, Write};
use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use std::{
    fs::OpenOptions,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
};

use block2::RcBlock;
use objc2::mutability::InteriorMutable;
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObject, ProtocolObject};
use objc2::{ClassType, DeclaredClass, declare_class, msg_send_id};
use objc2_app_kit::NSApplication;
use objc2_foundation::{MainThreadMarker, NSArray, NSError, NSObjectProtocol, NSSet, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification,
    UNNotificationAction, UNNotificationActionOptions, UNNotificationCategory,
    UNNotificationCategoryOptions, UNNotificationRequest, UNNotificationResponse,
    UNNotificationSettings, UNNotificationSound, UNTimeIntervalNotificationTrigger,
    UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};

use crate::mac_owner::{mac_category, mac_delivery_support, mac_route_temp_name};
use crate::{
    ActionSet, Authorization, BUNDLED_TONES, Capability, Content, DeliverySupport, Failure,
    FailureCode, MAC_ACTION_HANDLED, MAC_ACTION_OPEN, MAC_ACTION_SNOOZE, MAC_CATEGORY_OPEN_ONLY,
    MAC_CATEGORY_REMINDER, MAX_MAC_OWNER_MESSAGE, MAX_MAC_ROOT_LEN, MacActionTitles,
    MacOwnerRequest, MacOwnerResponse, MacOwnerValue, MacRootRoute, NativeHandle,
    NativeNotifications, NotificationNamespace, RegistrationId, Schedule, Sound,
    mac_agent_executable_from_owner, mac_agent_launch_args, mac_owner_executable_from,
    mac_owner_info_plist_from,
};

pub(crate) struct MacNotifications {
    namespace: NotificationNamespace,
    explicit_root: Option<PathBuf>,
}

impl MacNotifications {
    pub(crate) fn new(namespace: NotificationNamespace, explicit_root: Option<PathBuf>) -> Self {
        Self {
            namespace,
            explicit_root,
        }
    }

    fn native_id(&self, registration: &RegistrationId) -> String {
        self.namespace.native_id(registration)
    }

    fn owner(&self) -> Result<PathBuf, Failure> {
        let current = std::env::current_exe().map_err(|_| unsupported_owner())?;
        let owner = mac_owner_executable_from(&current).ok_or_else(unsupported_owner)?;
        let complete = owner.is_file()
            && mac_owner_info_plist_from(&owner).is_some_and(|path| path.is_file())
            && mac_agent_executable_from_owner(&owner).is_some_and(|path| path.is_file());
        complete.then_some(owner).ok_or_else(unsupported_owner)
    }

    fn invoke(&self, request: &MacOwnerRequest, timeout: Duration) -> MacOwnerResponse {
        let input = serde_json::to_vec(request).map_err(|_| invalid("invalid owner request"))?;
        if input.len() > MAX_MAC_OWNER_MESSAGE {
            return Err(invalid("notification-owner request exceeds its limit"));
        }
        let mut child = std::process::Command::new(self.owner()?)
            .arg("--request")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|error| platform(format!("could not start notification owner: {error}")))?;
        child
            .stdin
            .take()
            .ok_or_else(|| platform("notification owner has no input"))?
            .write_all(&input)
            .map_err(|_| platform("could not send notification-owner request"))?;
        let deadline = Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => break,
                Ok(Some(_)) => return Err(platform("notification owner rejected its request")),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(platform("notification owner timed out"));
                }
                Err(_) => return Err(platform("could not wait for notification owner")),
            }
        }
        let mut output = Vec::new();
        child
            .stdout
            .take()
            .ok_or_else(|| platform("notification owner has no output"))?
            .take((MAX_MAC_OWNER_MESSAGE + 1) as u64)
            .read_to_end(&mut output)
            .map_err(|_| platform("could not read notification-owner response"))?;
        if output.len() > MAX_MAC_OWNER_MESSAGE {
            return Err(platform("notification-owner response exceeds its limit"));
        }
        serde_json::from_slice(&output)
            .map_err(|_| platform("notification owner returned an invalid response"))?
    }

    fn support(&self) -> DeliverySupport {
        let owner = self.owner().ok();
        // Package geometry is the runtime gate. The separate signed matrix
        // remains manual evidence and is not claimed by this inspection.
        let sounds = owner
            .as_deref()
            .is_some_and(packaged_owner_sounds_available);
        mac_delivery_support(owner.is_some(), sounds)
    }
}

impl NativeNotifications for MacNotifications {
    fn delivery_support(&self) -> DeliverySupport {
        self.support()
    }

    fn capability(&mut self) -> Result<Capability, Failure> {
        let support = self.support();
        let MacOwnerValue::Authorization(authorization) =
            self.invoke(&MacOwnerRequest::Inspect, Duration::from_secs(5))?
        else {
            return Err(platform("notification owner returned the wrong response"));
        };
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
        let native_id = self.native_id(&request.registration);
        let root_route = MacRootRoute::from_explicit_root(self.explicit_root.as_deref())
            .map_err(|_| invalid("selected root cannot be routed by notification owner"))?;
        self.invoke(
            &MacOwnerRequest::Schedule {
                native_id: native_id.clone(),
                deliver_at_unix_ms: request.deliver_at_unix_ms,
                content: request.content,
                sound: request.sound,
                actions: request.actions,
                root_route,
            },
            Duration::from_secs(10),
        )?;
        Ok(NativeHandle(native_id))
    }

    fn withdraw(&mut self, registration: &RegistrationId) -> Result<(), Failure> {
        self.invoke(
            &MacOwnerRequest::Withdraw {
                native_id: self.native_id(registration),
            },
            Duration::from_secs(5),
        )?;
        Ok(())
    }

    fn reset_namespace(&mut self) -> Result<(), Failure> {
        self.invoke(
            &MacOwnerRequest::ResetNamespace {
                prefix: format!("koru.{}.", self.namespace.as_str()),
            },
            Duration::from_secs(10),
        )?;
        Ok(())
    }

    fn open_settings(&mut self) -> Result<(), Failure> {
        self.invoke(&MacOwnerRequest::OpenSettings, Duration::from_secs(5))?;
        Ok(())
    }

    fn request_authorization(&mut self) -> Result<Authorization, Failure> {
        match self.invoke(
            &MacOwnerRequest::RequestAuthorization,
            Duration::from_secs(15),
        )? {
            MacOwnerValue::Authorization(value) => Ok(value),
            MacOwnerValue::Done => Err(platform("notification owner returned the wrong response")),
        }
    }

    fn send_test(&mut self, content: Content, sound: Sound) -> Result<(), Failure> {
        let registration = RegistrationId::new("koru-notification-test").unwrap();
        self.invoke(
            &MacOwnerRequest::SendTest {
                native_id: self.native_id(&registration),
                content,
                sound,
            },
            Duration::from_secs(10),
        )?;
        Ok(())
    }
}

/// Execute one bounded request inside the signed notification-owner bundle.
/// This path cannot receive activations and never requests permission unless
/// its explicit request is `RequestAuthorization`.
pub fn run_notification_owner(
    request: MacOwnerRequest,
    titles: &MacActionTitles,
) -> MacOwnerResponse {
    unsafe { register_categories(titles) };
    match request {
        MacOwnerRequest::Inspect => authorization().map(MacOwnerValue::Authorization),
        MacOwnerRequest::Schedule {
            native_id,
            deliver_at_unix_ms,
            content,
            sound,
            actions,
            root_route,
        } => {
            validate_native_id(&native_id)?;
            require_authorized()?;
            require_sound(sound)?;
            let namespace = crate::namespace_from_native_id(&native_id).unwrap();
            write_route(namespace, &root_route)?;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| invalid("system time precedes Unix epoch"))?
                .as_millis() as i64;
            let delay = ((deliver_at_unix_ms - now).max(1) as f64) / 1000.0;
            unsafe {
                let trigger = UNTimeIntervalNotificationTrigger::triggerWithTimeInterval_repeats(
                    delay, false,
                );
                let native =
                    build_request(&native_id, &content, sound, Some(&trigger), Some(actions));
                withdraw_id(&native_id);
                add(&native)?;
            }
            Ok(MacOwnerValue::Done)
        }
        MacOwnerRequest::Withdraw { native_id } => {
            validate_native_id(&native_id)?;
            unsafe { withdraw_id(&native_id) };
            Ok(MacOwnerValue::Done)
        }
        MacOwnerRequest::ResetNamespace { prefix } => {
            validate_prefix(&prefix)?;
            reset_namespace(&prefix)?;
            Ok(MacOwnerValue::Done)
        }
        MacOwnerRequest::OpenSettings => {
            std::process::Command::new("/usr/bin/open")
                .arg("x-apple.systempreferences:com.apple.preference.notifications")
                .spawn()
                .map_err(|error| {
                    platform(format!(
                        "could not open macOS notification settings: {error}"
                    ))
                })?;
            Ok(MacOwnerValue::Done)
        }
        MacOwnerRequest::RequestAuthorization => {
            request_authorization().map(MacOwnerValue::Authorization)
        }
        MacOwnerRequest::SendTest {
            native_id,
            content,
            sound,
        } => {
            validate_native_id(&native_id)?;
            require_authorized()?;
            require_sound(sound)?;
            unsafe { add(&build_request(&native_id, &content, sound, None, None))? };
            Ok(MacOwnerValue::Done)
        }
    }
}

fn validate_native_id(value: &str) -> Result<(), Failure> {
    crate::namespace_from_native_id(value)
        .map(|_| ())
        .ok_or_else(|| invalid("native notification id is outside Koru's bounded namespace"))
}

fn validate_prefix(value: &str) -> Result<(), Failure> {
    let id = value
        .strip_suffix('.')
        .ok_or_else(|| invalid("invalid namespace prefix"))?;
    crate::namespace_from_native_id(&format!("{id}.r"))
        .map(|_| ())
        .ok_or_else(|| invalid("invalid namespace prefix"))
}

fn route_directory() -> Result<PathBuf, Failure> {
    let home =
        std::env::var_os("HOME").ok_or_else(|| platform("macOS home directory is unavailable"))?;
    Ok(PathBuf::from(home).join("Library/Application Support/Koru/notification-owner/routes"))
}

fn write_route(namespace: &str, route: &MacRootRoute) -> Result<(), Failure> {
    static TEMP_NONCE: AtomicU64 = AtomicU64::new(0);

    route
        .validate()
        .map_err(|_| invalid("invalid notification route"))?;
    let directory = route_directory()?;
    std::fs::create_dir_all(&directory)
        .map_err(|_| platform("could not create notification route directory"))?;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
        .map_err(|_| platform("could not make notification routes private"))?;
    let value =
        serde_json::to_vec(route).map_err(|_| invalid("could not encode notification route"))?;
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    let mut opened = None;
    for _ in 0..8 {
        let nonce = TEMP_NONCE.fetch_add(1, Ordering::Relaxed);
        let path = directory.join(mac_route_temp_name(
            namespace,
            std::process::id(),
            seed,
            nonce,
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => {
                opened = Some((path, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(platform("could not create notification route")),
        }
    }
    let (temporary, mut file) =
        opened.ok_or_else(|| platform("could not allocate notification route"))?;
    if file.write_all(&value).is_err() {
        drop(file);
        let _ = std::fs::remove_file(&temporary);
        return Err(platform("could not write notification route"));
    }
    drop(file);
    if std::fs::rename(&temporary, directory.join(namespace)).is_err() {
        let _ = std::fs::remove_file(&temporary);
        return Err(platform("could not publish notification route"));
    }
    Ok(())
}

fn read_route(namespace: &str) -> Result<MacRootRoute, Failure> {
    let bytes = std::fs::read(
        route_directory()
            .map_err(|_| invalid("notification route is unavailable"))?
            .join(namespace),
    )
    .map_err(|_| invalid("notification route is missing"))?;
    if bytes.len() > MAX_MAC_ROOT_LEN + 64 {
        return Err(invalid("notification route exceeds its limit"));
    }
    crate::decode_mac_root_route(Some(&bytes)).map_err(|_| invalid("notification route is corrupt"))
}

fn packaged_owner_sounds_available(owner: &std::path::Path) -> bool {
    crate::sounds::packaged_macos_sounds_available_from(Some(owner))
}

fn owner_sounds_available() -> bool {
    packaged_owner_sounds_available(&std::env::current_exe().unwrap_or_else(|_| PathBuf::new()))
}
fn require_sound(sound: Sound) -> Result<(), Failure> {
    if matches!(sound, Sound::Bundled(_)) && !owner_sounds_available() {
        return Err(Failure {
            code: FailureCode::AssetMissing,
            detail: "notification-owner bundle is missing reminder sounds".into(),
        });
    }
    Ok(())
}

fn authorization() -> Result<Authorization, Failure> {
    let center = unsafe { UNUserNotificationCenter::currentNotificationCenter() };
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let completion = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
        let _ = send.send(unsafe { settings.as_ref().authorizationStatus() });
    });
    unsafe { center.getNotificationSettingsWithCompletionHandler(&completion) };
    let status = receive
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| platform("macOS notification settings inspection timed out"))?;
    Ok(match status {
        UNAuthorizationStatus::NotDetermined => Authorization::NotDetermined,
        UNAuthorizationStatus::Denied => Authorization::Denied,
        UNAuthorizationStatus::Authorized
        | UNAuthorizationStatus::Provisional
        | UNAuthorizationStatus::Ephemeral => Authorization::Allowed,
        _ => Authorization::Unavailable,
    })
}

fn require_authorized() -> Result<(), Failure> {
    match authorization()? {
        Authorization::Allowed => Ok(()),
        Authorization::NotDetermined => Err(Failure {
            code: FailureCode::PermissionDenied,
            detail: "macOS notification permission has not been requested explicitly".into(),
        }),
        _ => Err(Failure {
            code: FailureCode::PermissionDenied,
            detail: "macOS notification permission is not allowed".into(),
        }),
    }
}

fn request_authorization() -> Result<Authorization, Failure> {
    let center = unsafe { UNUserNotificationCenter::currentNotificationCenter() };
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let completion = RcBlock::new(move |granted: Bool, error: *mut NSError| {
        let _ = send.send((bool::from(granted), error.is_null()));
    });
    unsafe {
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::UNAuthorizationOptionAlert
                | UNAuthorizationOptions::UNAuthorizationOptionSound,
            &completion,
        )
    };
    let (granted, no_error) = receive
        .recv_timeout(Duration::from_secs(10))
        .map_err(|_| platform("macOS authorization request timed out"))?;
    if !no_error {
        return Err(platform("macOS rejected the authorization request"));
    }
    Ok(if granted {
        Authorization::Allowed
    } else {
        Authorization::Denied
    })
}

unsafe fn register_categories(titles: &MacActionTitles) {
    let options = UNNotificationActionOptions::empty();
    let open = UNNotificationAction::actionWithIdentifier_title_options(
        &NSString::from_str(MAC_ACTION_OPEN),
        &NSString::from_str(&titles.open),
        options,
    );
    let reminder_actions = NSArray::from_vec(vec![
        open.clone(),
        UNNotificationAction::actionWithIdentifier_title_options(
            &NSString::from_str(MAC_ACTION_SNOOZE),
            &NSString::from_str(&titles.snooze),
            options,
        ),
        UNNotificationAction::actionWithIdentifier_title_options(
            &NSString::from_str(MAC_ACTION_HANDLED),
            &NSString::from_str(&titles.handled),
            options,
        ),
    ]);
    let reminder = UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
        &NSString::from_str(MAC_CATEGORY_REMINDER),
        &reminder_actions,
        &NSArray::new(),
        UNNotificationCategoryOptions::empty(),
    );
    let open_only_actions = NSArray::from_vec(vec![open]);
    let open_only =
        UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
            &NSString::from_str(MAC_CATEGORY_OPEN_ONLY),
            &open_only_actions,
            &NSArray::new(),
            UNNotificationCategoryOptions::empty(),
        );
    let categories = NSArray::from_vec(vec![reminder, open_only]);
    UNUserNotificationCenter::currentNotificationCenter()
        .setNotificationCategories(&NSSet::setWithArray(&categories));
}

unsafe fn build_request(
    native_id: &str,
    content: &Content,
    sound: Sound,
    trigger: Option<&UNTimeIntervalNotificationTrigger>,
    actions: Option<ActionSet>,
) -> objc2::rc::Retained<UNNotificationRequest> {
    let native = UNMutableNotificationContent::new();
    native.setTitle(&NSString::from_str(&content.title));
    if let Some(body) = &content.body {
        native.setBody(&NSString::from_str(body));
    }
    if let Some(actions) = actions {
        native.setCategoryIdentifier(&NSString::from_str(mac_category(actions)));
    }
    let sound = match sound {
        Sound::Silent => None,
        Sound::System => Some(UNNotificationSound::defaultSound()),
        Sound::Bundled(tone) => {
            let file = BUNDLED_TONES
                .iter()
                .find(|asset| asset.tone == tone)
                .unwrap()
                .file;
            Some(UNNotificationSound::soundNamed(&NSString::from_str(
                &format!("assets/reminders/{file}"),
            )))
        }
    };
    native.setSound(sound.as_deref());
    UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(native_id),
        &native,
        trigger.map(|value| value.as_ref()),
    )
}

unsafe fn withdraw_id(native_id: &str) {
    let center = UNUserNotificationCenter::currentNotificationCenter();
    let ids = NSArray::from_vec(vec![NSString::from_str(native_id)]);
    center.removePendingNotificationRequestsWithIdentifiers(&ids);
    center.removeDeliveredNotificationsWithIdentifiers(&ids);
}

fn reset_namespace(prefix: &str) -> Result<(), Failure> {
    let center = unsafe { UNUserNotificationCenter::currentNotificationCenter() };
    let (pending_send, pending_receive) = std::sync::mpsc::sync_channel(1);
    let pending = RcBlock::new(move |requests: NonNull<NSArray<UNNotificationRequest>>| {
        let ids = unsafe { requests.as_ref() }
            .iter()
            .map(|request| unsafe { request.identifier().to_string() })
            .collect();
        let _ = pending_send.send(ids);
    });
    unsafe { center.getPendingNotificationRequestsWithCompletionHandler(&pending) };
    let pending = pending_receive
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| platform("pending-notification enumeration timed out"))?;
    let (delivered_send, delivered_receive) = std::sync::mpsc::sync_channel(1);
    let delivered = RcBlock::new(move |notifications: NonNull<NSArray<UNNotification>>| {
        let ids = unsafe { notifications.as_ref() }
            .iter()
            .map(|notification| unsafe { notification.request().identifier().to_string() })
            .collect();
        let _ = delivered_send.send(ids);
    });
    unsafe { center.getDeliveredNotificationsWithCompletionHandler(&delivered) };
    let delivered = delivered_receive
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| platform("delivered-notification enumeration timed out"))?;
    unsafe {
        remove_matching_ids(&center, pending, prefix, false);
        remove_matching_ids(&center, delivered, prefix, true);
    }
    Ok(())
}

unsafe fn remove_matching_ids(
    center: &UNUserNotificationCenter,
    ids: Vec<String>,
    prefix: &str,
    delivered: bool,
) {
    let ids = ids
        .into_iter()
        .filter(|id| id.starts_with(prefix))
        .map(|id| NSString::from_str(&id))
        .collect::<Vec<_>>();
    if ids.is_empty() {
        return;
    }
    let ids = NSArray::from_vec(ids);
    if delivered {
        center.removeDeliveredNotificationsWithIdentifiers(&ids);
    } else {
        center.removePendingNotificationRequestsWithIdentifiers(&ids);
    }
}

unsafe fn add(request: &UNNotificationRequest) -> Result<(), Failure> {
    let center = UNUserNotificationCenter::currentNotificationCenter();
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let completion = RcBlock::new(move |error: *mut NSError| {
        let _ = send.send(error.is_null());
    });
    center.addNotificationRequest_withCompletionHandler(request, Some(&completion));
    match receive.recv_timeout(Duration::from_secs(5)) {
        Ok(true) => Ok(()),
        Ok(false) => Err(platform("macOS refused notification request")),
        Err(_) => Err(platform("macOS notification request timed out")),
    }
}

fn unsupported_owner() -> Failure {
    Failure {
        code: FailureCode::Unsupported,
        detail: "signed Koru Agent notification-owner app is not installed".into(),
    }
}
fn invalid(detail: &str) -> Failure {
    Failure {
        code: FailureCode::InvalidRequest,
        detail: detail.into(),
    }
}
fn platform(detail: impl Into<String>) -> Failure {
    Failure {
        code: FailureCode::Platform,
        detail: detail.into(),
    }
}

declare_class!(
    struct NotificationDelegate;

    unsafe impl ClassType for NotificationDelegate {
        type Super = NSObject;
        type Mutability = InteriorMutable;
        const NAME: &'static str = "KoruNotificationOwnerDelegate";
    }

    impl DeclaredClass for NotificationDelegate {}

    unsafe impl NSObjectProtocol for NotificationDelegate {}

    unsafe impl UNUserNotificationCenterDelegate for NotificationDelegate {
        #[method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:)]
        unsafe fn user_notification_center_did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion: &block2::Block<dyn Fn()>,
        ) {
            let request = response.notification().request();
            let native_id = request.identifier().to_string();
            let action_id = response.actionIdentifier().to_string();
            if let Some(namespace) = crate::namespace_from_native_id(&native_id)
                && let Ok(route) = read_route(namespace)
                && let Some(launch) =
                    crate::activation_from_mac_response(&native_id, &action_id, route)
            {
                let _ = forward_activation(&launch);
            }
            completion.call(());
            let marker = MainThreadMarker::new_unchecked();
            NSApplication::sharedApplication(marker).terminate(None);
        }
    }
);

impl NotificationDelegate {
    fn new() -> Retained<Self> {
        unsafe { msg_send_id![Self::alloc(), init] }
    }
}

fn forward_activation(launch: &crate::MacActivationLaunch) -> Result<(), Failure> {
    let current =
        std::env::current_exe().map_err(|_| platform("notification owner path is unavailable"))?;
    let agent = mac_agent_executable_from_owner(&current)
        .ok_or_else(|| platform("notification owner is outside the Koru bundle"))?;
    if !agent.is_file() {
        return Err(platform("Koru Agent is missing from the outer bundle"));
    }
    let mut command = std::process::Command::new(agent);
    command.args(mac_agent_launch_args(launch));
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command
        .spawn()
        .map(|_| ())
        .map_err(|_| platform("could not forward notification activation"))
}

/// Install the delegate before AppKit finishes launching, then wait for the
/// single action which caused LaunchServices to start this background app.
pub fn run_notification_owner_app(titles: &MacActionTitles) -> Result<(), Failure> {
    // The helper calls this directly from `main`; AppKit delivers its delegate
    // callback on that same main thread.
    let marker = unsafe { MainThreadMarker::new_unchecked() };
    unsafe { register_categories(titles) };
    let delegate = NotificationDelegate::new();
    let center = unsafe { UNUserNotificationCenter::currentNotificationCenter() };
    unsafe { center.setDelegate(Some(ProtocolObject::from_ref(delegate.as_ref()))) };
    let app = NSApplication::sharedApplication(marker);
    unsafe { app.run() };
    Ok(())
}

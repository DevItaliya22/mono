// SPDX-License-Identifier: MPL-2.0

//! Windows' manifest-declared COM callback, narrowed immediately to Koru's
//! bounded, content-free activation envelope.

#[cfg(windows)]
use std::collections::BTreeMap;
#[cfg(windows)]
use std::io::{Read as _, Write as _};
#[cfg(windows)]
use std::path::Path;
// `platform_notifications` below is every platform's, and it names `PathBuf`.
use std::path::PathBuf;

#[cfg(windows)]
use fs2::FileExt as _;
#[cfg(windows)]
use serde::{Deserialize, Serialize};

#[cfg(windows)]
const ROUTE_SCHEMA_VERSION: u8 = 1;
#[cfg(windows)]
const MAX_ROUTE_MAP_BYTES: usize = 1024 * 1024;

/// The private, machine-local answer to "which selected root owns this
/// registration?" It is deliberately absent from the native payload.
#[cfg(windows)]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum RouteTarget {
    Channel,
    Explicit { root: PathBuf },
}

#[cfg(windows)]
impl RouteTarget {
    fn for_paths(paths: &crate::Paths) -> Self {
        paths
            .explicit_root()
            .map_or(Self::Channel, |root| Self::Explicit {
                root: root.to_path_buf(),
            })
    }

    fn valid(&self) -> bool {
        match self {
            Self::Channel => true,
            Self::Explicit { root } => root.is_absolute(),
        }
    }
}

#[cfg(windows)]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteFile {
    schema_version: u8,
    routes: BTreeMap<String, RouteTarget>,
}

#[cfg(windows)]
impl Default for RouteFile {
    fn default() -> Self {
        Self {
            schema_version: ROUTE_SCHEMA_VERSION,
            routes: BTreeMap::new(),
        }
    }
}

#[cfg(windows)]
impl RouteFile {
    fn decode(bytes: &[u8]) -> Result<Self, RouteDecodeError> {
        if bytes.len() > MAX_ROUTE_MAP_BYTES {
            return Err(RouteDecodeError::Malformed);
        }
        let value: Self = serde_json::from_slice(bytes).map_err(|_| RouteDecodeError::Malformed)?;
        if value.schema_version != ROUTE_SCHEMA_VERSION {
            return Err(RouteDecodeError::UnsupportedSchema);
        }
        if value
            .routes
            .iter()
            .any(|(key, target)| !valid_route_key(key) || !target.valid())
        {
            return Err(RouteDecodeError::InvalidRoute);
        }
        Ok(value)
    }

    fn target(
        &self,
        namespace: &koru_native_notifications::NotificationNamespace,
        registration: &koru_native_notifications::RegistrationId,
    ) -> Option<RouteTarget> {
        self.routes
            .get(&route_key(namespace, registration))
            .cloned()
    }

    fn insert(
        &mut self,
        namespace: &koru_native_notifications::NotificationNamespace,
        registration: &koru_native_notifications::RegistrationId,
        target: RouteTarget,
    ) -> Option<RouteTarget> {
        self.routes
            .insert(route_key(namespace, registration), target)
    }

    fn restore(
        &mut self,
        namespace: &koru_native_notifications::NotificationNamespace,
        registration: &koru_native_notifications::RegistrationId,
        previous: Option<RouteTarget>,
    ) {
        let key = route_key(namespace, registration);
        if let Some(previous) = previous {
            self.routes.insert(key, previous);
        } else {
            self.routes.remove(&key);
        }
    }

    fn remove_if_target(
        &mut self,
        namespace: &koru_native_notifications::NotificationNamespace,
        registration: &koru_native_notifications::RegistrationId,
        target: &RouteTarget,
    ) {
        let key = route_key(namespace, registration);
        if self.routes.get(&key) == Some(target) {
            self.routes.remove(&key);
        }
    }
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RouteDecodeError {
    Malformed,
    UnsupportedSchema,
    InvalidRoute,
}

#[cfg(windows)]
impl RouteDecodeError {
    fn recoverable(self) -> bool {
        matches!(self, Self::Malformed | Self::InvalidRoute)
    }
}

#[cfg(windows)]
impl std::fmt::Display for RouteDecodeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Malformed => "native activation route map is malformed",
            Self::UnsupportedSchema => "native activation route map has an unsupported schema",
            Self::InvalidRoute => "native activation route map contains an invalid route",
        })
    }
}

#[cfg(windows)]
fn route_key(
    namespace: &koru_native_notifications::NotificationNamespace,
    registration: &koru_native_notifications::RegistrationId,
) -> String {
    format!("{}:{}", namespace.as_str(), registration.as_str())
}

#[cfg(windows)]
fn valid_route_key(key: &str) -> bool {
    let Some((namespace, registration)) = key.split_once(':') else {
        return false;
    };
    koru_native_notifications::NotificationNamespace::try_from(namespace.to_owned()).is_ok()
        && koru_native_notifications::RegistrationId::new(registration).is_ok()
}

#[cfg(windows)]
fn decode_for_mutation(
    bytes: &[u8],
    preserve_corrupt: impl FnOnce() -> Result<(), String>,
) -> Result<RouteFile, String> {
    match RouteFile::decode(bytes) {
        Ok(routes) => Ok(routes),
        Err(error) if error.recoverable() => {
            preserve_corrupt()?;
            Ok(RouteFile::default())
        }
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(windows)]
#[derive(Clone)]
struct RouteMap {
    path: PathBuf,
}

#[cfg(windows)]
impl RouteMap {
    fn for_channel() -> Result<Self, String> {
        let root = koru_channel::channel_root().ok_or_else(|| {
            "could not locate the channel root for native activation routes".to_string()
        })?;
        Ok(Self {
            path: root.join("agent/notification-activation-routes-v1.json"),
        })
    }

    fn read_bytes(&self) -> Result<Option<Vec<u8>>, String> {
        match std::fs::File::open(&self.path) {
            Ok(file) => {
                let mut bytes = Vec::new();
                file.take(MAX_ROUTE_MAP_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|error| format!("could not read native activation routes: {error}"))?;
                Ok(Some(bytes))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("could not read native activation routes: {error}")),
        }
    }

    fn read(&self) -> Result<RouteFile, String> {
        match self.read_bytes()? {
            Some(bytes) => RouteFile::decode(&bytes).map_err(|error| error.to_string()),
            None => Ok(RouteFile::default()),
        }
    }

    fn target(
        &self,
        namespace: &koru_native_notifications::NotificationNamespace,
        registration: &koru_native_notifications::RegistrationId,
    ) -> Result<Option<RouteTarget>, String> {
        self.read()
            .map(|routes| routes.target(namespace, registration))
    }

    fn mutate<T>(&self, change: impl FnOnce(&mut RouteFile) -> T) -> Result<T, String> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| "native activation route map has no parent".to_string())?;
        std::fs::create_dir_all(parent).map_err(|error| {
            format!("could not create native activation route directory: {error}")
        })?;
        let lock_path = self.path.with_extension("lock");
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(lock_path)
            .map_err(|error| format!("could not open native activation route lock: {error}"))?;
        lock.lock_exclusive()
            .map_err(|error| format!("could not lock native activation routes: {error}"))?;
        let mut routes = match self.read_bytes()? {
            Some(bytes) => decode_for_mutation(&bytes, || self.quarantine_corrupt())?,
            None => RouteFile::default(),
        };
        let answer = change(&mut routes);
        self.write(&routes)?;
        let _ = lock.unlock();
        Ok(answer)
    }

    fn quarantine_corrupt(&self) -> Result<(), String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default();
        for ordinal in 0..16 {
            let quarantine = self
                .path
                .with_extension(format!("json.corrupt-{now}-{ordinal}"));
            if quarantine.exists() {
                continue;
            }
            std::fs::copy(&self.path, quarantine).map_err(|error| {
                format!("could not preserve corrupt native activation routes: {error}")
            })?;
            return Ok(());
        }
        Err("could not allocate native activation route quarantine".into())
    }

    fn write(&self, routes: &RouteFile) -> Result<(), String> {
        let staged = self
            .path
            .with_extension(format!("json.{}.part", std::process::id()));
        let result = (|| {
            let mut file = std::fs::File::create(&staged)
                .map_err(|error| format!("could not stage native activation routes: {error}"))?;
            serde_json::to_writer(&mut file, routes)
                .map_err(|error| format!("could not encode native activation routes: {error}"))?;
            file.write_all(b"\n")
                .and_then(|()| file.sync_all())
                .map_err(|error| format!("could not sync native activation routes: {error}"))?;
            replace_file(&staged, &self.path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(staged);
        }
        result
    }
}

#[cfg(windows)]
fn replace_file(source: &Path, target: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let target: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            target.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(format!(
            "could not publish native activation routes: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

#[cfg(windows)]
struct RoutedNotifications {
    inner: Box<dyn koru_native_notifications::NativeNotifications>,
    routes: RouteMap,
    namespace: koru_native_notifications::NotificationNamespace,
    target: RouteTarget,
}

#[cfg(windows)]
impl RoutedNotifications {
    fn route_failure(detail: String) -> koru_native_notifications::Failure {
        koru_native_notifications::Failure {
            code: koru_native_notifications::FailureCode::Platform,
            detail,
        }
    }

    fn register(
        &self,
        registration: &koru_native_notifications::RegistrationId,
    ) -> Result<Option<RouteTarget>, String> {
        self.routes
            .mutate(|routes| routes.insert(&self.namespace, registration, self.target.clone()))
    }

    fn restore(
        &self,
        registration: &koru_native_notifications::RegistrationId,
        previous: Option<RouteTarget>,
    ) -> Result<(), String> {
        self.routes.mutate(|routes| {
            routes.restore(&self.namespace, registration, previous);
        })
    }

    fn unregister(
        &self,
        registration: &koru_native_notifications::RegistrationId,
    ) -> Result<(), String> {
        self.routes.mutate(|routes| {
            routes.remove_if_target(&self.namespace, registration, &self.target);
        })
    }

    fn unregister_target(&self) -> Result<(), String> {
        self.routes.mutate(|routes| {
            routes.routes.retain(|_, target| target != &self.target);
        })
    }
}

#[cfg(windows)]
impl koru_native_notifications::NativeNotifications for RoutedNotifications {
    fn delivery_support(&self) -> koru_native_notifications::DeliverySupport {
        self.inner.delivery_support()
    }

    fn capability(
        &mut self,
    ) -> Result<koru_native_notifications::Capability, koru_native_notifications::Failure> {
        self.inner.capability()
    }

    fn schedule(
        &mut self,
        request: koru_native_notifications::Schedule,
    ) -> Result<koru_native_notifications::NativeHandle, koru_native_notifications::Failure> {
        // Publish the route first so a due-now toast cannot race its own
        // activation. Restore the prior owner if the native add fails.
        let registration = request.registration.clone();
        let previous = self.register(&registration).map_err(Self::route_failure)?;
        match self.inner.schedule(request) {
            Ok(handle) => Ok(handle),
            Err(native) => {
                self.restore(&registration, previous)
                    .map_err(Self::route_failure)?;
                Err(native)
            }
        }
    }

    fn withdraw(
        &mut self,
        registration: &koru_native_notifications::RegistrationId,
    ) -> Result<(), koru_native_notifications::Failure> {
        self.inner.withdraw(registration)?;
        self.unregister(registration).map_err(Self::route_failure)
    }

    fn reset_namespace(&mut self) -> Result<(), koru_native_notifications::Failure> {
        self.inner.reset_namespace()?;
        self.unregister_target().map_err(Self::route_failure)
    }

    fn open_settings(&mut self) -> Result<(), koru_native_notifications::Failure> {
        self.inner.open_settings()
    }

    fn request_authorization(
        &mut self,
    ) -> Result<koru_native_notifications::Authorization, koru_native_notifications::Failure> {
        self.inner.request_authorization()
    }

    fn send_test(
        &mut self,
        content: koru_native_notifications::Content,
        sound: koru_native_notifications::Sound,
    ) -> Result<(), koru_native_notifications::Failure> {
        self.inner.send_test(content, sound)
    }
}

pub(crate) fn platform_notifications(
    paths: &crate::Paths,
    explicit_root: Option<PathBuf>,
    activation_ready: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Box<dyn koru_native_notifications::NativeNotifications> {
    let namespace_key = notification_namespace_key(paths.root());
    let namespace =
        koru_native_notifications::NotificationNamespace::from_stable_key(&namespace_key);
    #[cfg(windows)]
    let routes = match RouteMap::for_channel() {
        Ok(routes) => Some(routes),
        Err(_) => {
            activation_ready.store(false, std::sync::atomic::Ordering::Release);
            None
        }
    };
    let native = koru_native_notifications::platform_notifications_with_activation(
        namespace.clone(),
        explicit_root,
        activation_ready,
    );
    #[cfg(windows)]
    {
        let Some(routes) = routes else {
            return native;
        };
        return Box::new(RoutedNotifications {
            inner: native,
            routes,
            namespace,
            target: RouteTarget::for_paths(paths),
        });
    }
    #[cfg(not(windows))]
    native
}

fn notification_namespace_key(root: &std::path::Path) -> String {
    let value = root.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        value.to_ascii_lowercase()
    } else {
        value
    }
}

#[cfg(windows)]
fn activation_arguments(root: Option<&Path>, envelope: &str) -> Vec<std::ffi::OsString> {
    let mut arguments = Vec::with_capacity(if root.is_some() { 4 } else { 2 });
    if let Some(root) = root {
        arguments.push("--root".into());
        arguments.push(root.as_os_str().to_owned());
    }
    arguments.push("notification-activation".into());
    arguments.push(envelope.into());
    arguments
}

/// Whether the separately packaged Windows COM broker is beside this Agent.
/// Package identity alone is not enough: advertising actions without the
/// executable named by the manifest turns every click into a dead end.
#[cfg(windows)]
pub(crate) fn com_broker_available() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|executable| executable.parent().map(Path::to_path_buf))
        .is_some_and(|directory| directory.join(koru_update::COM_NOTIFICATION_EXE).is_file())
}

#[cfg(not(windows))]
pub(crate) fn com_broker_available() -> bool {
    true
}

#[cfg(windows)]
mod platform {
    use windows::Win32::Foundation::{CLASS_E_NOAGGREGATION, E_FAIL, E_INVALIDARG};
    use windows::Win32::System::Com::{
        CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED, CoInitializeEx, CoRegisterClassObject,
        CoRevokeClassObject, CoUninitialize, IClassFactory, IClassFactory_Impl, REGCLS_MULTIPLEUSE,
    };
    use windows::Win32::UI::Notifications::{
        INotificationActivationCallback, INotificationActivationCallback_Impl,
        NOTIFICATION_USER_INPUT_DATA,
    };
    use windows::core::{GUID, Interface, PCWSTR, Ref, Result as WindowsResult};

    use super::{RouteMap, RouteTarget, activation_arguments};

    #[derive(Clone)]
    struct Broker {
        routes: RouteMap,
        agent_executable: std::path::PathBuf,
    }

    enum DispatchError {
        Invalid,
        Busy,
        Failed(String),
    }

    impl Broker {
        fn dispatch(&self, envelope: &str) -> Result<(), DispatchError> {
            let started = std::time::Instant::now();
            let routed =
                koru_native_notifications::WindowsActivationEnvelope::decode_opaque(envelope)
                    .map_err(|_| DispatchError::Invalid)?;
            tracing::info!(
                target: "koru::notification_activation",
                namespace = routed.namespace.as_str(),
                registration = routed.registration.as_str(),
                action = ?routed.action,
                "decoded Windows notification action"
            );
            let target = self
                .routes
                .target(&routed.namespace, &routed.registration)
                .map_err(DispatchError::Failed)?
                .ok_or(DispatchError::Invalid)?;
            let target_kind = match &target {
                RouteTarget::Channel => "channel",
                RouteTarget::Explicit { .. } => "explicit",
            };
            let (paths, root) = match target {
                RouteTarget::Channel => (
                    crate::paths_for(None).map_err(|error| {
                        DispatchError::Failed(format!(
                            "could not locate the channel Agent: {error}"
                        ))
                    })?,
                    None,
                ),
                RouteTarget::Explicit { root } => {
                    (crate::Paths::for_root(root.clone()), Some(root))
                }
            };
            let activation = routed.into_activation();
            let foreground_granted = if matches!(
                activation.action,
                koru_native_notifications::Action::Open | koru_native_notifications::Action::Snooze
            ) {
                // Windows gave this short-lived COM server the user's toast
                // click. Pass that foreground capability on before handing the
                // action to the already-running Agent; the Agent cannot grant
                // the privilege to itself after this process exits.
                use windows::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};
                unsafe { AllowSetForegroundWindow(ASFW_ANY) }.is_ok()
            } else {
                false
            };
            tracing::info!(
                target: "koru::notification_activation",
                registration = activation.registration.as_str(),
                action = ?activation.action,
                route = target_kind,
                foreground_granted,
                "forwarding native notification activation"
            );
            match forward_with_backoff(&paths, &activation)? {
                true => {
                    tracing::info!(
                        target: "koru::notification_activation",
                        registration = activation.registration.as_str(),
                        action = ?activation.action,
                        elapsed_ms = started.elapsed().as_millis(),
                        destination = "running_agent",
                        "native notification activation handed off"
                    );
                    Ok(())
                }
                false => {
                    let mut command = std::process::Command::new(&self.agent_executable);
                    command.args(activation_arguments(root.as_deref(), envelope));
                    let child = command.spawn().map_err(|error| {
                        DispatchError::Failed(format!(
                            "could not start the notification Agent: {error}"
                        ))
                    })?;
                    tracing::info!(
                        target: "koru::notification_activation",
                        registration = activation.registration.as_str(),
                        action = ?activation.action,
                        elapsed_ms = started.elapsed().as_millis(),
                        destination = "new_agent",
                        child_pid = child.id(),
                        "native notification activation handed off"
                    );
                    Ok(())
                }
            }
        }
    }

    fn forward_with_backoff(
        paths: &crate::Paths,
        activation: &koru_native_notifications::Activation,
    ) -> Result<bool, DispatchError> {
        let started = std::time::Instant::now();
        let mut delay = std::time::Duration::from_millis(10);
        for attempt in 0..8 {
            match crate::client::notification_activation(paths, activation) {
                Ok(forwarded) => {
                    tracing::info!(
                        target: "koru::notification_activation",
                        registration = activation.registration.as_str(),
                        action = ?activation.action,
                        attempt = attempt + 1,
                        elapsed_ms = started.elapsed().as_millis(),
                        running_agent = forwarded,
                        "Agent activation ingress answered"
                    );
                    return Ok(forwarded);
                }
                Err(crate::client::ActivationForwardError::Other(error)) => {
                    return Err(DispatchError::Failed(error));
                }
                Err(crate::client::ActivationForwardError::Busy) if attempt < 7 => {
                    tracing::warn!(
                        target: "koru::notification_activation",
                        registration = activation.registration.as_str(),
                        action = ?activation.action,
                        attempt = attempt + 1,
                        retry_in_ms = delay.as_millis(),
                        "Agent activation ingress is busy; retrying"
                    );
                    std::thread::sleep(delay);
                    delay = (delay * 2).min(std::time::Duration::from_millis(640));
                }
                Err(crate::client::ActivationForwardError::Busy) => {
                    return Err(DispatchError::Busy);
                }
            }
        }
        unreachable!("the bounded activation retry loop always returns")
    }

    pub(crate) fn serve_once() -> Result<(), String> {
        let Some(publisher) = option_env!("KORU_WINDOWS_PACKAGE_PUBLISHER") else {
            return Err("this build has no Windows notification package identity".into());
        };
        let identity = koru_winres::notification_identity(koru_channel::segment(), publisher);
        let clsid = GUID::try_from(identity.toast_activator_clsid)
            .map_err(|error| format!("invalid notification activator CLSID: {error}"))?;
        let executable = std::env::current_exe()
            .map_err(|error| format!("could not locate notification broker executable: {error}"))?;
        let agent_executable = executable
            .parent()
            .ok_or_else(|| "notification broker executable has no parent directory".to_string())?
            .join(koru_update::AGENT_EXE);
        if !agent_executable.is_file() {
            return Err(format!(
                "the notification Agent is missing beside the COM broker: {}",
                agent_executable.display()
            ));
        }
        let (completed, wait_for_completion) = std::sync::mpsc::sync_channel(1);
        let broker = Broker {
            routes: RouteMap::for_channel()?,
            agent_executable,
        };
        let cookie = unsafe { register(clsid, broker, completed) }
            .map_err(|error| format!("could not register notification COM server: {error}"))?;
        let activation = wait_for_completion
            .recv_timeout(std::time::Duration::from_secs(30))
            .map_err(|error| match error {
                std::sync::mpsc::RecvTimeoutError::Timeout => {
                    "Windows did not invoke the notification COM callback within 30 seconds"
                        .to_string()
                }
                std::sync::mpsc::RecvTimeoutError::Disconnected => {
                    "notification COM callback disconnected before completing".to_string()
                }
            });
        let revoke = unsafe { CoRevokeClassObject(cookie) }
            .map_err(|error| format!("could not revoke notification COM server: {error}"));
        unsafe { CoUninitialize() };
        activation??;
        revoke
    }

    unsafe fn register(
        clsid: GUID,
        broker: Broker,
        completed: std::sync::mpsc::SyncSender<Result<(), String>>,
    ) -> WindowsResult<u32> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()? };
        let factory: IClassFactory = Factory { broker, completed }.into();
        match unsafe {
            CoRegisterClassObject(&clsid, &factory, CLSCTX_LOCAL_SERVER, REGCLS_MULTIPLEUSE)
        } {
            Ok(cookie) => Ok(cookie),
            Err(error) => {
                unsafe { CoUninitialize() };
                Err(error)
            }
        }
    }

    #[windows::core::implement(INotificationActivationCallback)]
    struct Callback {
        broker: Broker,
        completed: Option<std::sync::mpsc::SyncSender<Result<(), String>>>,
        outcome: std::sync::Mutex<Option<Result<(), String>>>,
    }

    impl Drop for Callback {
        fn drop(&mut self) {
            let outcome = self
                .outcome
                .lock()
                .ok()
                .and_then(|mut outcome| outcome.take())
                .unwrap_or_else(|| {
                    Err("Windows released the notification callback before invoking it".into())
                });
            if let Some(completed) = self.completed.take() {
                let _ = completed.send(outcome);
            }
        }
    }

    impl INotificationActivationCallback_Impl for Callback_Impl {
        fn Activate(
            &self,
            _app_user_model_id: &PCWSTR,
            invoked_args: &PCWSTR,
            _data: *const NOTIFICATION_USER_INPUT_DATA,
            _count: u32,
        ) -> WindowsResult<()> {
            let started = std::time::Instant::now();
            tracing::info!(
                target: "koru::notification_activation",
                pid = std::process::id(),
                user_input_items = _count,
                "Windows invoked the notification COM callback"
            );
            let outcome = unsafe { invoked_args.to_string() }
                .map_err(|_| DispatchError::Invalid)
                .and_then(|envelope| self.broker.dispatch(&envelope));
            match outcome {
                Ok(()) => {
                    tracing::info!(
                        target: "koru::notification_activation",
                        elapsed_ms = started.elapsed().as_millis(),
                        "Windows notification COM callback completed"
                    );
                    if let Ok(mut recorded) = self.outcome.lock() {
                        *recorded = Some(Ok(()));
                    }
                    Ok(())
                }
                Err(error) => {
                    let report = match &error {
                        DispatchError::Invalid => "invalid notification activation".to_string(),
                        DispatchError::Busy => "notification activation remained busy".to_string(),
                        DispatchError::Failed(detail) => detail.clone(),
                    };
                    if let Ok(mut recorded) = self.outcome.lock() {
                        *recorded = Some(Err(report));
                    }
                    Err(match error {
                        DispatchError::Invalid => windows::core::Error::from_hresult(E_INVALIDARG),
                        DispatchError::Busy => windows::core::Error::from_hresult(E_FAIL),
                        DispatchError::Failed(detail) => windows::core::Error::new(E_FAIL, detail),
                    })
                }
            }
        }
    }

    #[windows::core::implement(IClassFactory)]
    struct Factory {
        broker: Broker,
        completed: std::sync::mpsc::SyncSender<Result<(), String>>,
    }

    impl IClassFactory_Impl for Factory_Impl {
        fn CreateInstance(
            &self,
            outer: Ref<windows::core::IUnknown>,
            iid: *const GUID,
            object: *mut *mut core::ffi::c_void,
        ) -> WindowsResult<()> {
            if !outer.is_null() {
                return Err(windows::core::Error::from_hresult(CLASS_E_NOAGGREGATION));
            }
            let callback: INotificationActivationCallback = Callback {
                broker: self.broker.clone(),
                completed: Some(self.completed.clone()),
                outcome: std::sync::Mutex::new(None),
            }
            .into();
            unsafe { callback.query(iid, object).ok() }
        }

        fn LockServer(&self, _lock: windows::core::BOOL) -> WindowsResult<()> {
            Ok(())
        }
    }
}

#[cfg(not(windows))]
mod platform {
    pub(crate) fn serve_once() -> Result<(), String> {
        Err("Windows notification COM activation is available only on Windows".into())
    }
}

pub(crate) use platform::serve_once;

#[cfg(all(test, windows))]
mod tests;

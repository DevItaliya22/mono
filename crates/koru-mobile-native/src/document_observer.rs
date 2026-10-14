// SPDX-License-Identifier: MPL-2.0

//! JNI ownership and notification marshalling only. Subscription behavior is
//! implemented by the shared mobile host, over its existing commit hub.
use jni::{
    EnvUnowned,
    objects::{JByteArray, JObject},
    strings::JNIString,
    sys::jlong,
};
use koru_mobile_host::document_observer::DocumentObserver;
use prost::Message;
use std::{
    collections::HashMap,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicI64, Ordering},
    },
};

static NEXT: AtomicI64 = AtomicI64::new(1);
static OBSERVERS: OnceLock<Mutex<HashMap<i64, DocumentObserver>>> = OnceLock::new();

fn observers() -> &'static Mutex<HashMap<i64, DocumentObserver>> {
    OBSERVERS.get_or_init(Default::default)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_app_koru_mobile_services_KoruApiNative_nativeObserveDocument<
    'caller,
>(
    mut unowned_env: EnvUnowned<'caller>,
    _this: JObject<'caller>,
    request: JByteArray<'caller>,
    callback: JObject<'caller>,
) -> jlong {
    unowned_env
        .with_env(|env| {
            let bytes = zeroize::Zeroizing::new(env.convert_byte_array(&request)?);
            let callback = env.new_global_ref(&callback)?;
            let vm = env.get_java_vm()?;
            let result = (|| -> Result<i64, String> {
                if bytes.len() > 16 * 1024 {
                    return Err("document_observer_request_too_large".into());
                }
                let request =
                    koru_contract::mobile_proto::DocumentRequest::decode(bytes.as_slice())
                        .map_err(|_| "invalid_protobuf")?;
                let observer = koru_mobile_host::protobuf::observe_document(request, move || {
                    let result: jni::errors::Result<()> = vm.attach_current_thread(|env| {
                        env.call_method(
                            &callback,
                            jni::jni_str!("run"),
                            jni::jni_sig!(() -> void),
                            &[],
                        )?;
                        Ok(())
                    });
                    result.is_ok()
                })?;
                let mut owners = observers().lock().unwrap_or_else(|e| e.into_inner());
                // Bound malformed platform usage, not a durable document registry.
                if owners.len() >= 1024 {
                    return Err("document_observer_capacity".into());
                }
                let id = NEXT.fetch_add(1, Ordering::Relaxed);
                owners.insert(id, observer);
                Ok(id)
            })();
            match result {
                Ok(id) => Ok(id),
                Err(code) => {
                    env.throw_new(
                        JNIString::new("java/lang/RuntimeException"),
                        JNIString::new(&code),
                    )?;
                    Err(jni::errors::Error::JavaException)
                }
            }
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_app_koru_mobile_services_KoruApiNative_nativeCloseDocumentObserver<
    'caller,
>(
    _env: EnvUnowned<'caller>,
    _this: JObject<'caller>,
    id: jlong,
) {
    let owner = observers()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id);
    drop(owner);
}

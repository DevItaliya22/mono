// SPDX-License-Identifier: MPL-2.0

//! LaunchServices URL delivery, added to the delegate **winit** owns.
//!
//! `CFBundleURLTypes` launches the bundle's `CFBundleExecutable`, then AppKit
//! calls `application:openURLs:` on the application delegate.
//!
//! **Winit installs its own `NSApplication` delegate and asserts that it is
//! still there.** `EventLoop::build` sets one, overrides `sendEvent:` to route
//! through it, and every callback afterwards goes through
//! `ApplicationDelegate::get`, which panics with *"tried to get a delegate that
//! was not the one Winit has registered"* if `NSApp.delegate` is anything else.
//! That panic is raised inside an Objective-C frame that cannot unwind, so it
//! aborts the process — on the very first event, before a window is drawn. This
//! shell used to retain a delegate of its own and call `setDelegate:`, which
//! meant Koru could not start on macOS at all.
//!
//! What winit's delegate *does not* implement is `application:openURLs:` — it
//! has `applicationDidFinishLaunching:` and `applicationWillTerminate:` and
//! nothing else. So we add that one method to winit's delegate class before the
//! application finishes launching, which is the point AppKit decides whether the
//! delegate answers URLs. Winit's own instance stays the delegate, its state
//! stays intact, and AppKit forwards `koru://` to us through the ordinary path.
//!
//! A subclass would not do: `ApplicationDelegate::get` reads winit's ivars off
//! whatever object is installed, and winit built its instance already, so the
//! delegate has to remain that object.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

use objc2::runtime::{AnyObject, Imp, Sel};
use objc2::sel;
use objc2_app_kit::NSApplication;
use objc2_foundation::{MainThreadMarker, NSArray, NSURL};

use crate::app::Wake;

static PENDING: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

/// Set once, at install; read on the main thread by the added method.
///
/// The delegate is winit's object and carries winit's ivars, so this cannot
/// ride along in an ivar the way a delegate of our own would have.
static PROXY: OnceLock<winit::event_loop::EventLoopProxy<Wake>> = OnceLock::new();

/// `-[NSObject application:openURLs:]`, in the shape the runtime calls it.
///
/// # Safety
///
/// Installed only for the `application:openURLs:` selector, whose signature is
/// `v@:@@`: AppKit passes the delegate, the selector, a non-null
/// `NSApplication` and a non-null `NSArray<NSURL>`, always on the main thread.
///
/// Nothing here may panic — the caller is an Objective-C frame.
unsafe extern "C-unwind" fn application_open_urls(
    _this: *mut AnyObject,
    _cmd: Sel,
    _application: *mut AnyObject,
    urls: *mut AnyObject,
) {
    // SAFETY: the selector's second argument is a non-null `NSArray<NSURL>`.
    let urls: &NSArray<NSURL> = unsafe { &*urls.cast() };
    let mut pending = PENDING.lock().unwrap_or_else(|error| error.into_inner());
    for url in urls.to_vec() {
        if let Some(url) = url.absoluteString() {
            pending.push_back(url.to_string());
        }
    }
    drop(pending);
    if let Some(proxy) = PROXY.get() {
        let _ = proxy.send_event(Wake::ExternalUrl);
    }
}

/// Teach winit's application delegate to answer `application:openURLs:`.
///
/// Call between `EventLoop::build` — which is what installs the delegate — and
/// `run_app`, which is what finishes launching.
pub(crate) fn install(proxy: winit::event_loop::EventLoopProxy<Wake>) {
    let mtm = MainThreadMarker::new().expect("the winit event loop is created on the main thread");
    // A second call would be a second event loop, which winit does not allow;
    // the first proxy is the live one either way.
    let _ = PROXY.set(proxy);
    let application = NSApplication::sharedApplication(mtm);
    let Some(delegate) = application.delegate() else {
        tracing::warn!(
            target: "koru::macos_urls",
            "no application delegate: koru:// links will not open (winit installs one when the event loop is built)"
        );
        return;
    };
    let delegate: &AnyObject = delegate.as_ref();
    let class = delegate.class();
    let selector = sel!(application:openURLs:);
    if class.responds_to(selector) {
        // Winit grew its own URL callback. Adding ours would fail silently, and
        // overwriting it would take winit's away, so say so instead: the fix is
        // to read the URLs off winit rather than off AppKit.
        tracing::warn!(
            target: "koru::macos_urls",
            "{} already answers application:openURLs:; koru:// links are winit's to deliver now",
            class.name().to_string_lossy()
        );
        return;
    }
    // SAFETY: the selector takes two object arguments and returns nothing, which
    // is the `v@:@@` encoding and `application_open_urls`' own signature. The
    // class is winit's delegate class, live for the process.
    let added = unsafe {
        objc2::ffi::class_addMethod(
            (class as *const objc2::runtime::AnyClass).cast_mut(),
            selector,
            std::mem::transmute::<
                unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject),
                Imp,
            >(application_open_urls),
            c"v@:@@".as_ptr(),
        )
    };
    if !added.as_bool() {
        tracing::warn!(
            target: "koru::macos_urls",
            "could not add application:openURLs: to {}; koru:// links will not open",
            class.name().to_string_lossy()
        );
    }
}

pub(crate) fn take_pending() -> Vec<String> {
    PENDING
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .drain(..)
        .collect()
}

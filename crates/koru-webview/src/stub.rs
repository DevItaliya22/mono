// SPDX-License-Identifier: MPL-2.0

//! The `no-webview` engine: every call answers that there is no runtime.
//!
//! Exists so [`crate::WebSurface`] has one shape in both builds and a suite
//! above this crate can compile against the real API while never creating a
//! browser process.

use std::sync::Arc;
use std::sync::mpsc::Receiver;

use raw_window_handle::HasWindowHandle;

use crate::{PhysicalRect, WebConfig, WebError, WebEvent, debug};

pub(crate) struct Inner;

fn unavailable<T>() -> Result<T, WebError> {
    Err(WebError::RuntimeUnavailable("built with no-webview".into()))
}

impl Inner {
    pub(crate) fn open(
        _parent: &impl HasWindowHandle,
        _bounds: PhysicalRect,
        _config: WebConfig,
        _wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(Self, Receiver<WebEvent>), WebError> {
        debug::log("create", "no-webview", "");
        unavailable()
    }

    pub(crate) fn set_bounds(&self, _bounds: PhysicalRect) -> Result<(), WebError> {
        unavailable()
    }

    pub(crate) fn set_visible(&self, _visible: bool) -> Result<(), WebError> {
        unavailable()
    }

    pub(crate) fn focus(&self) -> Result<(), WebError> {
        unavailable()
    }

    pub(crate) fn focus_parent(&self) -> Result<(), WebError> {
        unavailable()
    }

    pub(crate) fn has_focus(&self) -> bool {
        false
    }

    pub(crate) fn eval(&self, _js: &str) -> Result<(), WebError> {
        unavailable()
    }

    pub(crate) fn navigate(&self, _url: &str) -> Result<(), WebError> {
        unavailable()
    }

    pub(crate) fn clear_all_browsing_data(&self) -> Result<(), WebError> {
        unavailable()
    }
}

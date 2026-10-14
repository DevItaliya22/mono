// SPDX-License-Identifier: MPL-2.0

//! The same claim, accepted on a Tokio reactor.
//!
//! Behind the `tokio` feature, which only a component that **already** has a
//! runtime turns on. A client, and any winit loop, must not be
//! forced inside an async runtime by the crate it uses to find a service.
//!
//! **That gate is one line from being decorative.** `tokio` here turns on
//! `interprocess/tokio`; writing `features = ["tokio"]` on the workspace's
//! `interprocess` entry instead hands it to every client and nothing complains.
//! That mistake once passed a full green test run.
//!
//! The canary is **`cargo tree -p koru-ipc -e normal`**, which must show no
//! `tokio` at all. Do not use `cargo tree -p koru-store-client -i tokio`, which
//! is what the original note said: `reqwest`'s blocking client runs a runtime
//! of its own, so that command has a legitimate answer and would hide this.
//!
//! Everything that decides *whether* the bind succeeded is
//! [`crate::listener::claim`], shared with the blocking half. What differs here
//! is one call — `create_tokio` rather than `create_sync` — and that is the
//! whole reason the sequence lives somewhere both can reach.

use std::io;

use interprocess::local_socket::tokio::prelude::*;
use interprocess::local_socket::tokio::{Listener as Inner, Stream};

use crate::endpoint::Endpoint;
use crate::errors::IpcError;

/// A bound endpoint, polled by a Tokio reactor.
pub struct AsyncListener {
    inner: Inner,
    endpoint: Endpoint,
}

/// The endpoint, and nothing about the listener — see the blocking twin.
impl std::fmt::Debug for AsyncListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsyncListener")
            .field("endpoint", &self.endpoint.name())
            .finish()
    }
}

impl AsyncListener {
    /// Claim `endpoint`, or report that someone else has it.
    ///
    /// **Must be called inside a Tokio runtime**: on Windows the underlying
    /// named pipe server registers with the reactor as it is created, and there
    /// is no runtime-free way to bind one that can later be polled.
    pub fn bind(endpoint: Endpoint) -> Result<Self, IpcError> {
        let inner = crate::listener::claim(&endpoint, create)?;
        Ok(Self { inner, endpoint })
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    pub async fn accept(&self) -> io::Result<Stream> {
        self.inner.accept().await
    }
}

fn create(endpoint: &Endpoint, overwrite: bool) -> io::Result<Inner> {
    crate::listener::options(endpoint, overwrite)?.create_tokio()
}

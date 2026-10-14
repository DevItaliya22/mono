// SPDX-License-Identifier: MPL-2.0

//! Reaching an endpoint somebody else is holding.
//!
//! Blocking, and that is the point: a winit loop cannot be put inside an async
//! runtime by the crate it uses to find a service, so the client half of this
//! crate needs no reactor at all. Only a listener does, and only when its owner
//! already has one — see [`crate::listener_async`].
//!
//! ## There is no polling loop here
//!
//! Connecting *is* how you learn something is there. A client that could not
//! connect has its answer; whether to start a service and try again is a
//! decision about a product, and this crate has no opinion on it — the retry
//! after a spawn lives with whoever knows how to spawn.

use std::time::Duration;

use interprocess::local_socket::Stream;
use interprocess::local_socket::prelude::*;

use crate::endpoint::Endpoint;
use crate::errors::IpcError;

/// Open a connection to `endpoint`, with no timeouts of its own.
///
/// The right shape for a **long-lived** connection: a reader thread parked on
/// `read_exact` is waiting on purpose, and a timeout there would turn an idle
/// peer into a disconnection.
pub fn connect(endpoint: &Endpoint) -> Result<Stream, IpcError> {
    match Stream::connect(name_of(endpoint)?) {
        Ok(stream) => {
            crate::ipc_debug!("connect: {} -> ok", endpoint.name());
            Ok(stream)
        }
        Err(e) => {
            // **The raw `ErrorKind` as well as the classification.** Which of
            // `NotFound` and `ConnectionRefused` arrived is platform trivia to a
            // caller and the whole question to somebody reading a log: the first
            // is a name that does not exist, the second a Unix socket file a
            // crash left behind.
            crate::ipc_debug!("connect: {} -> {:?} ({e})", endpoint.name(), e.kind());
            Err(classify(endpoint, e))
        }
    }
}

/// The same, with a deadline on both directions.
///
/// The right shape for a **request and a reply**: a peer that accepted the
/// connection and then wedged would otherwise block forever. Both directions,
/// because a stall on the write is as likely as one on the read and a timeout
/// on only one of them is a timeout on neither.
pub fn connect_timed(endpoint: &Endpoint, timeout: Duration) -> Result<Stream, IpcError> {
    let stream = connect(endpoint)?;
    let _ = stream.set_recv_timeout(Some(timeout));
    let _ = stream.set_send_timeout(Some(timeout));
    Ok(stream)
}

/// Is anything listening on this endpoint right now?
///
/// The one question the whole design turns on, and the reason it is answered by
/// connecting rather than by reading a lock or a pid.
///
/// **No handshake**: this asks whether *something* accepted, not whether it is
/// a peer that will talk to us. A process that binds the name and refuses to
/// speak still owns the name, and a second one must stand down for it.
pub fn probe(endpoint: &Endpoint) -> bool {
    let answered = name_of(endpoint).is_ok_and(|name| Stream::connect(name).is_ok());
    crate::ipc_debug!(
        "probe: {} -> {}",
        endpoint.name(),
        if answered {
            "answering"
        } else {
            "nothing there"
        }
    );
    answered
}

/// Turn an io error from `connect` into the thing a caller can act on.
///
/// `NotFound` and `ConnectionRefused` are both "nothing is listening" — the
/// first is a Windows pipe that does not exist and a Unix path that does not
/// exist, the second is a Unix socket file left behind by a crash. They are the
/// same answer to a client and a different one to a process trying to bind,
/// which is why [`crate::listener`] does its own probe rather than reusing this.
fn classify(endpoint: &Endpoint, e: std::io::Error) -> IpcError {
    use std::io::ErrorKind::*;
    match e.kind() {
        NotFound | ConnectionRefused => IpcError::NotRunning(endpoint.name().to_string()),
        PermissionDenied => {
            IpcError::PermissionDenied(format!("{} belongs to another user", endpoint.name()))
        }
        _ => IpcError::TransportFailed(format!("{}: {e}", endpoint.name())),
    }
}

/// The platform-native name `interprocess` takes.
///
/// Public because a test that means to speak the wire by hand needs one, and
/// because a diagnostic that prints where it looked is worth more than one that
/// says only "not running".
#[cfg(windows)]
pub fn name_of(endpoint: &Endpoint) -> Result<interprocess::local_socket::Name<'static>, IpcError> {
    use interprocess::local_socket::GenericNamespaced;
    endpoint
        .name()
        .to_ns_name::<GenericNamespaced>()
        .map(|n| n.into_owned())
        .map_err(|e| IpcError::TransportFailed(format!("endpoint name: {e}")))
}

/// The platform-native name `interprocess` takes. See the Windows twin.
#[cfg(unix)]
pub fn name_of(endpoint: &Endpoint) -> Result<interprocess::local_socket::Name<'static>, IpcError> {
    use interprocess::local_socket::GenericFilePath;
    endpoint
        .name()
        .to_fs_name::<GenericFilePath>()
        .map(|n| n.into_owned())
        .map_err(|e| IpcError::TransportFailed(format!("endpoint name: {e}")))
}

/// Put a deadline on both directions, or take one off.
///
/// **Both, always.** A peer that accepted and then wedged blocks the *write* as
/// readily as the read, and a timeout on only one of them is a timeout on
/// neither.
///
/// `None` is what a long-lived connection wants once its handshake is done: a
/// reader parked on `read_exact` is waiting on purpose, and a deadline there
/// turns an idle peer into a disconnection.
pub fn set_timeouts(stream: &Stream, timeout: Option<Duration>) {
    // Ignored rather than reported: a platform that will not set one still has
    // a usable socket, and refusing to talk to a peer because a timeout could
    // not be applied would trade something that works for something tidier.
    let _ = stream.set_recv_timeout(timeout);
    let _ = stream.set_send_timeout(timeout);
}

/// Split a connection into a half to read and a half to write.
///
/// Re-exported as a function so a caller does not have to import
/// `interprocess`'s trait to reach an inherent-looking method — the same reason
/// [`crate::Stream`] itself is re-exported. **Both halves are `Send`**, which is
/// what makes a reader thread and a writer queue over one socket possible, and
/// therefore what lets an event loop own a long-lived protocol without ever
/// blocking on it.
pub fn split(stream: Stream) -> (impl std::io::Read + Send, impl std::io::Write + Send) {
    use interprocess::local_socket::traits::Stream as _;
    stream.split()
}

#[cfg(test)]
mod tests;

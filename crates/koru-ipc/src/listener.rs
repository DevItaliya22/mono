// SPDX-License-Identifier: MPL-2.0

//! Claiming an endpoint, and accepting on it — blocking.
//!
//! ## Binding is the claim; accepting is the readiness signal
//!
//! These are two moments and the gap between them is load-bearing:
//!
//! ```text
//! bind        nothing else may hold this name    <- replaces any lock file
//! ...open whatever the service owns...
//! accept      ...and only now is there something to talk to
//! ```
//!
//! A second process fails at *bind*, before it has touched a file. A client
//! that connects in the window between the two waits at `accept` until the
//! service is ready, rather than being told an address that does not work yet.
//! Both are the behaviour you want and neither needed to be arranged.
//!
//! ## Why a blocking listener exists at all
//!
//! Because the agent is a synchronous winit loop. The old Store rendezvous only
//! ever needed the async half — the daemon already runs a Tokio runtime — and
//! for a while that was the only listener in the tree, which is what made the
//! app write its own raw `CreateNamedPipeW` accept loop instead. That loop, its
//! hand-rolled line format and its Windows-only lifetime are what this replaces
//! ([ADR-105](../../../docs/decisions/README.md)).
//!
//! A blocking accept belongs on a thread of its own, and a connection belongs
//! on another. `interprocess`'s `Stream::split` hands back a `RecvHalf` and a
//! `SendHalf` that are both `Send`, which is what lets one thread park on a
//! read while whoever owns the protocol queues writes from somewhere else —
//! **the event loop must never block on either**.

use std::io;

use interprocess::local_socket::ListenerOptions;
use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{Listener as Inner, Stream};

use crate::endpoint::Endpoint;
use crate::errors::IpcError;

/// A bound endpoint. Holding one is the claim on this service and channel.
pub struct Listener {
    inner: Inner,
    endpoint: Endpoint,
}

/// The endpoint, and nothing about the listener.
///
/// Hand-written rather than derived because the listener's own `Debug` prints a
/// raw handle, and a handle in a log is noise that reads like an address.
impl std::fmt::Debug for Listener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Listener")
            .field("endpoint", &self.endpoint.name())
            .finish()
    }
}

impl Listener {
    /// Claim `endpoint`, or report that someone else has it.
    pub fn bind(endpoint: Endpoint) -> Result<Self, IpcError> {
        let inner = claim(&endpoint, create)?;
        Ok(Self { inner, endpoint })
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Wait for the next peer. Blocks.
    pub fn accept(&self) -> io::Result<Stream> {
        let accepted = self.inner.accept();
        match &accepted {
            // The pid is the *transport's* answer, never a claim a peer made —
            // and a log that shows which process connected is what turns "the
            // agent did not route it" into a question with an answer.
            Ok(stream) => crate::ipc_debug!(
                "accept: {} <- pid={:?} same_user={}",
                self.endpoint.name(),
                crate::peer_pid(stream),
                crate::same_user(stream)
            ),
            Err(e) => crate::ipc_debug!("accept: {} failed: {e}", self.endpoint.name()),
        }
        accepted
    }
}

/// Bind, and work out what a failure meant.
///
/// Shared by both listeners, because the *sequence* is the interesting part and
/// it must not exist twice: the two flavours differ only in which `create_*`
/// call `interprocess` is asked for.
pub(crate) fn claim<L>(
    endpoint: &Endpoint,
    create: impl Fn(&Endpoint, bool) -> io::Result<L>,
) -> Result<L, IpcError> {
    prepare_parent(endpoint)?;

    let taken = match create(endpoint, false) {
        Ok(listener) => {
            crate::ipc_debug!("bind: {} -> held", endpoint.name());
            return Ok(listener);
        }
        // **Two error kinds mean "taken", and which one is platform trivia.**
        // Unix answers `AddrInUse`. Windows answers `PermissionDenied`:
        // `CreateNamedPipe` with `FILE_FLAG_FIRST_PIPE_INSTANCE` on a name that
        // already exists returns `ERROR_ACCESS_DENIED`, not a busy code.
        // Mapping that straight through would tell a developer their *second*
        // process had a permissions problem, which is the wrong thing to go and
        // investigate.
        //
        // (The flag itself is load-bearing and interprocess sets it: a named
        // pipe without it accepts a second *instance* of the same name, and two
        // servers would silently share the endpoint with clients dealt between
        // them.)
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::AddrInUse | io::ErrorKind::PermissionDenied
            ) =>
        {
            e
        }
        Err(e) => {
            crate::ipc_debug!("bind: {} -> failed {:?} ({e})", endpoint.name(), e.kind());
            return Err(IpcError::BindFailed(format!(
                "could not bind {}: {e}",
                endpoint.name()
            )));
        }
    };
    // **Three outcomes that arrive as two error kinds**, and which kind is
    // platform trivia — so the log says what was *tried* before it says what
    // was concluded.
    crate::ipc_debug!(
        "bind: {} -> taken ({:?}); probing to tell a live peer from a stale name",
        endpoint.name(),
        taken.kind()
    );

    // Which of the two it is gets answered the same way everything else here
    // does: by *connecting*. Not by a lock, not by a pid file.
    if crate::client::probe(endpoint) {
        crate::ipc_debug!("bind: {} -> already held by a live peer", endpoint.name());
        return Err(IpcError::AlreadyBound(endpoint.name().to_owned()));
    }

    if taken.kind() == io::ErrorKind::PermissionDenied {
        // Nothing answered and we still may not bind: a real permissions
        // problem — on Windows a pipe of that name owned by another user, on
        // Unix a directory we cannot write.
        crate::ipc_debug!(
            "bind: {} -> nothing answered and we still may not bind: a real permissions problem",
            endpoint.name()
        );
        return Err(IpcError::PermissionDenied(format!(
            "{}: {taken}",
            endpoint.name()
        )));
    }

    // Nothing answered and the name is in use: a socket file left behind by a
    // crash. Windows never reaches here — its pipes vanish with the process —
    // so this is the whole of the Unix-only stale-endpoint recovery, and it
    // needs no manual cleanup.
    // A socket file a crash left behind. Quiet on success, deliberately: a
    // recovery that worked is a *value*, not news — the rule
    // `WorkspaceStore::load` follows with `Opened::Recovered`.
    crate::ipc_debug!("bind: {} -> stale socket, taking it over", endpoint.name());
    create(endpoint, true).map_err(|e| {
        IpcError::BindFailed(format!(
            "could not take over the stale endpoint at {}: {e}",
            endpoint.name()
        ))
    })
}

// ------------------------------------------------------------- the two halves

#[cfg(windows)]
fn create(endpoint: &Endpoint, overwrite: bool) -> io::Result<Inner> {
    options(endpoint, overwrite)?.create_sync()
}

#[cfg(unix)]
fn create(endpoint: &Endpoint, overwrite: bool) -> io::Result<Inner> {
    options(endpoint, overwrite)?.create_sync()
}

/// The options both flavours bind with, including the one decision that is
/// genuinely about the endpoint: who may reach it.
#[cfg(windows)]
pub(crate) fn options(
    endpoint: &Endpoint,
    overwrite: bool,
) -> io::Result<ListenerOptions<'static>> {
    use interprocess::local_socket::GenericNamespaced;
    use interprocess::os::windows::local_socket::ListenerOptionsExt as _;

    let name = endpoint
        .name()
        .to_ns_name::<GenericNamespaced>()?
        .into_owned();
    Ok(ListenerOptions::new()
        .name(name)
        .try_overwrite(overwrite)
        .security_descriptor(crate::endpoint::owner::only_this_user()?))
}

/// See the Windows twin.
///
/// 0600 on the socket itself is advisory rather than load-bearing: the docs
/// list `mode` as unsupported on several platforms, macOS included — and
/// "unsupported" is not "ignored": on macOS a requested mode fails the *bind*
/// with `Unsupported`, which is how the agent could not start on a Mac at all.
/// So the mode is only requested where it works, and the 0700 parent directory
/// ([`prepare_parent`]) stays the actual barrier everywhere.
#[cfg(unix)]
pub(crate) fn options(
    endpoint: &Endpoint,
    overwrite: bool,
) -> io::Result<ListenerOptions<'static>> {
    use interprocess::local_socket::GenericFilePath;

    let name = endpoint
        .name()
        .to_fs_name::<GenericFilePath>()?
        .into_owned();
    let options = ListenerOptions::new().name(name).try_overwrite(overwrite);
    #[cfg(not(target_os = "macos"))]
    let options = {
        use interprocess::os::unix::local_socket::ListenerOptionsExt as _;
        options.mode(0o600)
    };
    Ok(options)
}

/// Create the directory the socket lives in, at `0700`, and refuse one this
/// user does not own.
///
/// **Both the service directory and the per-uid directory above it**, because
/// the sockets live under `/tmp` and that is world-writable ([ADR-417]): the
/// squattable name is `/tmp/koru-<uid>`, one level up from the directory this
/// endpoint actually needs. Checking only the leaf would leave the interesting
/// one unchecked.
#[cfg(unix)]
pub(crate) fn prepare_parent(endpoint: &Endpoint) -> Result<(), IpcError> {
    let Some(dir) = endpoint.parent() else {
        return Ok(());
    };
    // Outermost first: `/tmp/koru-<uid>` has to be ours before anything is
    // created inside it, or the check happens after the exposure.
    if let Some(runtime) = dir.parent() {
        ensure_private_directory(runtime)?;
    }
    ensure_private_directory(dir)
}

/// One directory: created `0700` if absent, and otherwise proved to be a plain
/// directory this user owns before anything is written into it.
#[cfg(unix)]
fn ensure_private_directory(dir: &std::path::Path) -> Result<(), IpcError> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

    // `symlink_metadata`, never `metadata`: the question is what this *name*
    // is, and a symlink that resolves to a directory we do own would answer
    // the wrong one — it is the redirection itself that has to be refused.
    let existing = match std::fs::symlink_metadata(dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // The mode is set *by* the builder rather than after it, so there is
            // no window in which the directory exists at the umask's mode.
            return std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)
                .map_err(|e| IpcError::BindFailed(format!("{}: {e}", dir.display())));
        }
        Err(error) => return Err(IpcError::BindFailed(format!("{}: {error}", dir.display()))),
    };

    if !existing.is_dir() {
        return Err(IpcError::PermissionDenied(format!(
            "{} is not a directory; refusing to place a socket there",
            dir.display()
        )));
    }
    // **Ownership is the check `/tmp` makes necessary.** Another local user can
    // create `/tmp/koru-<uid>` before we do. Binding inside it would fail
    // anyway, but as an unexplained `EACCES` three frames away; naming it here
    // turns a squatted directory into a sentence somebody can act on.
    let owner = existing.uid();
    let us = crate::endpoint::owner::current_uid();
    if owner != us {
        return Err(IpcError::PermissionDenied(format!(
            "{} is owned by uid {owner}, not {us}; refusing to place a socket there",
            dir.display()
        )));
    }
    // It already existed and is ours — from an earlier run, or from something
    // else that chose the same name. Tighten it rather than trusting it: a
    // directory left at 0755 by an older build would leave every socket in it
    // readable.
    let mut perms = existing.permissions();
    if perms.mode() & 0o077 != 0 {
        perms.set_mode(0o700);
        std::fs::set_permissions(dir, perms).map_err(|e| {
            IpcError::PermissionDenied(format!(
                "{} is readable by other users and could not be tightened: {e}",
                dir.display()
            ))
        })?;
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn prepare_parent(_endpoint: &Endpoint) -> Result<(), IpcError> {
    // The pipe namespace is not the filesystem; there is no directory to make.
    Ok(())
}

#[cfg(test)]
mod tests;

// SPDX-License-Identifier: MPL-2.0

//! Who is on the other end.
//!
//! The only thing in this crate that is about a *peer* rather than about bytes,
//! and it is here because both answers come off the transport rather than out
//! of anything the peer said.
//!
//! ## A pid a peer told you is not a pid
//!
//! It is a claim. A process that hands one over can hand over somebody else's,
//! and a well-behaved process that restarts hands over one that has been reused
//! — so a routing table keyed on a self-reported pid eventually addresses the
//! wrong window, and a `AllowSetForegroundWindow` on one gives away a
//! capability nobody chose. [`peer_pid`] asks the operating system instead.
//!
//! Both platforms answer, which was the pleasant surprise: `interprocess`
//! implements `peer_creds` over `SO_PEERCRED` on Unix and over
//! `GetNamedPipeClientProcessId` on Windows. Where it cannot — the
//! documentation lists platforms where credentials are unavailable — the answer
//! is `None`, and the caller must degrade rather than guess. **The connection
//! is the identity; the pid is only a capability**, so "the window did not come
//! forward" is the worst that a missing one can cost.

use interprocess::local_socket::traits::StreamCommon;

/// The process at the other end of `stream`, if the platform will say.
///
/// `u32` on both platforms: a Unix `pid_t` is signed and a Windows process id
/// is not, but a live pid is never negative and every Win32 call that takes one
/// takes a `u32`.
pub fn peer_pid<S: StreamCommon>(stream: &S) -> Option<u32> {
    let pid = stream.peer_creds().ok().and_then(|creds| creds.pid())?;
    #[cfg(windows)]
    {
        Some(pid)
    }
    #[cfg(unix)]
    {
        u32::try_from(pid).ok()
    }
}

/// Whether the peer is the same OS user as this process.
///
/// **Defence in depth, not the guarantee.** The guarantee is the security
/// descriptor on Windows and the directory mode on Unix, both of which stop the
/// connection before it exists. This catches the case where one of those was
/// wrong — an inherited directory, a descriptor that failed to apply — and it
/// is cheap enough to be worth having for that alone.
///
/// Unavailable credentials are **not** evidence of an intruder: refusing every
/// client on a platform that does not report them would make the service
/// unusable there, and the endpoint's own permissions still hold. On Windows
/// this is always `true` — the API reports the peer's pid but not its SID, and
/// a pid is subject to reuse, so there is nothing here that the descriptor
/// applied at creation does not already do better.
pub fn same_user<S: StreamCommon>(stream: &S) -> bool {
    #[cfg(unix)]
    {
        match stream.peer_creds() {
            Ok(creds) => creds.euid() == Some(crate::endpoint::owner::current_uid()),
            Err(_) => true,
        }
    }
    #[cfg(windows)]
    {
        let _ = stream;
        true
    }
}

// SPDX-License-Identifier: MPL-2.0

//! Where an endpoint is, and who is allowed to reach it.
//!
//! ## The name is derived, never configured
//!
//! An endpoint is identified by **the OS user who owns it, the channel it
//! serves, and the service it is** — never by a display name, never by a port.
//! Two users on one machine must never meet, two channels must never meet, and
//! two services must never collide; all three are settled by the name rather
//! than by a check afterwards.
//!
//! ```text
//! Windows  \\.\pipe\<service>.<segment>.<sid>
//! Unix     <runtime dir>/<service>/<segment>.sock   (the directory is 0700)
//! ```
//!
//! An endpoint on an explicit root gets a digest of that root appended. See
//! [`Endpoint::for_service_root`] for why — it was not obvious until the suite
//! broke.
//!
//! The Windows SID is in the name because named pipes live in one machine-wide
//! namespace: without it, the second user to start a service would collide with
//! the first, and — far worse — a client would dial it. The SID is not a
//! secret; it is an account identifier that appears in any process listing's
//! security tab, and having it legible is what makes a wrong endpoint
//! diagnosable.
//!
//! ## Connecting **is** the liveness check
//!
//! This is the whole reason for the mechanism. A file holding an address could
//! say a service was running when it was not: a process killed with the power
//! switch left the file behind, and a client dialled a dead port — or, worse, a
//! live port that something else had since been given. You cannot connect to a
//! pipe nobody is listening on, so the question answers itself.
//!
//! ## The one place that is not true, and what is done about it
//!
//! Windows named pipes vanish with the process. **Unix sockets do not.** A
//! crash leaves the socket file on the filesystem, and a later `bind` gets
//! `AddrInUse` from a file with nothing behind it. Interprocess deletes the
//! file when the listener is *dropped*, which covers every exit except the ones
//! that matter here.
//!
//! So the Unix bind is a sequence, not a call: bind; on `AddrInUse`, try to
//! *connect*; if the connection is refused the file is stale and may be
//! overwritten; if it succeeds, something really is listening and this process
//! should stand down. That is a documented order of operations, not a lock file
//! — the point of the whole design is that no side-channel is the authority.
//! It lives in [`crate::listener`], which is where binding happens.
//!
//! ## Restricting it to one user
//!
//! Windows: an explicit security descriptor granting `GA` (all access) to the
//! owning SID alone, `P`-protected so nothing is inherited. The default would
//! have been *nearly* right — a named pipe created with no descriptor inherits
//! the token's default DACL, which for an ordinary user grants that user and
//! SYSTEM — but for an **elevated** process the default DACL grants
//! Administrators rather than the specific account, which is a wider door than
//! anyone chose. Naming the SID makes elevation irrelevant.
//!
//! Unix: the socket's parent directory is `0700`. The socket's own mode is set
//! too, but it is the belt and not the braces —
//! `interprocess`'s `mode()` documents itself as unsupported on some platforms
//! (macOS among them) and returns `Unsupported` there. A directory mode works
//! everywhere, so that is what the guarantee rests on.

use std::path::PathBuf;

use crate::errors::IpcError;

/// The rendezvous for one user, one channel and one service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    service: String,
    segment: String,
    /// The platform-native name. On Windows a pipe name; on Unix a path.
    name: String,
    /// The directory that must exist and be `0700` before binding. `None` on
    /// Windows, where the pipe namespace is not the filesystem.
    parent: Option<PathBuf>,
}

impl Endpoint {
    /// The endpoint for a service on a channel's own root — the ordinary case.
    ///
    /// `service` is a short ASCII token such as [`crate::STORE`] or
    /// [`crate::AGENT`]. It reaches a pipe name and a directory name, so it is
    /// expected to be a constant rather than anything a user typed; the
    /// *segment* beside it is checked, because that one arrives from a command
    /// line.
    pub fn for_service(service: &str, segment: &str) -> Result<Self, IpcError> {
        if !koru_channel::SEGMENTS.contains(&segment) {
            return Err(IpcError::UnknownSegment {
                segment: segment.to_string(),
                expected: koru_channel::SEGMENTS.join(", "),
            });
        }
        let endpoint = platform::endpoint(service, segment);
        match &endpoint {
            Ok(endpoint) => crate::ipc_debug!(
                "endpoint: service={service} segment={segment} name={}",
                endpoint.name()
            ),
            // The Windows SID read is the only way this fails, and it fails the
            // same way for every service — so saying which one asked is what
            // makes the line worth having.
            Err(e) => {
                crate::ipc_debug!("endpoint: service={service} segment={segment} failed: {e}")
            }
        }
        endpoint
    }

    /// The endpoint for a service on a **specific root**, rather than on the
    /// channel's own.
    ///
    /// ## Why this exists, which was not obvious until the tests broke
    ///
    /// A rendezvous keyed on user, channel and service alone makes a service a
    /// singleton per channel — which is exactly right for the product and
    /// impossible for the suite, where `concurrency.rs`, `perf.rs` and
    /// `vertical_slice.rs` all run several stores on temp directories at once.
    /// Worse, a service on an explicit `--root` would have had no discoverable
    /// endpoint at all, so the escape hatch that exists for developers would
    /// have been the one configuration no client could find.
    ///
    /// So the rule is: **the endpoint identifies the instance, and an instance
    /// is identified by its root.** For the channel's own root that is user +
    /// channel + service and the name stays legible; for an explicit root a
    /// short digest is appended. The singleton property is unchanged where it
    /// matters — two clients on the default path always meet.
    ///
    /// **The channel's own root must produce the same name as
    /// [`for_service`](Self::for_service)**, and that is the load-bearing half.
    /// It did not, once: the store's daemon resolves a root before binding and
    /// so always went through here, while an ordinary client goes through
    /// `for_service` — so a store started with a plain `koru-store-server serve` bound
    /// one name and every client dialled another. The whole suite missed it,
    /// because a test passes `--root` on *both* sides and the two agreed with
    /// each other perfectly.
    pub fn for_service_root(
        service: &str,
        segment: &str,
        root: &std::path::Path,
    ) -> Result<Self, IpcError> {
        let mut endpoint = Self::for_service(service, segment)?;
        // **The half that goes wrong silently.** A store on the channel's own
        // root must derive the *same* name a plain client dials; if this
        // branches the wrong way the service binds a name nobody looks at, and
        // both sides are individually correct. So the line says which way it
        // went, not merely what came out.
        if is_the_channels_own_root(root) {
            crate::ipc_debug!(
                "endpoint: root={} is the channel's own; name unqualified",
                root.display()
            );
        } else {
            endpoint.qualify(&digest(&root_identity(root)));
            crate::ipc_debug!(
                "endpoint: root={} qualified name={}",
                root.display(),
                endpoint.name()
            );
        }
        Ok(endpoint)
    }

    /// The endpoint for a service whose authored root is the channel root.
    ///
    /// Store-owned services use [`for_service_root`](Self::for_service_root),
    /// whose ordinary root is `<channel>/store`. Product-level services such
    /// as the API Gateway instead receive `<channel>` and must still meet an
    /// unrooted client at the ordinary channel endpoint.
    pub fn for_service_channel_root(
        service: &str,
        segment: &str,
        root: &std::path::Path,
    ) -> Result<Self, IpcError> {
        let mut endpoint = Self::for_service(service, segment)?;
        if koru_channel::channel_root().as_deref() == Some(root) {
            crate::ipc_debug!(
                "endpoint: root={} is the channel's own; name unqualified",
                root.display()
            );
        } else {
            endpoint.qualify(&digest(&root_identity(root)));
            crate::ipc_debug!(
                "endpoint: channel-root={} qualified name={}",
                root.display(),
                endpoint.name()
            );
        }
        Ok(endpoint)
    }

    /// A short-lived, claim-scoped endpoint beneath one service root.
    ///
    /// Unlike [`Self::for_service_root`], this always appends a digest: two
    /// claims under the same installation must be able to coexist. The claim
    /// is an internally minted opaque token, canonicalized by its caller; it is
    /// folded into the digest rather than exposed in a pipe or socket name.
    /// Root spelling follows the same lexical normalization as rooted service
    /// endpoints and likewise does not depend on whether the directory exists.
    pub fn for_service_claim(
        service: &str,
        segment: &str,
        root: &std::path::Path,
        claim: &str,
    ) -> Result<Self, IpcError> {
        let mut endpoint = Self::for_service(service, segment)?;
        let identity = format!("{}\0{claim}", root_identity(root));
        // Claims are numerous and ephemeral, unlike the handful of explicit
        // service roots. Keep 64 bits of the stable hash so unrelated window
        // replays do not accidentally exclude one another.
        endpoint.qualify(&digest64(&identity));
        crate::ipc_debug!(
            "endpoint: root={} claim-scoped name={}",
            root.display(),
            endpoint.name()
        );
        Ok(endpoint)
    }

    pub fn service(&self) -> &str {
        &self.service
    }

    pub fn segment(&self) -> &str {
        &self.segment
    }

    /// The platform-native name, for diagnostics and for `interprocess`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The directory that has to exist, on platforms with one.
    pub fn parent(&self) -> Option<&std::path::Path> {
        self.parent.as_deref()
    }

    /// Append a discriminator, keeping the platform's shape intact.
    fn qualify(&mut self, suffix: &str) {
        match &mut self.parent {
            // Unix: the name is a path, so the suffix goes on the file stem and
            // not on the extension — `nightly.a1b2c3d4.sock`, so a person
            // listing the directory still sees sockets.
            Some(_) => {
                let path = std::path::PathBuf::from(&self.name);
                let stem = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let renamed = path.with_file_name(format!("{stem}.{suffix}.sock"));
                self.name = renamed.to_string_lossy().into_owned();
            }
            // Windows: a pipe name is one flat string.
            None => self.name.push_str(&format!(".{suffix}")),
        }
    }
}

/// Whether `root` is the place this channel keeps its data anyway.
///
/// A component that takes a `--root` resolves one before it binds, so it
/// *always* calls [`Endpoint::for_service_root`] — including on a plain launch,
/// where the root it resolved is the channel's own. An ordinary client goes
/// through [`Endpoint::for_service`]. Those two have to produce the same name
/// or the service binds somewhere nobody looks.
///
/// This helper is for Store-owned services, whose ordinary root is
/// `koru_channel::store_root()`. Product services rooted one directory higher
/// use [`Endpoint::for_service_channel_root`] instead.
///
/// Compared after `canonicalize` where both sides exist, because
/// `C:\Users\me\AppData\Local` and a path through a junction or a `..` are the
/// same directory and must not be two endpoints. Falling back to a plain
/// comparison when canonicalizing fails is deliberate: a root that does not
/// exist yet is about to be created by the caller, and the literal paths still
/// match in the case that matters.
fn is_the_channels_own_root(root: &std::path::Path) -> bool {
    let Some(own) = koru_channel::store_root() else {
        return false;
    };
    if own == root {
        return true;
    }
    match (own.canonicalize(), root.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Eight hex characters of FNV-1a over a string.
///
/// **Not `DefaultHasher`**, which is documented as unstable across Rust
/// releases: two binaries built from one tree would agree today and a
/// toolchain upgrade would silently move every endpoint, which presents as
/// "the app cannot find the service it just started". FNV-1a is ten lines and
/// deterministic forever, and this is a *name*, not a security boundary — the
/// user identity beside it is what keeps people apart.
fn digest(value: &str) -> String {
    format!("{:08x}", fnv1a(value) as u32)
}

fn digest64(value: &str) -> String {
    format!("{:016x}", fnv1a(value))
}

fn fnv1a(value: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The identity used to identify an explicit service root.
///
/// Windows' verbatim and ordinary spellings are normalized before hashing.
/// That is not cosmetic: `std::fs::canonicalize` returns a `\\?\` path, so a
/// daemon that validates a root before binding and a later `status`/`stop`
/// client that receives the ordinary drive-letter spelling would otherwise
/// address two different pipes for the same directory.
///
/// This deliberately does *not* call `canonicalize` here. A child Store root
/// may not exist when its launcher derives the endpoint and does exist by the
/// time the service binds; allowing existence to change the identity splits
/// first startup across two pipes. Slash direction and case are normalized for
/// the same reason. Case-sensitive Unix paths remain untouched.
fn root_identity(root: &std::path::Path) -> String {
    #[cfg(windows)]
    {
        let value = root.to_string_lossy().replace('/', "\\");
        let ordinary = if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{rest}")
        } else {
            value.strip_prefix(r"\\?\").unwrap_or(&value).to_string()
        };
        ordinary.to_lowercase()
    }
    #[cfg(not(windows))]
    {
        root.to_string_lossy().into_owned()
    }
}

// ------------------------------------------------------------------- Windows

#[cfg(windows)]
mod platform {
    use super::{Endpoint, IpcError};

    /// `\\.\pipe\<service>.<segment>.<sid>` — but expressed as a *namespaced*
    /// name, so `interprocess` prepends `\\.\pipe\` itself and the same code
    /// reads correctly beside its Unix twin.
    pub fn endpoint(service: &str, segment: &str) -> Result<Endpoint, IpcError> {
        let sid = super::owner::current_user_sid()?;
        Ok(Endpoint {
            service: service.to_string(),
            segment: segment.to_string(),
            name: format!("{service}.{segment}.{sid}"),
            parent: None,
        })
    }
}

// ---------------------------------------------------------------------- Unix

#[cfg(unix)]
mod platform {
    use std::path::PathBuf;

    use super::{Endpoint, IpcError};

    pub fn endpoint(service: &str, segment: &str) -> Result<Endpoint, IpcError> {
        let dir = runtime_dir()?.join(service);
        let path = dir.join(format!("{segment}.sock"));
        Ok(Endpoint {
            service: service.to_string(),
            segment: segment.to_string(),
            name: path.to_string_lossy().into_owned(),
            parent: Some(dir),
        })
    }

    /// `$XDG_RUNTIME_DIR` when the platform sets one, otherwise a per-uid
    /// directory under `/tmp`.
    ///
    /// The uid is in the fallback's name because `/tmp` is shared: without it,
    /// the first user to start a service would own the directory and every
    /// other user's would fail to bind — which reads as a broken install rather
    /// than as the collision it is. macOS never sets `XDG_RUNTIME_DIR`, so the
    /// fallback is the normal path there, not an edge case.
    ///
    /// **`/tmp` literally, not `std::env::temp_dir()`, and that is the whole
    /// point on macOS** ([ADR-417]). `temp_dir()` answers `$TMPDIR`, which on
    /// macOS is a per-user `/var/folders/<2>/<26>/T/` — **49 bytes before this
    /// crate chooses anything**, against a `sun_path` of 104. The longest name
    /// the product can ask for, `koru-session-host` on the `internal-release`
    /// channel, reached **114** and could not bind; even the narrow 8-hex
    /// qualifier reached 106. From `/tmp` the same name is 70, which is 34
    /// bytes of headroom rather than minus ten.
    ///
    /// Linux has always landed here — `temp_dir()` *is* `/tmp` there whenever
    /// `$XDG_RUNTIME_DIR` is unset — so this makes the two platforms agree
    /// rather than giving macOS something of its own, and the uid-in-the-name
    /// convention above was written for exactly this shared directory.
    ///
    /// The cost is that `/tmp` is world-writable and `$TMPDIR` is not, so
    /// another local user can create `/tmp/koru-<uid>` first. That is a denial
    /// of service and not an escalation, and it is closed by
    /// `listener::prepare_parent`, which refuses a directory it does not own
    /// and says so, rather than binding inside it.
    ///
    /// [ADR-417]: ../../../docs/decisions/ADR-417-sunpath.md
    fn runtime_dir() -> Result<PathBuf, IpcError> {
        if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
            let dir = PathBuf::from(dir);
            if !dir.as_os_str().is_empty() {
                return Ok(dir);
            }
        }
        let uid = super::owner::current_uid();
        Ok(PathBuf::from("/tmp").join(format!("koru-{uid}")))
    }
}

// ------------------------------------------------------- who this process is

#[cfg(windows)]
pub(crate) mod owner {
    use std::ptr;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    use crate::errors::IpcError;

    /// The SID of the user this process runs as, as a string (`S-1-5-21-…`).
    ///
    /// This is the identity the whole design keys on, so it is read from the
    /// process token rather than from `%USERNAME%`: an environment variable is
    /// a display name, is changeable, and is not unique across a domain.
    pub fn current_user_sid() -> Result<String, IpcError> {
        // SAFETY: every call below is checked, every handle and allocation is
        // released on both the success and the failure path, and the buffer is
        // sized by the first `GetTokenInformation` before the second fills it.
        unsafe {
            let mut token: HANDLE = ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(fail("could not open this process's access token"));
            }
            let mut needed: u32 = 0;
            // The documented two-call pattern: the first is *expected* to fail
            // with ERROR_INSUFFICIENT_BUFFER and exists only to size `needed`.
            GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut needed);
            if needed == 0 {
                CloseHandle(token);
                return Err(fail("the access token reported no user information"));
            }
            let mut buffer = vec![0u8; needed as usize];
            let ok = GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            );
            CloseHandle(token);
            if ok == 0 {
                return Err(fail("could not read the user out of the access token"));
            }
            let user = &*(buffer.as_ptr() as *const TOKEN_USER);
            let mut wide: *mut u16 = ptr::null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut wide) == 0 {
                return Err(fail("could not render this user's SID as text"));
            }
            let mut len = 0usize;
            while *wide.add(len) != 0 {
                len += 1;
            }
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(wide, len));
            LocalFree(wide.cast());
            Ok(sid)
        }
    }

    fn fail(what: &str) -> IpcError {
        IpcError::PermissionDenied(format!(
            "{what}; without it there is no way to name an endpoint that belongs to one user"
        ))
    }

    /// A security descriptor granting all access to this user and nobody else.
    ///
    /// `D:P(A;;GA;;;<sid>)` in [security descriptor string format][sddl]:
    ///
    /// ```text
    /// D:      the DACL follows
    ///   P     protected — nothing is inherited from the namespace
    ///   A     allow
    ///   GA    generic all
    ///   <sid> exactly one trustee: the account this process runs as
    /// ```
    ///
    /// No SYSTEM entry, deliberately. SYSTEM does not need one — a process
    /// running as SYSTEM can take ownership of any object — so adding it would
    /// only widen the ACL by an entry that changes nothing.
    ///
    /// **Not behind a feature.** It used to be gated on the store's `server`
    /// feature, because only the daemon bound anything; both listeners here
    /// need it, and a descriptor that exists only in some builds is a
    /// permissions difference nobody would look for.
    ///
    /// [sddl]: https://learn.microsoft.com/en-us/windows/win32/secauthz/security-descriptor-string-format
    pub fn only_this_user()
    -> std::io::Result<interprocess::os::windows::security_descriptor::SecurityDescriptor> {
        use interprocess::os::windows::security_descriptor::SecurityDescriptor;

        let sid = current_user_sid().map_err(|e| std::io::Error::other(e.to_string()))?;
        let sddl = widestring::U16CString::from_str(format!("D:P(A;;GA;;;{sid})"))
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
        SecurityDescriptor::deserialize(&sddl)
    }
}

#[cfg(unix)]
pub(crate) mod owner {
    /// The effective uid. Used only to keep two users' sockets apart in a
    /// shared temp directory — the actual restriction is the directory's mode.
    pub fn current_uid() -> u32 {
        // SAFETY: `geteuid` takes no arguments, cannot fail, and returns a
        // plain integer. It is unsafe only because it is `extern "C"`.
        unsafe { libc::geteuid() }
    }
}

#[cfg(test)]
mod tests;

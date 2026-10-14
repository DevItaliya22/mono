// SPDX-License-Identifier: MPL-2.0

//! What version of macOS this is, read once and by one function.
//!
//! Beside [`system_locale`](crate::system_locale) and
//! [`system_theme`](crate::system_theme) for the same reason all three are
//! here: a platform fact that both Koru and Agent need, that winit cannot
//! answer, and that names no node, no workspace and no document.
//!
//! **Why this module is macOS-only, when its two neighbours are not.** Two
//! callers had grown a reader each — `koru-agent`'s `build_stamp`, which
//! stamps the OS onto the build record a device advertises, and
//! `koru-workspace-host`'s `environment`, which reports it as one of the machine's
//! dimensions — and neither crate may depend on the other. The Windows halves
//! of those two are *also* a duplicated `RtlGetVersion`, and they are
//! deliberately left where they are: this change was written and compiled on a
//! Mac with no Windows toolchain installed, and moving working `unsafe` code
//! across a crate boundary you cannot compile is how a platform gets broken by
//! a tidy-up. See ADR-173 and TECHDEBT 43.
//!
//! **The numbers, never a sentence.** `ProcessInfo`'s
//! `operatingSystemVersionString` is documented by Apple as *"not appropriate
//! for parsing"*, and `operatingSystemVersion` gives the three numbers but not
//! the Darwin build. `sysctl` gives both as plain strings, spawns no process
//! and parses no plist, which is what makes it usable on a startup path.

/// The two facts About This Mac shows: `26.0.1`, and the Darwin build `25A362`.
///
/// `product` is dotted digits and nothing else, because that is what the
/// `app.environment` event's `os_version` is declared to be; the display
/// forms ("macOS 26.0") belong to whoever is drawing, not here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemVersion {
    pub product: String,
    pub build: Option<String>,
}

/// `None` when `sysctl` will not answer — which should not happen on a Mac,
/// and is still not a reason to invent a version.
pub fn system_version() -> Option<SystemVersion> {
    let product = sysctl("kern.osproductversion")?;
    Some(SystemVersion {
        product,
        build: sysctl("kern.osversion"),
    })
}

/// A `sysctl` whose value is a string. The buffer is fixed because both keys
/// answer in well under sixteen bytes; a truncation would be reported as a
/// short version rather than an error, so the buffer is generous rather than
/// exact.
fn sysctl(name: &str) -> Option<String> {
    let name = std::ffi::CString::new(name).ok()?;
    let mut buffer = [0u8; 64];
    let mut len = buffer.len();
    // SAFETY: `name` is a valid C string, `buffer` is writable for `len`
    // bytes, and `sysctlbyname` writes at most `len` and updates it.
    let status = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            buffer.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 {
        return None;
    }
    let text = String::from_utf8_lossy(&buffer[..len]);
    let text = text.trim_end_matches('\0').trim();
    (!text.is_empty()).then(|| text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run on the machine doing the reading, which is the only place this
    /// can be checked: the product version is dotted digits, and the Darwin
    /// build is present and is not the product version again.
    #[test]
    fn this_mac_answers_with_numbers_and_a_build() {
        let version = system_version().expect("sysctl answers on a Mac");
        assert!(
            version
                .product
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())),
            "{}",
            version.product
        );
        let build = version.build.expect("kern.osversion answers on a Mac");
        assert!(!build.is_empty());
        assert_ne!(build, version.product);
    }
}

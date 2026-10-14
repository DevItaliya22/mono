// SPDX-License-Identifier: MPL-2.0

//! The Windows half: creating a small tree of keys under `HKEY_CURRENT_USER`.
//!
//! `koru-autostart` does the same kind of work and cannot be reused: it only
//! ever *opens* a key that already exists, because `Run` always does. None of
//! `koru`, `DefaultIcon`, `shell`, `open` or `command` exists until we make it,
//! so this needs `RegCreateKeyExW` — which appears nowhere else in the tree.

use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};

pub const SUPPORTED: bool = true;

pub fn unsupported() -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "handling koru:// links is currently supported on Windows",
    ))
}

/// A NUL-terminated wide string. Windows needs the terminator and `OsStr` does
/// not carry one.
fn wide(value: &str) -> Vec<u16> {
    OsStr::new(value).encode_wide().chain(Some(0)).collect()
}

/// Write every entry, creating each key and all of its missing parents.
///
/// `RegCreateKeyExW` makes every missing level of the path in one call, so this
/// is four calls rather than a recursive walk.
pub fn write_all(root: &str, entries: &[super::Entry]) -> io::Result<()> {
    for (subkey, value, data) in entries {
        let path = wide(&format!("{root}\\{subkey}"));
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: `path` is NUL-terminated and outlives the call; `key` is
        // written only on success, and every other pointer is the documented
        // "no class, no disposition" null.
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                std::ptr::null_mut(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        // An empty name is the key's *default* value. `wide("")` rather than a
        // null pointer, so both cases take one code path.
        let name = wide(value);
        let bytes = wide(data);
        // SAFETY: `key` is open, both buffers are NUL-terminated and live
        // across the call, and the length is in bytes even though the data is
        // UTF-16.
        let status = unsafe {
            RegSetValueExW(
                key,
                name.as_ptr(),
                0,
                REG_SZ,
                bytes.as_ptr().cast(),
                (bytes.len() * size_of::<u16>()) as u32,
            )
        };
        unsafe { RegCloseKey(key) };
        if status != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
    }
    Ok(())
}

/// Remove a key and everything under it. Absent is success — the caller asked
/// for it to be gone, and it is.
pub fn delete_tree(root: &str, subkey: &str) -> io::Result<()> {
    let parent = wide(root);
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: `parent` is NUL-terminated; `key` is written only on success.
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            parent.as_ptr(),
            0,
            KEY_SET_VALUE,
            &mut key,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let target = wide(subkey);
    // SAFETY: `key` is open and `target` is NUL-terminated.
    let status = unsafe { RegDeleteTreeW(key, target.as_ptr()) };
    unsafe { RegCloseKey(key) };
    if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status as i32))
    }
}

/// One value's contents, or `None` for anything at all going wrong.
///
/// Total on purpose, as in `koru-autostart`: no key, no value, the wrong type
/// and unreadable bytes are one answer to the only question being asked.
pub fn read(root: &str, subkey: &str, value: &str) -> Option<String> {
    let path = wide(&format!("{root}\\{subkey}"));
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: `path` is NUL-terminated; `key` is written only on success.
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            path.as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &mut key,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let name = wide(value);
    let mut kind = 0u32;
    let mut bytes = 0u32;
    // The documented two-call pattern: the first sizes the buffer.
    // SAFETY: `key` is open and `name` is NUL-terminated; null data and length
    // pointers are what ask for the size.
    let status = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut kind,
            std::ptr::null_mut(),
            &mut bytes,
        )
    };
    if status != ERROR_SUCCESS || kind != REG_SZ || bytes == 0 {
        unsafe { RegCloseKey(key) };
        return None;
    }
    let mut buffer = vec![0u16; bytes as usize / size_of::<u16>() + 1];
    let mut length = bytes;
    // SAFETY: the buffer is at least `bytes` long, which is what the call above
    // said is needed.
    let status = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut kind,
            buffer.as_mut_ptr().cast(),
            &mut length,
        )
    };
    unsafe { RegCloseKey(key) };
    if status != ERROR_SUCCESS {
        return None;
    }
    // The stored length includes the terminator, and a trailing NUL would make
    // a string compare unequal to everything it looks identical to. This is the
    // bug `koru-autostart`'s own reader already had to fix once.
    let chars = length as usize / size_of::<u16>();
    let text = &buffer[..chars.min(buffer.len())];
    let text = match text.iter().position(|c| *c == 0) {
        Some(end) => &text[..end],
        None => text,
    };
    Some(String::from_utf16_lossy(text))
}

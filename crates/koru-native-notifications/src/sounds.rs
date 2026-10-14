// SPDX-License-Identifier: MPL-2.0

//! The stable tone inventory shared by Settings, packaging, and native adapters.
//!
//! Filenames are explicit rather than discovered. A half-copied asset folder
//! must fail packaging instead of silently shrinking the choice list.

#[derive(Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum BundledTone {
    KoruDefault,
    Soft,
    Chime,
    Pulse,
    Bright,
    Calm,
    Drop,
    Knock,
    Signal,
    Urgent,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SoundAsset {
    pub tone: BundledTone,
    /// Adapter/asset key. It is never a localized label; the persisted
    /// reminder model owns its own `ToneId` mapping rather than importing this
    /// mechanism type.
    pub key: &'static str,
    /// Source and package filename beneath `assets/reminders/`.
    pub file: &'static str,
}

pub const BUNDLED_TONES: &[SoundAsset] = &[
    SoundAsset {
        tone: BundledTone::KoruDefault,
        key: "koru-default",
        file: "01_Koru_Default.wav",
    },
    SoundAsset {
        tone: BundledTone::Soft,
        key: "soft",
        file: "02_Soft.wav",
    },
    SoundAsset {
        tone: BundledTone::Chime,
        key: "chime",
        file: "03_Chime.wav",
    },
    SoundAsset {
        tone: BundledTone::Pulse,
        key: "pulse",
        file: "04_Pulse.wav",
    },
    SoundAsset {
        tone: BundledTone::Bright,
        key: "bright",
        file: "05_Bright.wav",
    },
    SoundAsset {
        tone: BundledTone::Calm,
        key: "calm",
        file: "06_Calm.wav",
    },
    SoundAsset {
        tone: BundledTone::Drop,
        key: "drop",
        file: "07_Drop.wav",
    },
    SoundAsset {
        tone: BundledTone::Knock,
        key: "knock",
        file: "08_Knock.wav",
    },
    SoundAsset {
        tone: BundledTone::Signal,
        key: "signal",
        file: "09_Signal.wav",
    },
    SoundAsset {
        tone: BundledTone::Urgent,
        key: "urgent",
        file: "10_Urgent.wav",
    },
];

/// Runtime lookup order shared with fonts: an explicit override and packaged
/// layouts must win over absolute source paths compiled on the build machine.
pub fn sound_asset_roots() -> Vec<std::path::PathBuf> {
    sound_asset_roots_for(
        std::env::current_exe().ok().as_deref(),
        std::env::var_os("KORU_ASSETS").as_deref(),
    )
}

pub(crate) fn sound_asset_roots_for(
    executable: Option<&std::path::Path>,
    override_root: Option<&std::ffi::OsStr>,
) -> Vec<std::path::PathBuf> {
    let mut roots = Vec::new();
    if let Some(root) = override_root {
        roots.push(std::path::PathBuf::from(root).join("reminders"));
    }
    if let Some(exe) = executable
        && let Some(dir) = exe.parent()
    {
        roots.push(dir.join("assets/reminders"));
        if let Some(contents) = dir.parent() {
            roots.push(contents.join("Resources/assets/reminders"));
        }
    }
    roots.push(std::path::PathBuf::from("assets/reminders"));
    roots.push(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/reminders"));
    roots
}

pub fn read_sound_asset(tone: BundledTone) -> Option<Vec<u8>> {
    let file = BUNDLED_TONES.iter().find(|asset| asset.tone == tone)?.file;
    sound_asset_roots()
        .into_iter()
        .find_map(|root| std::fs::read(root.join(file)).ok())
}

#[cfg(windows)]
pub(crate) fn sound_asset(tone: BundledTone) -> Option<&'static SoundAsset> {
    BUNDLED_TONES.iter().find(|asset| asset.tone == tone)
}

#[cfg(windows)]
pub(crate) fn packaged_windows_sounds_available() -> bool {
    let Some(root) = std::env::current_exe().ok().and_then(|exe| {
        exe.parent()
            .map(|directory| directory.join("assets/reminders"))
    }) else {
        return false;
    };
    BUNDLED_TONES
        .iter()
        .all(|asset| root.join(asset.file).is_file())
}

#[cfg(target_os = "macos")]
pub(crate) fn packaged_macos_sounds_available_from(executable: Option<&std::path::Path>) -> bool {
    let Some(root) = executable.and_then(|exe| {
        exe.parent()?
            .parent()
            .map(|contents| contents.join("Resources/assets/reminders"))
    }) else {
        return false;
    };
    BUNDLED_TONES
        .iter()
        .all(|asset| root.join(asset.file).is_file())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WavError {
    Truncated,
    NotWave,
    MissingFormat,
    MissingData,
    UnsupportedEncoding,
    InvalidRate,
    TooLong,
}

/// Validate the subset accepted by both Windows and macOS notification sound
/// services: PCM WAV, with a bounded duration.
///
/// Parsing chunks rather than assuming `fmt` then `data` also accepts metadata
/// chunks added by an audio editor. It returns the duration in milliseconds so
/// packaging diagnostics can name the evidence without decoding audio.
pub fn validate_notification_wav(bytes: &[u8]) -> Result<u64, WavError> {
    if bytes.len() < 12 {
        return Err(WavError::Truncated);
    }
    if &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(WavError::NotWave);
    }

    let mut format = None;
    let mut data_len = None;
    let mut at = 12usize;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let start = at + 8;
        let end = start.checked_add(len).ok_or(WavError::Truncated)?;
        if end > bytes.len() {
            return Err(WavError::Truncated);
        }
        if id == b"fmt " {
            if len < 16 {
                return Err(WavError::Truncated);
            }
            let encoding = u16::from_le_bytes(bytes[start..start + 2].try_into().unwrap());
            let byte_rate = u32::from_le_bytes(bytes[start + 8..start + 12].try_into().unwrap());
            format = Some((encoding, byte_rate));
        } else if id == b"data" {
            data_len = Some(len as u64);
        }
        at = end + (len & 1);
    }

    let (encoding, byte_rate) = format.ok_or(WavError::MissingFormat)?;
    if encoding != 1 {
        return Err(WavError::UnsupportedEncoding);
    }
    if byte_rate == 0 {
        return Err(WavError::InvalidRate);
    }
    let data_len = data_len.ok_or(WavError::MissingData)?;
    let duration_ms = data_len.saturating_mul(1000) / u64::from(byte_rate);
    if duration_ms >= 30_000 {
        return Err(WavError::TooLong);
    }
    Ok(duration_ms)
}

/// **Play a tone once, now** — Settings' preview beside *Default tone*.
///
/// `None` is the operating system's own notification sound. Answers whether
/// the request was queued, not whether playback succeeded. The single worker
/// owns filesystem/native work; at most one latest request waits behind it.
/// A second preview stops the first. Unsupported platforms return `false`.
pub fn preview(tone: Option<BundledTone>) -> bool {
    #[cfg(windows)]
    {
        use std::sync::{Arc, Condvar, Mutex, OnceLock};
        type Queue = (Mutex<Option<Option<BundledTone>>>, Condvar);
        static QUEUE: OnceLock<Option<Arc<Queue>>> = OnceLock::new();
        let Some(queue) = QUEUE.get_or_init(|| {
            let queue = Arc::new((Mutex::new(None), Condvar::new()));
            let worker = Arc::clone(&queue);
            std::thread::Builder::new()
                .name("koru-tone-preview".into())
                .spawn(move || {
                    loop {
                        let tone = {
                            let (pending, wake) = &*worker;
                            let mut pending = pending.lock().unwrap_or_else(|e| e.into_inner());
                            while pending.is_none() {
                                pending = wake.wait(pending).unwrap_or_else(|e| e.into_inner());
                            }
                            pending.take().unwrap()
                        };
                        let _ = platform_preview(tone);
                    }
                })
                .ok()
                .map(|_| queue)
        }) else {
            return false;
        };
        *queue.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(tone);
        queue.1.notify_one();
        true
    }
    #[cfg(not(windows))]
    {
        let _ = tone;
        false
    }
}

#[cfg(windows)]
fn platform_preview(tone: Option<BundledTone>) -> bool {
    use std::sync::Mutex;
    use windows::Win32::Media::Audio::{
        PlaySoundW, SND_ALIAS, SND_ASYNC, SND_MEMORY, SND_NODEFAULT,
    };
    use windows::core::{PCWSTR, w};

    /// `SND_MEMORY | SND_ASYNC` plays from the caller's buffer after the call
    /// returns, so the bytes must outlive the playback: they live here until
    /// the next preview has stopped this one.
    static PLAYING: Mutex<Option<Vec<u8>>> = Mutex::new(None);

    let mut playing = PLAYING.lock().unwrap_or_else(|held| held.into_inner());
    // Stop whatever is sounding before its buffer is replaced.
    unsafe {
        let _ = PlaySoundW(PCWSTR::null(), None, SND_ASYNC);
    }
    match tone {
        None => {
            *playing = None;
            unsafe {
                PlaySoundW(
                    w!("Notification.Default"),
                    None,
                    SND_ALIAS | SND_ASYNC | SND_NODEFAULT,
                )
                .as_bool()
            }
        }
        Some(tone) => {
            let Some(bytes) = read_sound_asset(tone) else {
                return false;
            };
            let bytes = playing.insert(bytes);
            unsafe {
                PlaySoundW(
                    PCWSTR(bytes.as_ptr().cast()),
                    None,
                    SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
                )
                .as_bool()
            }
        }
    }
}

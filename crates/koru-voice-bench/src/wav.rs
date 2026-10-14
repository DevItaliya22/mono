// SPDX-License-Identifier: MPL-2.0

//! **16 kHz, mono, 16-bit PCM WAV, and deliberately no decoder.**
//!
//! The protocol chose the fixture format so the harness needs none, and the
//! corpus acquisition step converts everything else through one `ffmpeg`
//! command — so every file this crate ever opens is already in this shape. A
//! decoder here would be a second opinion about the sample rate that
//! `koru_voice::SAMPLE_RATE` already declares, and an engine handed 48 kHz audio
//! it believes is 16 kHz transcribes fluent nonsense at a third of the speed
//! rather than failing.
//!
//! So this reader is strict about exactly the properties that would be silently
//! wrong: **format, channel count, bit depth and sample rate are refused rather
//! than converted.** A benchmark that resampled would be measuring its own
//! resampler in every latency figure.
//!
//! ## The RIFF details that are not guessable
//!
//! - **Chunks are walked, not assumed.** A `LIST`/`INFO` chunk between `fmt `
//!   and `data` is legal and common — `ffmpeg` writes one — so a reader that
//!   seeks to byte 44 gets metadata and calls it audio.
//! - **Chunk bodies are padded to even lengths and the pad byte is not counted
//!   in the size field.** Skipping `size` bytes rather than `size + size % 2`
//!   leaves the cursor one byte inside the next chunk id, and every subsequent
//!   id reads as garbage.
//! - **`WAVE_FORMAT_EXTENSIBLE` (0xFFFE) is a legal spelling of PCM.** Refusing
//!   it outright would reject files `ffmpeg` produces for some inputs, so it is
//!   accepted when its bit depth and channel count are right.

use std::path::Path;

use koru_voice::SAMPLE_RATE;

/// What a WAV file could not be read as.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct WavError(pub String);

impl std::fmt::Display for WavError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WavError {}

fn bad(message: impl Into<String>) -> WavError {
    WavError(message.into())
}

const FORMAT_PCM: u16 = 1;
const FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// Read one file into mono `f32` at [`SAMPLE_RATE`].
pub fn read_wav(path: &Path) -> Result<Vec<f32>, WavError> {
    let bytes =
        std::fs::read(path).map_err(|e| bad(format!("could not read {}: {e}", path.display())))?;
    parse_wav(&bytes).map_err(|e| bad(format!("{}: {e}", path.display())))
}

/// The parser, separated so a test can hand it bytes rather than a file.
pub fn parse_wav(bytes: &[u8]) -> Result<Vec<f32>, WavError> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(bad("not a RIFF/WAVE file"));
    }

    let mut cursor = 12usize;
    let mut format: Option<(u16, u16, u32, u16)> = None;
    let mut data: Option<&[u8]> = None;

    while cursor + 8 <= bytes.len() {
        let id = &bytes[cursor..cursor + 4];
        let size = u32::from_le_bytes([
            bytes[cursor + 4],
            bytes[cursor + 5],
            bytes[cursor + 6],
            bytes[cursor + 7],
        ]) as usize;
        let body_start = cursor + 8;
        let body_end = body_start
            .checked_add(size)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| bad("a chunk declares more bytes than the file holds"))?;
        let body = &bytes[body_start..body_end];

        if id == b"fmt " {
            if body.len() < 16 {
                return Err(bad("the fmt chunk is shorter than 16 bytes"));
            }
            let tag = u16::from_le_bytes([body[0], body[1]]);
            let channels = u16::from_le_bytes([body[2], body[3]]);
            let rate = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
            let bits = u16::from_le_bytes([body[14], body[15]]);
            format = Some((tag, channels, rate, bits));
        } else if id == b"data" {
            data = Some(body);
        }

        // The pad byte. See the module note — this is the line whose absence
        // makes every later chunk id read as garbage.
        cursor = body_end + (size % 2);
    }

    let (tag, channels, rate, bits) = format.ok_or_else(|| bad("there is no fmt chunk"))?;
    let data = data.ok_or_else(|| bad("there is no data chunk"))?;

    if tag != FORMAT_PCM && tag != FORMAT_EXTENSIBLE {
        return Err(bad(format!(
            "format tag {tag} is not PCM; the corpus step converts everything to \
             16 kHz mono s16 and this reader deliberately does not"
        )));
    }
    if channels != 1 {
        return Err(bad(format!("{channels} channels; the protocol says mono")));
    }
    if bits != 16 {
        return Err(bad(format!("{bits}-bit samples; the protocol says 16")));
    }
    if rate != SAMPLE_RATE {
        return Err(bad(format!(
            "{rate} Hz; koru_voice::SAMPLE_RATE is {SAMPLE_RATE} and resampling \
             here would put this harness's resampler in every latency figure"
        )));
    }
    if data.len() % 2 != 0 {
        return Err(bad("the data chunk holds half a sample"));
    }

    Ok(data
        .chunks_exact(2)
        .map(|pair| {
            let raw = i16::from_le_bytes([pair[0], pair[1]]);
            // Divide by 32768 rather than 32767: it makes the mapping exact for
            // every negative value and puts the range in `-1.0..1.0`, which is
            // what every engine here expects. Scaling by 32767 makes -32768
            // land at -1.00003 and clip.
            f32::from(raw) / 32_768.0
        })
        .collect())
}

/// A file's duration, from its sample count.
pub fn duration(samples: &[f32]) -> std::time::Duration {
    std::time::Duration::from_secs_f64(samples.len() as f64 / f64::from(SAMPLE_RATE))
}

#[cfg(test)]
mod tests;

// SPDX-License-Identifier: MPL-2.0

//! **The one test in this tree where audio becomes text.**
//!
//! Everything else about the worker is checked against a scripted engine,
//! because a real recogniser makes every assertion depend on a model, a machine
//! and a person's voice. This file is the exception, and it exists because the
//! whole feature is worth nothing until somebody has watched a waveform come
//! out the other end as words.
//!
//! It drives the *worker*, not the adapter: a recording goes into
//! [`CaptureSource::Recorded`], through the ring, through
//! `Session`'s authorisation, into `SherpaEngineHost`'s engine, and the
//! assertion is on the `Final` event a client would receive. No microphone and
//! no socket — the two things a test may not need — and every other link in the
//! chain is real.
//!
//! ## It needs half a gigabyte of weights, so it says when it has none
//!
//! `KORU_VOICE_SHERPA_MODEL` names a provisioned sherpa model directory. It is
//! gitignored content, so on a fresh clone the variable is unset and this
//! suite prints why it did not run and passes. **That is the one thing a test
//! like this must not do quietly**: a test that silently passes because a file
//! is absent is worse than no test, because it will be reported as evidence.
//! The `eprintln!` is the mitigation, and the honest statement is in the
//! report: this is a manual verification with a checked-in driver, not a gate.
//!
//! ## What a passing run does and does not prove
//!
//! The clips are the model publisher's own `test_wavs/`, which are almost
//! certainly what the package was demonstrated on. So a good transcript here is
//! evidence that **the pipe carries words** — capture source, ring, resampler,
//! session authorisation, adapter, event stream — and is *not* evidence about
//! accuracy. It is not a benchmark. Plan 0.4 is still owed and
//! `docs/features/voice/plans/ENGINE_BENCHMARK.md` is what it answers.
//!
//! ## The clips are not all 16 kHz, which is why there is a resampler here
//!
//! `ar.wav` is 16 kHz, `de.wav` is 22.05 kHz and `ja.wav` is 44.1 kHz.
//! `koru_voice_bench::read_wav` deliberately **refuses** anything that is not
//! already at [`koru_voice::SAMPLE_RATE`] — a benchmark that resampled would be
//! measuring its own resampler — so this file reads the header itself and runs
//! the samples through [`crate::resample::Resampler`], which is the same code
//! the microphone path uses. Handing 44.1 kHz to an engine that believes it is
//! 16 kHz produces fluent nonsense at a third of the speed rather than an
//! error, which is exactly the failure this note exists to stop somebody
//! reintroducing.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use koru_voice::{Event, ModelId, Requested, SessionId, VoiceLanguage};
use koru_voice_models::{
    InstalledModel, LicenceRef, ModelDescriptor, ModelTier, ModelVersion, Sha256Digest,
};

use super::*;
use crate::engine::sherpa::SherpaEngineHost;
use crate::engine::whisper::WhisperEngineHost;
use crate::models::MemoryModelStore;

/// **Not a semantic budget.** Loading a 657 MB int8 encoder off a cold disk and
/// decoding twenty seconds of speech on four threads is real work, and the
/// harness's own `PATIENCE` is fifteen seconds because everything it waits for
/// is immediate. Five minutes is chosen so a slow machine fails this test for
/// being wrong rather than for being slow.
const LOADING: Duration = Duration::from_secs(300);

const MODEL: &str = "nemotron-3.5-streaming-0.6b-160ms";
const VERSION: &str = "int8-2026-06-11";
const WHISPER_MODEL: &str = "whisper-base";
const WHISPER_VERSION: &str = "q5_1";
const WHISPER_DIGEST: &str = "c48fb5a2a1499e8d90cefab6e5f36ba6bc71e291354c1f7a781e2930ec4f4493";

/// The provisioned model directory, or `None`.
fn model_dir() -> Option<PathBuf> {
    let named = std::env::var_os("KORU_VOICE_SHERPA_MODEL")?;
    let dir = PathBuf::from(named);
    dir.is_dir().then_some(dir)
}

/// The provisioned Compact model directory, or `None`.
fn whisper_model_dir() -> Option<PathBuf> {
    let named = std::env::var_os("KORU_VOICE_WHISPER_MODEL")?;
    let dir = PathBuf::from(named);
    dir.is_dir().then_some(dir)
}

/// A descriptor for the provisioned package, declaring the eight languages the
/// published catalog row declares.
fn store(root: &Path) -> Arc<MemoryModelStore> {
    Arc::new(MemoryModelStore::new(vec![InstalledModel {
        root: root.to_path_buf(),
        descriptor: ModelDescriptor {
            schema_version: koru_voice_models::DESCRIPTOR_SCHEMA_VERSION,
            id: ModelId::parse(MODEL).unwrap(),
            version: ModelVersion::parse(VERSION).unwrap(),
            engine: koru_voice_sherpa::engine_id(),
            tier: ModelTier::HighAccuracy,
            languages: VoiceLanguage::ALL.to_vec(),
            size: 475_273_363,
            digest: Sha256Digest::parse(crate::fixtures::DIGEST).unwrap(),
            working_set_bytes: 2 * 1024 * 1024 * 1024,
            licence: LicenceRef::parse("OpenMDW-1.1").unwrap(),
            attribution: "NVIDIA Corporation; ONNX export by k2-fsa/sherpa-onnx".to_string(),
        },
    }]))
}

/// A descriptor for the provisioned Compact package.
fn whisper_store(root: &Path) -> Arc<MemoryModelStore> {
    Arc::new(MemoryModelStore::new(vec![InstalledModel {
        root: root.to_path_buf(),
        descriptor: ModelDescriptor {
            schema_version: koru_voice_models::DESCRIPTOR_SCHEMA_VERSION,
            id: ModelId::parse(WHISPER_MODEL).unwrap(),
            version: ModelVersion::parse(WHISPER_VERSION).unwrap(),
            engine: koru_voice_whisper::engine_id(),
            tier: ModelTier::Compact,
            languages: VoiceLanguage::ALL.to_vec(),
            size: 56_703_003,
            digest: Sha256Digest::parse(WHISPER_DIGEST).unwrap(),
            working_set_bytes: 512 * 1024 * 1024,
            licence: LicenceRef::parse("MIT").unwrap(),
            attribution: "OpenAI; GGML quantisation by whisper.cpp".to_string(),
        },
    }]))
}

/// **A 16-bit PCM WAV, walked chunk by chunk, resampled to 16 kHz mono.**
///
/// Deliberately minimal and deliberately not a decoder: the publisher's clips
/// are PCM, and anything else is a refusal rather than a conversion. The chunk
/// walk is not optional — a `LIST`/`INFO` chunk between `fmt ` and `data` is
/// legal and common, so a reader that seeks to byte 44 reads metadata and calls
/// it audio.
fn read_16k_mono(path: &Path) -> Result<Vec<f32>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let bad = |why: &str| Err(format!("{}: {why}", path.display()));
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return bad("not a RIFF/WAVE file");
    }
    let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let u32_at =
        |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);

    let mut cursor = 12usize;
    let mut format: Option<(u16, u16, u32)> = None;
    let mut data: Option<&[u8]> = None;
    while cursor + 8 <= bytes.len() {
        let id = &bytes[cursor..cursor + 4];
        let size = u32_at(cursor + 4) as usize;
        let body = cursor + 8;
        if body + size > bytes.len() {
            break;
        }
        if id == b"fmt " && size >= 16 {
            format = Some((u16_at(body), u16_at(body + 2), u32_at(body + 4)));
        } else if id == b"data" {
            data = Some(&bytes[body..body + size]);
        }
        // The pad byte is not counted in the size field; skipping `size` alone
        // leaves the cursor one byte inside the next chunk id.
        cursor = body + size + (size % 2);
    }

    let Some((tag, channels, rate)) = format else {
        return bad("has no fmt chunk");
    };
    let Some(data) = data else {
        return bad("has no data chunk");
    };
    if tag != 1 {
        return Err(format!("{}: format {tag} is not PCM", path.display()));
    }
    let channels = channels.max(1) as usize;

    // 16-bit signed, interleaved. Downmixed by the same average the device path
    // uses, then resampled by the same resampler.
    let frames: Vec<f32> = data
        .chunks_exact(2 * channels)
        .map(|frame| {
            let sum: f32 = frame
                .chunks_exact(2)
                .map(|s| i16::from_le_bytes([s[0], s[1]]) as f32 / 32_768.0)
                .sum();
            sum / channels as f32
        })
        .collect();

    let mut resampler = crate::resample::Resampler::new(rate, koru_voice::SAMPLE_RATE);
    let mut out = Vec::with_capacity(frames.len());
    for sample in frames {
        resampler.push(sample, |emitted| out.push(emitted));
    }
    Ok(out)
}

/// Wait for a `Final`, collecting everything on the way.
///
/// Its own loop rather than the harness's `wait_for`, because that one is
/// bounded by `PATIENCE` — see [`LOADING`].
fn wait_for_final(drive: &Driver, seen: &mut Vec<Event>) -> Option<String> {
    let deadline = Instant::now() + LOADING;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        match drive.events.recv_timeout(left) {
            Ok(Outgoing::Event(event)) => {
                if let Event::Final { text, .. } = &event {
                    let text = text.as_str().to_string();
                    seen.push(event);
                    return Some(text);
                }
                seen.push(event);
            }
            Ok(Outgoing::Status(_)) => {}
            Err(_) => return None,
        }
    }
}

/// One clip: the file, the language to ask for, and the Unicode block a correct
/// transcript has to land in.
struct Clip {
    file: &'static str,
    language: VoiceLanguage,
    /// **A script check, not a string comparison.** The reference text is not
    /// shipped with the package and inventing one would be asserting against
    /// something nobody verified; what *is* checkable without it is that
    /// Arabic speech produced Arabic letters rather than empty text or
    /// romanisation, which is precisely the claim "the pipe carries words"
    /// makes.
    script: fn(char) -> bool,
    script_name: &'static str,
}

fn is_arabic(c: char) -> bool {
    ('\u{0600}'..='\u{06FF}').contains(&c) || ('\u{0750}'..='\u{077F}').contains(&c)
}

fn is_japanese(c: char) -> bool {
    // Hiragana, katakana, or a CJK ideograph: a Japanese transcript uses all
    // three and a romanised one uses none.
    ('\u{3040}'..='\u{30FF}').contains(&c) || ('\u{4E00}'..='\u{9FFF}').contains(&c)
}

fn is_latin_letter(c: char) -> bool {
    c.is_ascii_alphabetic() || ('\u{00C0}'..='\u{024F}').contains(&c)
}

const CLIPS: [Clip; 4] = [
    Clip {
        file: "ar.wav",
        language: VoiceLanguage::Ar,
        script: is_arabic,
        script_name: "Arabic",
    },
    Clip {
        file: "ja.wav",
        language: VoiceLanguage::Ja,
        script: is_japanese,
        script_name: "Japanese",
    },
    Clip {
        file: "de.wav",
        language: VoiceLanguage::De,
        script: is_latin_letter,
        script_name: "Latin",
    },
    Clip {
        file: "fr.wav",
        language: VoiceLanguage::Fr,
        script: is_latin_letter,
        script_name: "Latin",
    },
];

/// **The proof.** A recorded utterance reaches a client as a transcript in the
/// script the person was speaking.
#[test]
fn recorded_speech_becomes_text() {
    let Some(dir) = model_dir() else {
        eprintln!(
            "SKIPPED recorded_speech_becomes_text: KORU_VOICE_SHERPA_MODEL names no directory, \
             so there are no weights to load. This suite proves nothing today."
        );
        return;
    };
    let wavs = dir.join("test_wavs");
    if !wavs.is_dir() {
        eprintln!(
            "SKIPPED recorded_speech_becomes_text: {} has no test_wavs/ beside the graphs.",
            dir.display()
        );
        return;
    }

    for clip in CLIPS {
        let path = wavs.join(clip.file);
        if !path.is_file() {
            eprintln!("SKIPPED {}: it is not in {}", clip.file, wavs.display());
            continue;
        }
        let samples = read_16k_mono(&path).expect("the publisher's clips are 16-bit PCM");
        assert!(
            samples.len() > koru_voice::SAMPLE_RATE as usize / 2,
            "{} decoded to {} samples, which is not speech",
            clip.file,
            samples.len()
        );

        let store = store(&dir);
        let mut drive = Driver::new(WorkerConfig {
            store: store.clone(),
            engine: Box::new(SherpaEngineHost::new()),
            capture: CaptureSource::Recorded(Arc::new(samples.clone())),
            // The whole clip has to fit: the recorded source pushes it into the
            // ring in one go at `StartCapture`, and the default two seconds
            // would drop everything but the tail.
            ring_capacity: samples.len() + koru_voice::SAMPLE_RATE as usize,
            idle: crate::idle::IdlePolicy {
                unload_after: Duration::ZERO,
                exit_after: Duration::ZERO,
            },
            supervisor_launch_id: None,
        });

        // The model is named rather than chosen, so this test does not also
        // depend on how a tier resolves. The language deliberately is *not*
        // named: `Auto` is the product default, and this real-model proof is
        // what prevents an unavailable per-stream override from being mistaken
        // for an inability to detect the language.
        let session = SessionId::new();
        let generation = drive.next_generation;
        drive.next_generation += 1;
        drive.send(Request::Begin {
            session,
            generation,
            language: Requested::Auto,
            tier: koru_voice::ModelTier::HighAccuracy,
            model: Some(ModelId::parse(MODEL).unwrap()),
            input: koru_voice::InputChoice::SystemDefault,
        });

        let mut seen = Vec::new();
        // `listening` is bounded by the harness's `PATIENCE`, and reaching
        // `Listening` is what costs a model load — so this waits on its own
        // clock too.
        let began = Instant::now();
        loop {
            match drive.events.recv_timeout(LOADING) {
                Ok(Outgoing::Event(event)) => {
                    let listening =
                        matches!(&event, Event::State { state, .. } if *state == State::Listening);
                    let refused = matches!(&event, Event::Ended { .. });
                    seen.push(event);
                    if refused {
                        panic!("the session ended before it listened: {seen:?}");
                    }
                    if listening {
                        break;
                    }
                }
                Ok(Outgoing::Status(_)) => {}
                Err(_) => panic!("the worker never began listening: {seen:?}"),
            }
        }
        eprintln!(
            "{}: the model loaded and capture started in {} ms",
            clip.file,
            began.elapsed().as_millis()
        );

        // **`Finish` may be sent immediately, and that is not a race.** The
        // inference loop is `pump(); recv_timeout(..)` in that order, and
        // `start_capture` pushed the whole recording into the ring before the
        // `Listening` this test just read — so the pump that drains the ring
        // into the recogniser runs *before* this command is taken off the
        // channel. The belt and braces is in the worker itself: the `Finish`
        // arm calls `pump_audio` again before stepping the session, precisely
        // because audio left in the ring is the last syllable of every
        // dictation.
        drive.send(Request::Finish {
            session,
            generation,
        });

        let text = wait_for_final(&drive, &mut seen)
            .unwrap_or_else(|| panic!("no final transcript for {}: {seen:?}", clip.file));

        // **Every sample the file held was authorised by the session.** This is
        // the assertion that the *pipeline* is transparent — ring, resampler,
        // `Session::audio` — as distinct from whatever the recogniser then made
        // of it, and it is what separates "the model is imperfect" from "we
        // dropped audio", which read identically in a transcript.
        let report = drive.status();
        assert_eq!(
            report.session.samples,
            samples.len() as u64,
            "{}: the worker authorised {} of {} samples",
            clip.file,
            report.session.samples,
            samples.len()
        );
        assert_eq!(
            report.ring_overruns, 0,
            "{}: the ring dropped audio, so the transcript is missing words \
             the recogniser never saw",
            clip.file
        );

        let partials = seen
            .iter()
            .filter(|event| matches!(event, Event::Partial { .. }))
            .count();
        eprintln!(
            "{} ({}, {} partials): {text}",
            clip.file,
            clip.language.tag(),
            partials
        );

        assert!(
            !text.trim().is_empty(),
            "{} produced an empty transcript: the runtime loaded and the recogniser said nothing",
            clip.file
        );
        assert!(
            text.chars().any(clip.script),
            "{} produced text with no {} characters in it: {text}",
            clip.file,
            clip.script_name
        );
        // The recogniser is a streaming one, so it revised itself while the
        // audio was arriving rather than answering only at `finish`.
        assert!(
            partials > 0,
            "{} produced no partial: a streaming recogniser that only answers at \
             finish is not streaming",
            clip.file
        );
    }
}

/// **The Compact product path, through the same worker boundary.** Whisper is
/// whole-utterance rather than streaming, so the proof expects no partial and
/// requires a non-empty final after `Finish`. This is the exact gap a live
/// microphone exposed: loading successfully is not evidence that any words
/// return to the document.
#[test]
fn compact_recorded_speech_becomes_text_after_finish() {
    let Some(dir) = whisper_model_dir() else {
        eprintln!(
            "SKIPPED compact_recorded_speech_becomes_text_after_finish: \
             KORU_VOICE_WHISPER_MODEL names no directory, so there are no weights to load."
        );
        return;
    };
    let Some(audio_root) = model_dir() else {
        eprintln!(
            "SKIPPED compact_recorded_speech_becomes_text_after_finish: \
             KORU_VOICE_SHERPA_MODEL names no directory containing the publisher's test WAVs."
        );
        return;
    };
    let clip = &CLIPS[2]; // de.wav: a short, clean Latin-script utterance.
    let path = audio_root.join("test_wavs").join(clip.file);
    if !path.is_file() {
        eprintln!(
            "SKIPPED {}: it is not in the provisioned package",
            clip.file
        );
        return;
    }
    let samples = read_16k_mono(&path).expect("the publisher's clip is 16-bit PCM");
    let mut drive = Driver::new(WorkerConfig {
        store: whisper_store(&dir),
        engine: Box::new(WhisperEngineHost::new()),
        capture: CaptureSource::Recorded(Arc::new(samples.clone())),
        ring_capacity: samples.len() + koru_voice::SAMPLE_RATE as usize,
        idle: crate::idle::IdlePolicy {
            unload_after: Duration::ZERO,
            exit_after: Duration::ZERO,
        },
        supervisor_launch_id: None,
    });

    let session = SessionId::new();
    let generation = drive.next_generation;
    drive.next_generation += 1;
    drive.send(Request::Begin {
        session,
        generation,
        language: Requested::Auto,
        tier: ModelTier::Compact,
        model: Some(ModelId::parse(WHISPER_MODEL).unwrap()),
        input: koru_voice::InputChoice::SystemDefault,
    });

    let mut seen = Vec::new();
    drive.listening(&mut seen);
    let finalising = Instant::now();
    drive.send(Request::Finish {
        session,
        generation,
    });
    let text = wait_for_final(&drive, &mut seen)
        .unwrap_or_else(|| panic!("Compact produced no final transcript: {seen:?}"));
    eprintln!(
        "Compact finalised {} in {} ms: {text}",
        clip.file,
        finalising.elapsed().as_millis()
    );

    let report = drive.status();
    assert_eq!(report.session.samples, samples.len() as u64);
    assert_eq!(report.ring_overruns, 0);
    assert!(
        !text.trim().is_empty(),
        "Compact returned an empty final transcript"
    );
    assert!(
        text.chars().any(clip.script),
        "Compact produced text with no {} characters: {text}",
        clip.script_name
    );
    assert!(
        !seen
            .iter()
            .any(|event| matches!(event, Event::Partial { .. })),
        "Compact is configured as whole-utterance and must not promise partials: {seen:?}"
    );
}

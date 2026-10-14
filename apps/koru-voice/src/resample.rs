// SPDX-License-Identifier: MPL-2.0

//! **Whatever the device gives us, into 16 kHz mono `f32`.**
//!
//! [`koru_voice::SAMPLE_RATE`] is declared once and four components resample
//! into it; this is the desktop's. The reason it is not optional is in that
//! constant's own note: an engine handed 48 kHz audio it believes is 16 kHz
//! transcribes fluent nonsense at a third of the speed rather than failing, so
//! there is no error to notice — only a bad transcript nobody can explain.
//!
//! ## Why linear interpolation, and what it costs
//!
//! A device's default input is 44.1 or 48 kHz, so this is nearly always a
//! **downsample by about three**, and linear interpolation downsampling
//! aliases: anything above 8 kHz in the source folds back into the band the
//! recogniser reads. Speech energy that matters is well below that, and every
//! candidate engine's own front end is a mel filterbank that discards most of
//! it again, so the honest description is *good enough and measured in M0*
//! rather than *correct*. A polyphase resampler with a proper low-pass is one
//! dependency (`rubato`) away if the benchmark says the accuracy is there; that
//! is a Plan 0.4 decision with a number attached, not one to take here on
//! taste. Written down in `docs/features/voice/archive/QUESTIONS.md` so it is not silently
//! re-decided.
//!
//! ## Both halves run inside the device callback
//!
//! So both are allocation-free and branch-light: [`Downmix`] is an average over
//! a frame and [`Resampler`] holds two `f32` and an `f64`. Neither owns a
//! buffer, because the output goes straight into [`crate::ring::Ring`] one
//! sample at a time and a scratch buffer would be one more thing whose capacity
//! could be wrong.

/// Average a device frame down to one sample.
///
/// **An average rather than the first channel**, because "the first channel" of
/// a stereo headset is one of the two microphones and choosing it silently
/// halves what a person said when they turn their head. Averaging is also what
/// every consumer capture stack does, so it is the behaviour a person's own
/// experience predicts.
#[derive(Copy, Clone, Debug)]
pub struct Downmix {
    channels: usize,
}

impl Downmix {
    /// `channels` is the device's, and zero is treated as one — a config that
    /// claimed no channels is a broken backend, and refusing to open a
    /// microphone over it would be a worse answer than mono.
    pub fn new(channels: u16) -> Downmix {
        Downmix {
            channels: (channels as usize).max(1),
        }
    }

    pub fn channels(self) -> usize {
        self.channels
    }

    /// Feed one interleaved buffer, calling `emit` once per whole frame.
    ///
    /// A trailing partial frame — which a backend should never produce and one
    /// of them eventually will — is dropped rather than padded with silence,
    /// because a padded frame is a click and a dropped one is nothing.
    pub fn frames(self, interleaved: &[f32], mut emit: impl FnMut(f32)) {
        for frame in interleaved.chunks_exact(self.channels) {
            let mut sum = 0.0f32;
            for sample in frame {
                sum += *sample;
            }
            emit(sum / self.channels as f32);
        }
    }
}

/// A stateful linear resampler from one rate to another.
///
/// Stateful because a device callback delivers a few hundred frames at a time
/// and the phase has to survive between them: a resampler restarted per buffer
/// produces a periodic discontinuity at exactly the callback rate, which is a
/// tone rather than a distortion.
#[derive(Clone, Debug)]
pub struct Resampler {
    /// Input samples per output sample. 3.0 at 48 kHz in, 16 kHz out.
    step: f64,
    /// Where the next output sample sits, in input samples after `previous`.
    phase: f64,
    previous: f32,
    started: bool,
}

impl Resampler {
    /// From `input_rate` to `output_rate`. A zero rate is treated as 1:1, for
    /// the same reason [`Downmix::new`] treats zero channels as one.
    pub fn new(input_rate: u32, output_rate: u32) -> Resampler {
        let step = if input_rate == 0 || output_rate == 0 {
            1.0
        } else {
            input_rate as f64 / output_rate as f64
        };
        Resampler {
            step,
            phase: 0.0,
            previous: 0.0,
            started: false,
        }
    }

    /// Whether this is a pass-through — the device is already at the rate the
    /// engine wants, which is worth knowing because it is the one case with no
    /// aliasing at all.
    pub fn is_identity(&self) -> bool {
        (self.step - 1.0).abs() < f64::EPSILON
    }

    /// Feed one mono sample; `emit` is called for each output sample it
    /// completes — none, one, or several when upsampling.
    pub fn push(&mut self, sample: f32, mut emit: impl FnMut(f32)) {
        if !self.started {
            // The first input sample is the anchor; nothing is emitted from it,
            // because interpolating from a zero that was never recorded puts a
            // ramp at the start of every session.
            self.started = true;
            self.previous = sample;
            return;
        }
        while self.phase < 1.0 {
            let t = self.phase as f32;
            emit(self.previous + (sample - self.previous) * t);
            self.phase += self.step;
        }
        self.phase -= 1.0;
        self.previous = sample;
    }
}

#[cfg(test)]
mod tests;

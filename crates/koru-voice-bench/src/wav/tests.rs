// SPDX-License-Identifier: MPL-2.0

use super::*;

/// Build a minimal RIFF/WAVE file, with an optional extra chunk between `fmt `
/// and `data`.
fn wav(
    rate: u32,
    channels: u16,
    bits: u16,
    samples: &[i16],
    extra: Option<(&[u8; 4], &[u8])>,
) -> Vec<u8> {
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&1u16.to_le_bytes()); // PCM
    fmt.extend_from_slice(&channels.to_le_bytes());
    fmt.extend_from_slice(&rate.to_le_bytes());
    fmt.extend_from_slice(&(rate * u32::from(channels) * u32::from(bits) / 8).to_le_bytes());
    fmt.extend_from_slice(&(channels * bits / 8).to_le_bytes());
    fmt.extend_from_slice(&bits.to_le_bytes());

    let mut data = Vec::new();
    for sample in samples {
        data.extend_from_slice(&sample.to_le_bytes());
    }

    let mut body = Vec::new();
    body.extend_from_slice(b"WAVE");
    body.extend_from_slice(b"fmt ");
    body.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
    body.extend_from_slice(&fmt);
    if let Some((id, payload)) = extra {
        body.extend_from_slice(id);
        body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        body.extend_from_slice(payload);
        // The pad byte the *writer* owes, which is what makes this fixture a
        // real test of the reader's pad handling rather than of its luck.
        if payload.len() % 2 == 1 {
            body.push(0);
        }
    }
    body.extend_from_slice(b"data");
    body.extend_from_slice(&(data.len() as u32).to_le_bytes());
    body.extend_from_slice(&data);

    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

#[test]
fn a_plain_16k_mono_file_reads() {
    let bytes = wav(16_000, 1, 16, &[0, 16_384, -16_384, 32_767], None);
    let samples = parse_wav(&bytes).expect("a well-formed file reads");
    assert_eq!(samples.len(), 4);
    assert_eq!(samples[0], 0.0);
    assert!((samples[1] - 0.5).abs() < 1e-6);
    assert!((samples[2] + 0.5).abs() < 1e-6);
}

#[test]
fn the_most_negative_sample_maps_to_exactly_minus_one() {
    // Scaling by 32767 would put it at -1.00003 and clip; dividing by 32768
    // makes the mapping exact for every negative value.
    let bytes = wav(16_000, 1, 16, &[i16::MIN], None);
    let samples = parse_wav(&bytes).expect("reads");
    assert_eq!(samples[0], -1.0);
}

#[test]
fn a_list_chunk_between_fmt_and_data_is_walked_past() {
    // ffmpeg writes one. A reader that seeks to byte 44 gets metadata and calls
    // it audio.
    let bytes = wav(
        16_000,
        1,
        16,
        &[1000, -1000],
        Some((b"LIST", b"INFOISFT\0lavf")),
    );
    let samples = parse_wav(&bytes).expect("a file with a LIST chunk reads");
    assert_eq!(samples.len(), 2);
}

#[test]
fn an_odd_length_chunk_is_padded_and_the_next_id_still_reads() {
    // Chunk bodies are padded to even lengths and the pad byte is not counted
    // in the size field. Skipping `size` rather than `size + size % 2` leaves
    // the cursor one byte inside the next id.
    let bytes = wav(16_000, 1, 16, &[7, 8, 9], Some((b"LIST", b"odd")));
    let samples = parse_wav(&bytes).expect("an odd-length chunk is skipped correctly");
    assert_eq!(samples.len(), 3);
}

#[test]
fn the_wrong_sample_rate_is_refused_rather_than_resampled() {
    // Resampling here would put this harness's resampler in every latency
    // figure, which is not a thing anybody asked to measure.
    let bytes = wav(48_000, 1, 16, &[0, 0], None);
    let error = parse_wav(&bytes).expect_err("48 kHz is refused");
    assert!(error.to_string().contains("48000"));
    assert!(error.to_string().contains("SAMPLE_RATE"));
}

#[test]
fn stereo_and_eight_bit_are_refused() {
    let stereo = wav(16_000, 2, 16, &[0, 0], None);
    assert!(
        parse_wav(&stereo)
            .expect_err("stereo is refused")
            .to_string()
            .contains("channels")
    );
    let eight = wav(16_000, 1, 8, &[0, 0], None);
    assert!(
        parse_wav(&eight)
            .expect_err("8-bit is refused")
            .to_string()
            .contains("8-bit")
    );
}

#[test]
fn a_file_that_is_not_riff_is_refused_before_anything_is_indexed() {
    assert!(parse_wav(b"not a wav at all").is_err());
    assert!(parse_wav(&[]).is_err());
}

#[test]
fn a_chunk_that_overruns_the_file_is_refused_rather_than_panicking() {
    // The failure mode a slice index would give is a panic inside a benchmark
    // that has been running for hours.
    let mut bytes = wav(16_000, 1, 16, &[1, 2, 3], None);
    let len = bytes.len();
    // Inflate the data chunk's declared size.
    bytes[len - 10..len - 6].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(parse_wav(&bytes).is_err());
}

#[test]
fn the_checked_in_smoke_fixture_reads_in_all_eight_languages() {
    // The fixture is 16 kHz mono s16 by construction, and this is what says so
    // after somebody replaces it with real recordings.
    let root = crate::corpus::SmokeFixture::default_root();
    for language in koru_voice::VoiceLanguage::ALL {
        let path = root.join(format!("{}.wav", language.tag()));
        let samples = read_wav(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let seconds = duration(&samples).as_secs_f64();
        assert!(
            (3.0..=5.0).contains(&seconds),
            "{} is {seconds:.2} s; the protocol says 3-5",
            path.display()
        );
    }
}

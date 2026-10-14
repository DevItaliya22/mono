# Koru patch: sherpa-onnx without text-to-speech

Koru builds the `sherpa-onnx` and `sherpa-onnx-sys` crates, version
**1.13.6**, from crates.io with Koru's changes in
[`sherpa-onnx.patch`](sherpa-onnx.patch) and
[`sherpa-onnx-sys.patch`](sherpa-onnx-sys.patch). `scripts/setup/fetch-third-party.*`
fetches each crate, checks it against its pin in
[`../fetch.manifest`](../fetch.manifest), applies its patch and places it in
`third-party/crates/`, where the workspace `Cargo.toml` substitutes it through
`[patch.crates-io]`. They are Apache-2.0 (each crate's own `LICENSE`), © the
k2-fsa / sherpa-onnx authors; the changes listed below are Koru's.

## Why

Upstream's ordinary prebuilt *static* archive includes sherpa-onnx's
text-to-speech, and `sherpa-onnx-sys` links its three libraries
unconditionally: `espeak-ng` and `ucd` (both **GPL-3.0**) and
`piper_phonemize` (MIT). Koru uses speech *recognition* only, but the linker
kept the TTS code anyway. The 2026-10-09 nightly's `koru-agent.exe`,
`koru-com-notification.exe` and `koru-voice-worker.exe` each contained eSpeak
NG. That made all three GPL-3.0 works as distributed. `licensing/THIRD-PARTY.md`
has the audit.

Upstream also publishes every static archive built with
`SHERPA_ONNX_ENABLE_TTS=OFF`, named `…-no-tts-lib.tar.bz2`. Those have no TTS
libraries at all, and no eSpeak NG code inside the others. Koru links those, on
every target. That needs both crates changed:

## What changed

**`sherpa-onnx`** (the safe wrapper):

- `src/tts.rs` is deleted, and `mod tts;` / `pub use tts::*;` are removed from
  `src/lib.rs`, along with its TTS doc example. The TTS-free C API has no TTS
  functions for these wrappers to call.

**`sherpa-onnx-sys`** (`build.rs` only; the bindings are unchanged):

- `piper_phonemize`, `espeak-ng` and `ucd` are removed from
  `SHERPA_ONNX_STATIC_LIBS`, and are never linked. What is linked is the other
  ten: `sherpa-onnx-c-api`, `sherpa-onnx-core`, `kaldi-decoder-core`,
  `sherpa-onnx-kaldifst-core`, `sherpa-onnx-fstfar`, `sherpa-onnx-fst`,
  `kaldi-native-fbank-core`, `kissfft-float`, `onnxruntime` and
  `ssentencepiece_core`, plus `c++` and the Foundation framework on macOS.
- Every static archive name is upstream's TTS-free one:
  `win-x64-static-MT-Release-no-tts-lib`, `osx-arm64-static-no-tts-lib`,
  `osx-x64-static-no-tts-lib`, and, for targets Koru does not ship,
  `win-arm64-static-MT-Release-no-tts-lib` and `linux-x64-static-no-tts-lib`.
  Downloading is upstream's code, unchanged; Koru's launchers place the pinned
  archive in `SHERPA_ONNX_ARCHIVE_DIR` first, so the build script copies it
  rather than downloading it.
- **Linux aarch64** is refused: upstream publishes no TTS-free static archive
  for it.
- **Shared linking names no archive**, and is an error unless
  `SHERPA_ONNX_LIB_DIR` names a directory. Upstream's shared archives that the
  crate would download carry text-to-speech, and Koru links statically.

The macOS archives also hold `sherpa-onnx-cxx-api` and
`sherpa-onnx-portaudio_static`. Neither is linked: the C API is all the
bindings use, and Koru captures audio itself with `cpal`.

Everything else in both crates, including `Cargo.toml` and the `sys`
bindings, is upstream's, unchanged.

## Updating

To move to a new sherpa-onnx version: change both crates' version and
crates.io sha256 in `fetch.manifest`, fetch, and resolve whatever in the
patches no longer applies, editing `third-party/crates/<name>/` and running
`scripts\setup\fetch-third-party.ps1 -UpdatePatch <name>` (or the `.sh`'s
`--update-patch <name>`) to rewrite each patch and print its new pin
([`../README.md`](../README.md) has the whole procedure). Then download the new
version's three `…-no-tts-lib` archives, check that none holds an
`espeak-ng`, `ucd` or `piper_phonemize` library, and pin their sha256 and size
in `crates/koru-voice-sherpa/src/artifacts.rs` and
`scripts/setup/fetch-voice-runtime.*`.

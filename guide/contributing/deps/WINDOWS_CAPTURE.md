# windows-capture

Windows Graphics Capture and a hardware video encoder, used only by the
accessibility runner's Plan 4.1 sample recorder and the Plan 4.2 contract
that froze it.
ADR-349-windowscapture,
ADR-350-recordingcontract.

Evaluated version: **2.0.1**. It links `windows` 0.62.2, the same pin
`koru-demo` already uses.

## What we rely on

- `Window::from_raw_hwnd` — capture the HWND the runner already proved, not a
  title search and not the foreground window.
- `CursorCaptureSettings::WithoutCursor`
- `DrawBorderSettings::WithoutBorder`
- `SecondaryWindowSettings::Include` — Koru menus are drawn into the same HWND
  today (`scene::floating_panel`); the flag is the notes' default if a popup
  ever becomes a second native window.
- `VideoSettingsSubType::H264` — **required**. The crate default is HEVC.
- `AudioSettingsBuilder::new().disabled(true)`
- `Frame::timestamp` exists; Plan 4.1's pairing origin is still the Instant the
  first frame was *successfully sent to the encoder*, because that is the moment
  we know an encoded frame exists. A configured `MinimumUpdateInterval` does not
  guarantee constant frame rate; the crate says so.
- `Frame::width` / `Frame::height` are recorded on each stamp so a mid-clip
  resize can fail as `capture-geometry-changed` instead of warping boxes.
- `GetDpiForWindow` is recorded as `dpi_scale_milli` (1000 = 100%) and is not
  applied; UIA bounds are already physical.

**A mid-clip resize does not error.** `send_frame` answers `Ok` to a frame
whose size no longer matches the encoder's: 2.0.1 pads or crops it into the
configured dimensions (`build_padded_surface` in the vendored `encoder.rs`)
and the clip warps *silently*. Nothing downstream says so — comparing the
per-frame stamps is the only way to know, which is why they are recorded.
koru-demo's pairing checks live (`same_capture_size`); the session
recorder's shell checks at finalize (`frames_hold_size`) and reports
`capture-geometry-changed`.

## What it is not

It is not reachable in a public `koru.exe`. The crate moved to
`crates/koru-recording` (ADR-422) and
is linked into every Windows build of the app, but the only in-app control —
the session recorder's menu row and F9 — is gated `this_build_is_internal() &&
cfg!(windows)`, so a public build carries the mechanism and offers nothing. It
does not replace UI Automation. It is not constructed by `cargo test
--workspace`. Ordinary tests drive `recording::pair` with injected frame
timestamps.

Apple requires a separate capture client and platform verification.

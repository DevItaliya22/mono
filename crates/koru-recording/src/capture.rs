// SPDX-License-Identifier: MPL-2.0

//! Windows Graphics Capture of one named HWND. Not compiled into ordinary tests'
//! execution path: nothing here is constructed by `cargo test --workspace`.
//!
//! H.264 is explicit (the crate default is HEVC), cursor and border are off
//! unless the caller asks otherwise, and audio is always disabled. `None` for
//! `frame_rate` / `bitrate_bps` keeps the encoder's own defaults (60 fps,
//! 15 Mbps) — the values the accepted Plan 4.1/4.2 clips were recorded with,
//! so `koru-demo` passes `None` and its contract is untouched.

use crate::frame::{FrameStamp, Origin};
use crate::geometry::{CaptureGeometry, Rect, even_dim};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};
use windows_capture::{
    capture::{CaptureControl, Context, GraphicsCaptureApiHandler},
    encoder::{
        AudioSettingsBuilder, ContainerSettingsBuilder, VideoEncoder, VideoSettingsBuilder,
        VideoSettingsSubType,
    },
    frame::Frame,
    graphics_capture_api::InternalCaptureControl,
    settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    },
    window::Window,
};

struct Shared {
    origin: Mutex<Option<Origin>>,
    ready: Condvar,
    frames: Mutex<Vec<FrameStamp>>,
    error: Mutex<Option<&'static str>>,
    stills: Mutex<Vec<StillRequest>>,
}

struct StillRequest {
    step: String,
    delivered_ms: u64,
    saved: bool,
}

pub struct Session {
    shared: Arc<Shared>,
    control: Option<CaptureControl<Handler, &'static str>>,
    stills_dir: Option<PathBuf>,
}

struct Handler {
    encoder: Option<VideoEncoder>,
    shared: Arc<Shared>,
    stills_dir: Option<PathBuf>,
    started: Instant,
}

pub struct StartConfig {
    pub window_handle: i64,
    pub process_id: u32,
    pub clip: PathBuf,
    /// Per-step PNG stills beside the clip. `None` records video only.
    pub stills: Option<PathBuf>,
    /// Whether the mouse cursor is visible in the clip.
    pub cursor: bool,
    /// `None` keeps the encoder default (60).
    pub frame_rate: Option<u32>,
    /// `None` keeps the encoder default (15 Mbps).
    pub bitrate_bps: Option<u32>,
}

struct HandlerFlags {
    clip: PathBuf,
    stills: Option<PathBuf>,
    width: u32,
    height: u32,
    frame_rate: Option<u32>,
    bitrate_bps: Option<u32>,
    shared: Arc<Shared>,
}

impl GraphicsCaptureApiHandler for Handler {
    type Flags = HandlerFlags;
    type Error = &'static str;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        let flags = ctx.flags;
        let mut video = VideoSettingsBuilder::new(flags.width, flags.height)
            .sub_type(VideoSettingsSubType::H264);
        if let Some(frame_rate) = flags.frame_rate {
            video = video.frame_rate(frame_rate);
        }
        if let Some(bitrate_bps) = flags.bitrate_bps {
            video = video.bitrate(bitrate_bps);
        }
        let encoder = VideoEncoder::new(
            video,
            AudioSettingsBuilder::new().disabled(true),
            ContainerSettingsBuilder::new(),
            flags.clip,
        )
        .map_err(|_| "encoder-unavailable")?;
        Ok(Self {
            encoder: Some(encoder),
            shared: flags.shared,
            stills_dir: flags.stills,
            started: Instant::now(),
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame<'_>,
        _capture_control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let encoder = self.encoder.as_mut().ok_or("encoder-unavailable")?;
        encoder.send_frame(frame).map_err(|_| "encode-failed")?;
        let arrived = Instant::now();
        let mut origin = self.shared.origin.lock().unwrap();
        if origin.is_none() {
            *origin = Some(Origin { instant: arrived });
            self.shared.ready.notify_all();
        }
        let origin_instant = origin.as_ref().unwrap().instant;
        drop(origin);
        let arrived_ms = arrived
            .saturating_duration_since(origin_instant)
            .as_millis() as u64;
        let mut frames = self.shared.frames.lock().unwrap();
        let index = frames.len() as u32;
        frames.push(FrameStamp {
            index,
            arrived_ms,
            width: Some(even_dim(frame.width())),
            height: Some(even_dim(frame.height())),
            dpi_scale_milli: None,
        });
        drop(frames);
        if let Some(stills_dir) = &self.stills_dir {
            let mut stills = self.shared.stills.lock().unwrap();
            for request in stills.iter_mut() {
                if request.saved || arrived_ms < request.delivered_ms {
                    continue;
                }
                let path = stills_dir.join(format!("{}.png", request.step));
                if frame
                    .save_as_image(&path, windows_capture::encoder::ImageFormat::Png)
                    .is_ok()
                {
                    request.saved = true;
                }
            }
        }
        let _ = self.started;
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        if let Some(encoder) = self.encoder.take() {
            encoder.finish().map_err(|_| "encode-finalize-failed")?;
        }
        Ok(())
    }
}

pub fn start(config: StartConfig) -> Result<(Session, CaptureGeometry), &'static str> {
    if config.window_handle == 0 {
        return Err("window-bind-failed");
    }
    let window = Window::from_raw_hwnd(config.window_handle as *mut std::ffi::c_void);
    if !window.is_valid() {
        return Err("window-bind-failed");
    }
    let process_id = window.process_id().map_err(|_| "window-bind-failed")?;
    if process_id != config.process_id {
        return Err("wrong-process");
    }
    let width = even_dim(window.width().map_err(|_| "capture-size-unavailable")? as u32);
    let height = even_dim(window.height().map_err(|_| "capture-size-unavailable")? as u32);
    if width == 0 || height == 0 {
        return Err("capture-size-unavailable");
    }
    let rect = window.rect().map_err(|_| "capture-size-unavailable")?;
    let title_bar_height = window.title_bar_height().unwrap_or(0);
    let hwnd = windows::Win32::Foundation::HWND(config.window_handle as *mut std::ffi::c_void);
    let dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) };
    let dpi_scale_milli = if dpi == 0 {
        1000
    } else {
        (dpi as u64 * 1000 / 96) as u32
    };
    let geometry = CaptureGeometry {
        window: Rect {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        },
        width,
        height,
        title_bar_height,
        dpi_scale_milli,
    };
    if let Some(stills) = &config.stills {
        std::fs::create_dir_all(stills).map_err(|_| "stills-unwritable")?;
    }
    let shared = Arc::new(Shared {
        origin: Mutex::new(None),
        ready: Condvar::new(),
        frames: Mutex::new(Vec::new()),
        error: Mutex::new(None),
        stills: Mutex::new(Vec::new()),
    });
    let cursor = if config.cursor {
        CursorCaptureSettings::WithCursor
    } else {
        CursorCaptureSettings::WithoutCursor
    };
    let settings = Settings::new(
        window,
        cursor,
        DrawBorderSettings::WithoutBorder,
        SecondaryWindowSettings::Include,
        MinimumUpdateIntervalSettings::Default,
        DirtyRegionSettings::Default,
        ColorFormat::Rgba8,
        HandlerFlags {
            clip: config.clip.clone(),
            stills: config.stills.clone(),
            width,
            height,
            frame_rate: config.frame_rate,
            bitrate_bps: config.bitrate_bps,
            shared: shared.clone(),
        },
    );
    let control = Handler::start_free_threaded(settings).map_err(|_| "capture-start-failed")?;
    Ok((
        Session {
            shared,
            control: Some(control),
            stills_dir: config.stills,
        },
        geometry,
    ))
}

impl Session {
    pub fn wait_first_frame(&self, timeout: Duration) -> Result<Origin, &'static str> {
        let started = Instant::now();
        let mut origin = self.shared.origin.lock().unwrap();
        loop {
            if let Some(ready) = origin.as_ref() {
                return Ok(Origin {
                    instant: ready.instant,
                });
            }
            if let Some(error) = *self.shared.error.lock().unwrap() {
                return Err(error);
            }
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return Err("capture-not-ready");
            }
            let (guard, timed_out) = self.shared.ready.wait_timeout(origin, remaining).unwrap();
            origin = guard;
            if timed_out.timed_out() && origin.is_none() {
                return Err("capture-not-ready");
            }
        }
    }

    pub fn request_still(&self, step: &str, delivered_ms: u64) {
        self.shared.stills.lock().unwrap().push(StillRequest {
            step: step.into(),
            delivered_ms,
            saved: false,
        });
    }

    pub fn frames(&self) -> Vec<FrameStamp> {
        self.shared.frames.lock().unwrap().clone()
    }

    pub fn saved_stills(&self) -> Vec<(String, String)> {
        self.shared
            .stills
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.saved)
            .map(|request| (request.step.clone(), format!("{}.png", request.step)))
            .collect()
    }

    pub fn stills_dir(&self) -> Option<&Path> {
        self.stills_dir.as_deref()
    }

    pub fn finish(mut self) -> (Vec<FrameStamp>, Result<(), &'static str>) {
        let frames = self.frames();
        let stop = match self.control.take() {
            Some(control) => control
                .stop()
                .map(|_| ())
                .map_err(|_| "capture-stop-failed"),
            None => Ok(()),
        };
        let result = if let Err(error) = stop {
            Err(error)
        } else if frames.is_empty() {
            Err("capture-not-ready")
        } else {
            Ok(())
        };
        (frames, result)
    }
}

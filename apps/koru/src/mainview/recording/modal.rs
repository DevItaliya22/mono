// SPDX-License-Identifier: MPL-2.0

//! The session recorder's setup modal — size, frame rate, cursor, destination
//! (the internal session-recorder design).
//!
//! **What is true lives here; where it is on screen is [`super::paint`], and
//! anything that touches the world is the container's.** The native picker,
//! the window lock and the capture itself are all performed outside the modal;
//! committing hands back an [`Outcome`]. That split is what lets every test in
//! `super::tests` drive the surface without a window.

use koru_draw::text::TextEngine;
use koru_draw::theme;
use koru_i18n::Key as I18nKey;
use koru_ui::{TextField, Typed};
use std::path::{Path, PathBuf};
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// The resolutions offered, in menu order. `Current` and `Custom` carry no
/// dimensions: the window's own size is the shell's to read at apply time
/// (never a stale view-side copy), and a custom pair is the person's to type.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum Preset {
    Current,
    Youtube,
    Tiktok,
    Square,
    Portrait,
    Web,
    Custom,
}

impl Preset {
    pub(crate) const ALL: [Preset; 7] = [
        Self::Current,
        Self::Youtube,
        Self::Tiktok,
        Self::Square,
        Self::Portrait,
        Self::Web,
        Self::Custom,
    ];

    /// The **physical** pixel size a fixed preset means, on any display scale.
    pub(crate) fn dims(self) -> Option<(u32, u32)> {
        match self {
            Self::Current | Self::Custom => None,
            Self::Youtube => Some((1920, 1080)),
            Self::Tiktok => Some((1080, 1920)),
            Self::Square => Some((1080, 1080)),
            Self::Portrait => Some((1080, 1350)),
            Self::Web => Some((1024, 768)),
        }
    }

    pub(crate) fn label(self) -> I18nKey {
        match self {
            Self::Current => I18nKey::RecordingPresetCurrent,
            Self::Youtube => I18nKey::RecordingPresetYoutube,
            Self::Tiktok => I18nKey::RecordingPresetTiktok,
            Self::Square => I18nKey::RecordingPresetSquare,
            Self::Portrait => I18nKey::RecordingPresetPortrait,
            Self::Web => I18nKey::RecordingPresetWeb,
            Self::Custom => I18nKey::RecordingPresetCustom,
        }
    }

    /// The stored form, for the settings section. Stable tokens, not the
    /// localized label — a recording rig does not rename itself with the UI
    /// language.
    pub(crate) fn token(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Youtube => "youtube",
            Self::Tiktok => "tiktok",
            Self::Square => "square",
            Self::Portrait => "portrait",
            Self::Web => "web",
            Self::Custom => "custom",
        }
    }

    /// Back from the stored form. `None` for anything unrecognised — a
    /// hand-edited settings file degrades to the modal's default, never to a
    /// failed load.
    pub(crate) fn from_token(token: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|preset| preset.token() == token)
    }
}

/// The frame rates offered, default first.
pub(crate) const FRAME_RATES: [u32; 2] = [60, 30];

/// A width or height the recorder accepts. Below 160 there is nothing left of
/// the app to read; above 8192 the encoder says no.
pub(crate) const MIN_DIM: u32 = 160;
pub(crate) const MAX_DIM: u32 = 8192;

/// Every control a person can reach, by keyboard or by pointer.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum Hit {
    Preset(usize),
    CustomWidth,
    CustomHeight,
    FrameRate(usize),
    Cursor(bool),
    Path,
    Browse,
    Record,
    Cancel,
}

/// What the container should do about a gesture.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum Outcome {
    /// The surface changed; redraw.
    Keep,
    /// Nothing changed (a key no control wanted, a disabled Record).
    Ignored,
    /// Take the surface down, changing nothing.
    Close,
    /// Open the save-file picker. A modal may not: a native dialog blocks, so
    /// the container performs it through the shell (the delegation picker's
    /// own rule), and the answer comes back through `MainView`'s
    /// `recording_path_chosen`.
    Browse,
    /// Validated: lock the size and start the countdown.
    Record(Request),
}

/// The size a [`Request`] asks the window to take.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum SizeChoice {
    /// Lock the window as it is — the shell reads the real inner size at apply
    /// time, so an in-flight resize cannot hand the lock a stale rectangle.
    Current,
    /// Lock exactly these physical pixels.
    Exact(u32, u32),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Request {
    pub size: SizeChoice,
    pub frame_rate: u32,
    pub cursor: bool,
    pub clip: PathBuf,
}

/// What a new modal pre-fills: the values the person last recorded with, or
/// the first-use set.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Defaults {
    pub preset: Preset,
    pub custom_width: String,
    pub custom_height: String,
    pub frame_rate: u32,
    pub cursor: bool,
    /// The folder the last clip went to. `None` on first use, when the modal
    /// falls back to the shell's `videos_dir`.
    pub folder: Option<PathBuf>,
}

impl Defaults {
    pub(crate) fn first_use() -> Self {
        Self {
            preset: Preset::Current,
            custom_width: "1280".into(),
            custom_height: "720".into(),
            frame_rate: FRAME_RATES[0],
            cursor: true,
            folder: None,
        }
    }
}

/// The default clip name, local time: `koru-<yyyymmdd>-<hhmmss>.mp4`.
pub(crate) fn timestamped_name() -> String {
    chrono::Local::now()
        .format("koru-%Y%m%d-%H%M%S.mp4")
        .to_string()
}

/// `path`, or `name-1.mp4`, `name-2.mp4`… when a file is already there.
/// Overwriting a recording somebody kept is not an option, and refusing the
/// whole start for a name collision is friction a dev tool does not need.
/// `exists` is injected so the rule itself stays testable without a disk.
pub(crate) fn suffixed(path: &Path, exists: impl Fn(&Path) -> bool) -> PathBuf {
    if !exists(path) {
        return path.to_path_buf();
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "koru".into());
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_else(|| "mp4".into());
    for n in 1..1000 {
        let candidate = path.with_file_name(format!("{stem}-{n}.{ext}"));
        if !exists(&candidate) {
            return candidate;
        }
    }
    // A thousand same-named clips is a fact worth seeing, not looping past.
    path.with_file_name(format!("{stem}-{}.{}", uuid::Uuid::new_v4(), ext))
}

/// The destination names a folder: non-empty, and more than a bare file
/// name — a relative `koru.mp4` would hand the shell a working-directory
/// path. Whether the folder *exists* is `commit`'s check, not a
/// per-keystroke one.
fn has_a_folder(raw: &str) -> bool {
    let raw = raw.trim();
    !raw.is_empty()
        && Path::new(raw)
            .parent()
            .is_some_and(|parent| !parent.as_os_str().is_empty())
}

/// Parse a dimension field: digits, adjusted down to the encoder's even
/// size, within [`MIN_DIM`]..=[`MAX_DIM`].
pub(crate) fn parse_dim(raw: &str) -> Option<u32> {
    let value: u32 = raw.trim().parse().ok()?;
    let even = koru_recording::even_dim(value);
    (MIN_DIM..=MAX_DIM).contains(&even).then_some(even)
}

pub(crate) struct Modal {
    preset: Preset,
    width: TextField,
    height: TextField,
    frame_rate: u32,
    cursor: bool,
    path: TextField,
    /// The keyboard focus ring's position. `None` only until the modal is
    /// first drawn: it opens on *Record*, because F9 → Enter is the whole
    /// point of the flow.
    focus: Option<Hit>,
    /// The first thing wrong with the current values, shown inline. Computed
    /// live, so *Record* can be dimmed rather than scolded after the fact.
    problem: Option<I18nKey>,
    /// The control the pointer is standing on. Paint reads the raw pointer
    /// for its hover tint; this copy is for the cursor answer, which is asked
    /// with no engine and no layout pass in hand.
    hover: Option<Hit>,
    dragging: bool,
}

impl Modal {
    pub(crate) fn new(engine: &mut TextEngine, defaults: &Defaults, suggested: &Path) -> Self {
        let mut modal = Self {
            preset: defaults.preset,
            width: TextField::with_text(engine, theme::role::body(), &defaults.custom_width),
            height: TextField::with_text(engine, theme::role::body(), &defaults.custom_height),
            frame_rate: defaults.frame_rate,
            cursor: defaults.cursor,
            path: TextField::with_text(engine, theme::role::body(), &suggested.to_string_lossy()),
            focus: None,
            problem: None,
            hover: None,
            dragging: false,
        };
        modal.revalidate();
        modal.focus = Some(Hit::Record);
        modal
    }

    pub(crate) fn preset(&self) -> Preset {
        self.preset
    }

    pub(crate) fn frame_rate(&self) -> u32 {
        self.frame_rate
    }

    pub(crate) fn cursor(&self) -> bool {
        self.cursor
    }

    pub(crate) fn focus(&self) -> Option<Hit> {
        self.focus
    }

    pub(crate) fn problem(&self) -> Option<I18nKey> {
        self.problem
    }

    /// The folder a finished clip went to, for the next modal's default.
    pub(crate) fn clip_folder(&self) -> Option<PathBuf> {
        let path = PathBuf::from(self.path.value());
        path.parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf)
    }

    /// The focus ring, in arrow order. The custom fields exist only while the
    /// *Custom…* preset is chosen, so focus can never point at a hidden
    /// control.
    fn ring(&self) -> Vec<Hit> {
        let mut ring: Vec<Hit> = (0..Preset::ALL.len()).map(Hit::Preset).collect();
        if self.preset == Preset::Custom {
            ring.push(Hit::CustomWidth);
            ring.push(Hit::CustomHeight);
        }
        ring.extend((0..FRAME_RATES.len()).map(Hit::FrameRate));
        ring.push(Hit::Cursor(true));
        ring.push(Hit::Cursor(false));
        ring.push(Hit::Path);
        ring.push(Hit::Browse);
        ring.push(Hit::Record);
        ring.push(Hit::Cancel);
        ring
    }

    fn move_focus(&mut self, engine: &mut TextEngine, backwards: bool) {
        let ring = self.ring();
        let at = self
            .focus
            .and_then(|hit| ring.iter().position(|candidate| *candidate == hit))
            .unwrap_or(0);
        let next = if backwards {
            (at + ring.len() - 1) % ring.len()
        } else {
            (at + 1) % ring.len()
        };
        self.focus_to(engine, ring[next]);
    }

    /// Put the ring on `hit`, giving a text field its caret conventions.
    fn focus_to(&mut self, engine: &mut TextEngine, hit: Hit) {
        self.focus = Some(hit);
        match hit {
            Hit::CustomWidth => self.width.move_to_end(engine),
            Hit::CustomHeight => self.height.move_to_end(engine),
            Hit::Path => self.path.move_to_end(engine),
            _ => {}
        }
    }

    fn focused_field(&mut self) -> Option<&mut TextField> {
        match self.focus {
            Some(Hit::CustomWidth) => Some(&mut self.width),
            Some(Hit::CustomHeight) => Some(&mut self.height),
            Some(Hit::Path) => Some(&mut self.path),
            _ => None,
        }
    }

    /// The fields, for the paint pass — drawing is the only thing these are
    /// for; behaviour stays in this file's own handlers.
    pub(crate) fn custom_width_field(&mut self) -> &mut TextField {
        &mut self.width
    }

    pub(crate) fn custom_height_field(&mut self) -> &mut TextField {
        &mut self.height
    }

    pub(crate) fn path_field(&mut self) -> &mut TextField {
        &mut self.path
    }

    /// Recompute the inline problem from the current values: the first thing
    /// wrong, or nothing. *Record* is enabled exactly when this is `None`.
    fn revalidate(&mut self) {
        self.problem = if self.preset == Preset::Custom
            && (parse_dim(&self.width.value()).is_none()
                || parse_dim(&self.height.value()).is_none())
        {
            Some(I18nKey::RecordingProblemSize)
        } else if !has_a_folder(&self.path.value()) {
            Some(I18nKey::RecordingProblemPath)
        } else {
            None
        };
    }

    /// One key press. The modal is a blocker: whatever happens, the key goes
    /// no further (`Ignored` still means "swallowed").
    pub(crate) fn key(
        &mut self,
        engine: &mut TextEngine,
        key: &Key,
        text: Option<&str>,
        mods: ModifiersState,
    ) -> Outcome {
        match key {
            Key::Named(NamedKey::Escape) => return Outcome::Close,
            // F9 toggles: it opened the modal, it can put it away again.
            Key::Named(NamedKey::F9) if mods.is_empty() => return Outcome::Close,
            Key::Named(NamedKey::Tab) => {
                self.move_focus(engine, mods.shift_key());
                return Outcome::Keep;
            }
            Key::Named(NamedKey::ArrowDown) if !self.field_focused() => {
                self.move_focus(engine, false);
                return Outcome::Keep;
            }
            Key::Named(NamedKey::ArrowUp) if !self.field_focused() => {
                self.move_focus(engine, true);
                return Outcome::Keep;
            }
            Key::Named(NamedKey::Enter) => {
                let hit = self.focus;
                return match hit {
                    Some(Hit::Record) | None => self.commit(),
                    Some(Hit::Cancel) => Outcome::Close,
                    Some(hit) => self.activate(hit),
                };
            }
            _ => {}
        }
        if self.field_focused()
            && let Some(field) = self.focused_field()
        {
            match field.key(engine, key, text, mods) {
                Typed::Handled => {
                    self.revalidate();
                    return Outcome::Keep;
                }
                // Enter/Tab/Escape/arrows were the panel's, decided above.
                Typed::Passed => return Outcome::Ignored,
            }
        }
        Outcome::Ignored
    }

    fn field_focused(&self) -> bool {
        matches!(
            self.focus,
            Some(Hit::CustomWidth) | Some(Hit::CustomHeight) | Some(Hit::Path)
        )
    }

    /// A pointer press on a control. Field clicks land the caret: `tx` is the
    /// press position inside the field's text area, which only the paint knows
    /// — see [`super::paint`].
    pub(crate) fn press(&mut self, engine: &mut TextEngine, hit: Hit, tx: f32) -> Outcome {
        match hit {
            Hit::CustomWidth => {
                self.focus = Some(hit);
                self.dragging = true;
                self.width.click(engine, tx);
                Outcome::Keep
            }
            Hit::CustomHeight => {
                self.focus = Some(hit);
                self.dragging = true;
                self.height.click(engine, tx);
                Outcome::Keep
            }
            Hit::Path => {
                self.focus = Some(hit);
                self.dragging = true;
                self.path.click(engine, tx);
                Outcome::Keep
            }
            _ => {
                self.focus = Some(hit);
                self.activate(hit)
            }
        }
    }

    /// Extend a field selection while the button is held.
    pub(crate) fn drag(&mut self, engine: &mut TextEngine, tx: f32) -> bool {
        if !self.dragging {
            return false;
        }
        if let Some(field) = self.focused_field() {
            field.drag_to(engine, tx);
        }
        true
    }

    pub(crate) fn release(&mut self) -> bool {
        std::mem::take(&mut self.dragging)
    }

    /// The control the pointer is standing on, reported by the container's
    /// move routing (the only place with the engine the hit test needs).
    pub(crate) fn set_hover(&mut self, hover: Option<Hit>) {
        self.hover = hover;
    }

    /// The cursor over the modal: text over a field, the press shape over a
    /// control, idle over the panel — the delegation modal's own table.
    pub(crate) fn cursor_icon(&self) -> winit::window::CursorIcon {
        match self.hover {
            Some(Hit::CustomWidth | Hit::CustomHeight | Hit::Path) => {
                winit::window::CursorIcon::Text
            }
            Some(_) => koru_ui::cursor::PRESS,
            None => koru_ui::cursor::IDLE,
        }
    }

    /// Choose a chip, press a button. Also `Enter`'s path for a focused
    /// control, so the two never disagree about what a control does.
    pub(crate) fn activate(&mut self, hit: Hit) -> Outcome {
        match hit {
            Hit::Preset(index) => {
                let Some(preset) = Preset::ALL.get(index).copied() else {
                    return Outcome::Ignored;
                };
                if self.preset != preset {
                    self.preset = preset;
                    self.revalidate();
                }
                Outcome::Keep
            }
            Hit::FrameRate(index) => {
                let Some(rate) = FRAME_RATES.get(index).copied() else {
                    return Outcome::Ignored;
                };
                self.frame_rate = rate;
                Outcome::Keep
            }
            Hit::Cursor(on) => {
                self.cursor = on;
                Outcome::Keep
            }
            Hit::Browse => Outcome::Browse,
            Hit::Record => self.commit(),
            Hit::Cancel => Outcome::Close,
            Hit::CustomWidth | Hit::CustomHeight | Hit::Path => Outcome::Keep,
        }
    }

    /// The person picked a path in the native picker: it replaces the field.
    pub(crate) fn chose_path(&mut self, engine: &mut TextEngine, chosen: &Path) {
        self.path.set_text(engine, &chosen.to_string_lossy());
        self.revalidate();
    }

    /// Validate and ask for the recording. Reaches the world only as a value;
    /// the container performs everything it says.
    fn commit(&mut self) -> Outcome {
        self.commit_with(super::dir_exists)
    }

    /// `commit` with the one filesystem question injected, so the suite
    /// answers it without a disk (the `suffixed` precedent).
    pub(crate) fn commit_with(&mut self, dir_exists: impl Fn(&Path) -> bool) -> Outcome {
        self.revalidate();
        if self.problem.is_some() {
            return Outcome::Ignored;
        }
        let clip = PathBuf::from(self.path.value().trim());
        // The plan's "parent exists" — the one check revalidate cannot make
        // without a disk, asked once at the Record press, never per keystroke.
        let folder_there = clip.parent().is_some_and(dir_exists);
        if !folder_there {
            self.problem = Some(I18nKey::RecordingProblemPath);
            return Outcome::Ignored;
        }
        let size = match self.preset {
            Preset::Current => SizeChoice::Current,
            Preset::Custom => {
                let (Some(w), Some(h)) = (
                    parse_dim(&self.width.value()),
                    parse_dim(&self.height.value()),
                ) else {
                    self.problem = Some(I18nKey::RecordingProblemSize);
                    return Outcome::Ignored;
                };
                SizeChoice::Exact(w, h)
            }
            preset => {
                let Some((w, h)) = preset.dims() else {
                    return Outcome::Ignored;
                };
                SizeChoice::Exact(w, h)
            }
        };
        Outcome::Record(Request {
            size,
            frame_rate: self.frame_rate,
            cursor: self.cursor,
            clip,
        })
    }
}

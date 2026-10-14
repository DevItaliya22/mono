// SPDX-License-Identifier: MPL-2.0

//! One compiled-step vocabulary, shared by tests and later recordings.
use crate::inspect::{Handle, Rect};
use std::fmt;

#[derive(Clone, PartialEq, Eq)]
pub enum Action {
    Focus,
    Click,
    RightClick,
    Text(String),
    Chord(Vec<u16>),
}
impl fmt::Debug for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.kind())
    }
}
impl Action {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Focus => "focus",
            Self::Click => "click",
            Self::RightClick => "right-click",
            Self::Text(_) => "text",
            Self::Chord(_) => "chord",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum Condition {
    Text(String),
    Name(String),
    NamePrefixWithout { prefix: String, excluded: String },
    SelectedText(String),
    Focused,
    Enabled(bool),
    Offscreen(bool),
    ReadOnly(bool),
    Selected(bool),
}
impl fmt::Debug for Condition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.kind())
    }
}
impl Condition {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Text(_) => "text-equals",
            Self::Name(_) => "name-equals",
            Self::NamePrefixWithout { .. } => "name-prefix-without",
            Self::SelectedText(_) => "selection-equals",
            Self::Focused => "focused",
            Self::Enabled(_) => "enabled",
            Self::Offscreen(_) => "offscreen",
            Self::ReadOnly(_) => "read-only",
            Self::Selected(_) => "selected",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Consequence {
    pub element: Handle,
    pub root_window: i64,
    pub process_id: u32,
    pub action: Action,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Result {
    Complete { bounds: Option<Rect> },
    Refused(&'static str),
}

/// Resolve the application's one shortcut registry instead of duplicating chords.
pub fn shortcut(id: koru_keys::ShortcutId) -> std::result::Result<Action, &'static str> {
    use koru_keys::On;
    use winit::keyboard::NamedKey;
    let shortcut = koru_keys::REGISTRY
        .iter()
        .find(|s| s.id == id)
        .ok_or("shortcut-missing")?;
    let chord = shortcut.chord;
    let key = match chord.on {
        On::Letter(c) if c.is_ascii_alphanumeric() => c.to_ascii_uppercase() as u16,
        On::Named(NamedKey::Enter) => 0x0d,
        On::Named(NamedKey::Tab) => 0x09,
        On::Named(NamedKey::Escape) => 0x1b,
        On::Named(NamedKey::ArrowLeft) => 0x25,
        On::Named(NamedKey::ArrowRight) => 0x27,
        On::Named(NamedKey::ContextMenu) => 0x5d,
        On::Named(NamedKey::F6) => 0x75,
        On::Named(NamedKey::F10) => 0x79,
        _ => return Err("shortcut-key-unsupported"),
    };
    let mut keys = Vec::new();
    if chord.mods.control_key() {
        keys.push(0x11);
    }
    if chord.mods.shift_key() {
        keys.push(0x10);
    }
    if chord.mods.alt_key() {
        keys.push(0x12);
    }
    if chord.mods.super_key() {
        return Err("shortcut-platform-unsupported");
    }
    keys.push(key);
    Ok(Action::Chord(keys))
}

#[cfg(test)]
mod tests;

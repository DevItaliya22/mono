// SPDX-License-Identifier: MPL-2.0

//! The standalone chat window's row in the machine-local geometry file.
//!
//! AI Chat is machine-wide, not profile-owned, so its geometry has one named
//! row beside Agent Search rather than a fabricated profile/slot key.

use std::fs;
use std::path::{Path, PathBuf};

use winit::dpi::PhysicalPosition;

const KEY: &str = "ai-chat";
pub const DEFAULT_WIDTH: f32 = 460.0;
pub const DEFAULT_HEIGHT: f32 = 680.0;

#[derive(Copy, Clone, Debug, PartialEq)]
struct Geometry {
    position: Option<PhysicalPosition<i32>>,
    /// Logical pixels, like profile-window geometry.
    size: (f32, f32),
    maximized: bool,
    pinned: bool,
}

impl Default for Geometry {
    fn default() -> Self {
        Self {
            position: None,
            size: (DEFAULT_WIDTH, DEFAULT_HEIGHT),
            maximized: false,
            pinned: false,
        }
    }
}

pub struct ChatGeometry {
    path: Option<PathBuf>,
    geometry: Geometry,
}

impl ChatGeometry {
    pub fn open(path: Option<PathBuf>) -> Self {
        let geometry = path.as_deref().and_then(load_geometry).unwrap_or_default();
        Self { path, geometry }
    }

    pub fn position(&self) -> Option<PhysicalPosition<i32>> {
        self.geometry.position
    }

    pub fn size(&self) -> (f32, f32) {
        self.geometry.size
    }

    pub fn maximized(&self) -> bool {
        self.geometry.maximized
    }

    pub fn pinned(&self) -> bool {
        self.geometry.pinned
    }

    pub fn moved(&mut self, position: PhysicalPosition<i32>) {
        if !self.geometry.maximized && usable_position(position) {
            self.geometry.position = Some(position);
        }
    }

    pub fn resized(&mut self, width: f32, height: f32) {
        if !self.geometry.maximized && usable_size(width, height) {
            self.geometry.size = (width, height);
        }
    }

    pub fn set_maximized(&mut self, maximized: bool) {
        self.geometry.maximized = maximized;
    }

    pub fn set_pinned(&mut self, pinned: bool) {
        self.geometry.pinned = pinned;
    }

    /// Persist at a lifecycle boundary, never in a move/resize event.
    pub fn persist(&self) {
        let Some(path) = &self.path else { return };
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let existing = fs::read_to_string(path).unwrap_or_default();
        let _ = fs::write(path, with_geometry(&existing, self.geometry));
    }
}

fn usable_position(position: PhysicalPosition<i32>) -> bool {
    // Windows' minimised sentinel is around (-32000, -32000). A real monitor
    // may be left or above the primary, but not this far on both axes.
    position.x > -10_000 || position.y > -10_000
}

fn usable_size(width: f32, height: f32) -> bool {
    width.is_finite()
        && height.is_finite()
        && (240.0..=20_000.0).contains(&width)
        && (240.0..=20_000.0).contains(&height)
}

fn load_geometry(path: &Path) -> Option<Geometry> {
    let text = fs::read_to_string(path).ok()?;
    let (_, row) = rows(&text).find(|(key, _)| *key == KEY)?;
    Some(parse_geometry(row))
}

fn parse_geometry(row: &str) -> Geometry {
    let mut geometry = Geometry::default();
    let (mut x, mut y, mut width, mut height) = (None, None, None, None);
    for field in row.split(';') {
        let Some((key, value)) = field.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "x" => x = value.parse().ok(),
            "y" => y = value.parse().ok(),
            "w" => width = value.parse().ok(),
            "h" => height = value.parse().ok(),
            "maximized" => geometry.maximized = value == "1" || value == "true",
            "pinned" => geometry.pinned = value == "1" || value == "true",
            _ => {}
        }
    }
    if let (Some(x), Some(y)) = (x, y) {
        let position = PhysicalPosition::new(x, y);
        geometry.position = usable_position(position).then_some(position);
    }
    if let (Some(width), Some(height)) = (width, height)
        && usable_size(width, height)
    {
        geometry.size = (width, height);
    }
    geometry
}

fn with_geometry(existing: &str, geometry: Geometry) -> String {
    let mut kept: Vec<(String, String)> = rows(existing)
        .filter(|(key, _)| *key != KEY)
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect();
    let mut fields = Vec::new();
    if let Some(position) = geometry.position {
        fields.push(format!("x={}", position.x));
        fields.push(format!("y={}", position.y));
    }
    fields.push(format!("w={:.0}", geometry.size.0));
    fields.push(format!("h={:.0}", geometry.size.1));
    fields.push(format!("maximized={}", u8::from(geometry.maximized)));
    fields.push(format!("pinned={}", u8::from(geometry.pinned)));
    kept.push((KEY.into(), fields.join(";")));

    let mut out = String::from(
        "# Where Koru's windows are on THIS machine.\n\
         # Profile windows own '<profile id> | <slot>'; standalone rows are 'global-search' and 'ai-chat'.\n\
         # Profile/AI Chat: x;y;w;h;maximized;pinned. Search: x;y;h.\n",
    );
    for (key, value) in kept {
        out.push_str(&format!("{key} = {value}\n"));
    }
    out
}

fn rows(text: &str) -> impl Iterator<Item = (&str, &str)> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim(), value.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_geometry_round_trips_without_disturbing_other_rows() {
        let existing = "profile-id | default = x=10;y=20;w=940;h=620;maximized=0;pinned=0\n\
                        global-search = x=30;y=40;h=500\n";
        let geometry = Geometry {
            position: Some(PhysicalPosition::new(-1200, 80)),
            size: (520.0, 740.0),
            maximized: true,
            pinned: true,
        };
        let body = with_geometry(existing, geometry);
        assert_eq!(
            parse_geometry(rows(&body).find(|(key, _)| *key == KEY).unwrap().1),
            geometry
        );
        assert!(body.contains("profile-id | default ="));
        assert!(body.contains("global-search ="));
    }

    #[test]
    fn minimized_sentinel_and_broken_sizes_are_ignored() {
        let geometry = parse_geometry("x=-32000;y=-32000;w=inf;h=10;maximized=true;pinned=true");
        assert_eq!(geometry.position, None);
        assert_eq!(geometry.size, (DEFAULT_WIDTH, DEFAULT_HEIGHT));
        assert!(geometry.maximized);
        assert!(geometry.pinned);
    }
}

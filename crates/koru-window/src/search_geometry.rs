// SPDX-License-Identifier: MPL-2.0

//! The `global-search` row in the shared window-geometry file.
//!
//! Keyed-row parsing, merging, coalescing and every filesystem write belong to
//! [`crate::window_geometry`], so closing a search surface never writes on the
//! event thread. What is here is the row's *value schema*: a position and a
//! height, and what counts as a usable one.
//!
//! ## Why it is in this crate rather than in the Agent
//!
//! Because two processes show global search — the Agent's overlay and the
//! shell's ephemeral search window — and until SP-6b they remembered its height
//! in two different files, the Agent here and the shell in `desktop-v1.json`.
//! That was the duplicate ADR-266 names: the height is geometry, and it was
//! only ever in the preferences file by adjacency. One schema, in the crate
//! both link, is the same argument that put `DesktopPreferences` here.
//!
//! **The last completed write wins**, which is this file's stated policy —
//! there is deliberately no lock. The shell never moves the row's position, and
//! [`SearchGeometry::reload_position`] lets it take the Agent's latest before
//! it writes, so the two cannot talk past each other about where the overlay is.

use std::path::PathBuf;

use winit::dpi::PhysicalPosition;

use crate::WindowRowWriter;

#[cfg(test)]
mod tests;

const KEY: &str = "global-search";

#[derive(Copy, Clone, Debug, Default, PartialEq)]
struct Geometry {
    position: Option<PhysicalPosition<i32>>,
    /// Logical pixels, like every other window size Koru persists.
    height: Option<f32>,
}

pub struct SearchGeometry {
    geometry: Geometry,
    path: Option<PathBuf>,
    writer: WindowRowWriter,
}

impl SearchGeometry {
    pub fn open(path: Option<PathBuf>) -> Self {
        let geometry = path
            .as_deref()
            .and_then(|path| crate::read_window_row(path, KEY))
            .map(|row| parse_geometry(&row))
            .unwrap_or_default();
        let writer = WindowRowWriter::open(path.clone(), KEY, "koru global-search geometry");
        Self {
            geometry,
            path,
            writer,
        }
    }

    pub fn disabled() -> Self {
        Self {
            geometry: Geometry::default(),
            path: None,
            writer: WindowRowWriter::default(),
        }
    }

    pub fn position(&self) -> Option<PhysicalPosition<i32>> {
        self.geometry.position
    }

    pub fn height(&self) -> Option<f32> {
        self.geometry.height
    }

    pub fn moved(&mut self, position: PhysicalPosition<i32>) {
        if usable(position) {
            self.geometry.position = Some(position);
        }
    }

    pub fn resized(&mut self, logical_height: f32) {
        if usable_height(logical_height) {
            self.geometry.height = Some(logical_height);
        }
    }

    /// **Take the row's current position back off the disk.**
    ///
    /// For a holder that changes the height and never the position — the
    /// shell's search window — called immediately before [`persist`]. Without
    /// it that process would write back whichever position it happened to read
    /// at launch, and the row's owner having moved its overlay since would be
    /// silently undone. One read, once, when a window closes.
    ///
    /// [`persist`]: Self::persist
    pub fn reload_position(&mut self) {
        let Some(position) = self
            .path
            .as_deref()
            .and_then(|path| crate::read_window_row(path, KEY))
            .and_then(|row| parse_geometry(&row).position)
        else {
            return;
        };
        self.geometry.position = Some(position);
    }

    /// Queue the last geometry once at teardown. The shared writer remembers
    /// the rendered row, so duplicate teardown paths and idle passes produce no
    /// additional writes.
    pub fn persist(&mut self) {
        if self.geometry.position.is_some() || self.geometry.height.is_some() {
            self.writer.write_if_changed(render_geometry(self.geometry));
        }
    }
}

fn usable(position: PhysicalPosition<i32>) -> bool {
    // Windows' minimised sentinel is around (-32000, -32000). A real monitor
    // may be left or above the primary, but not this far on both axes.
    position.x > -10_000 || position.y > -10_000
}

fn usable_height(height: f32) -> bool {
    height.is_finite() && (150.0..=20_000.0).contains(&height)
}

fn parse_geometry(row: &str) -> Geometry {
    let mut x = None;
    let mut y = None;
    let mut height = None;
    for field in row.split(';') {
        let Some((key, value)) = field.split_once('=') else {
            continue;
        };
        match key.trim() {
            "x" => x = value.trim().parse().ok(),
            "y" => y = value.trim().parse().ok(),
            "h" => {
                height = value
                    .trim()
                    .parse()
                    .ok()
                    .filter(|height| usable_height(*height))
            }
            _ => {}
        }
    }
    let position = match (x, y) {
        (Some(x), Some(y)) => {
            let position = PhysicalPosition::new(x, y);
            usable(position).then_some(position)
        }
        _ => None,
    };
    Geometry { position, height }
}

fn render_geometry(geometry: Geometry) -> String {
    let mut fields = Vec::new();
    if let Some(position) = geometry.position {
        fields.push(format!("x={}", position.x));
        fields.push(format!("y={}", position.y));
    }
    if let Some(height) = geometry.height {
        fields.push(format!("h={height:.0}"));
    }
    fields.join(";")
}

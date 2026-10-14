// SPDX-License-Identifier: MPL-2.0

//! Screen position is physical; client size is logical and independent of app zoom.
use koru_window::{WindowRowWriter, read_window_row, window_geometry_path};
use std::path::Path;

const KEY: &str = "ui-inspector";

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Geometry {
    pub position: Option<(i32, i32)>,
    pub size: (f32, f32),
    pub pinned: bool,
}

impl Default for Geometry {
    fn default() -> Self {
        Self {
            position: None,
            size: (520.0, 720.0),
            pinned: false,
        }
    }
}

pub(crate) struct Placement {
    pub geometry: Geometry,
    writer: WindowRowWriter,
}

impl Placement {
    pub fn open(root: Option<&Path>, remembered: Option<Geometry>) -> Self {
        let path = window_geometry_path(root);
        let geometry = remembered
            .or_else(|| {
                path.as_deref()
                    .and_then(|p| read_window_row(p, KEY))
                    .and_then(|row| parse(&row))
            })
            .unwrap_or_default();
        Self {
            geometry,
            writer: WindowRowWriter::open(path, KEY, "koru-inspector-position"),
        }
    }

    pub fn remember(&mut self, window: &winit::window::Window, pinned: bool) {
        if window.is_minimized() == Some(true) || window.is_maximized() {
            self.geometry.pinned = pinned;
            self.write();
            return;
        }
        let Ok(position) = window.outer_position() else {
            return;
        };
        let size = window.inner_size().to_logical::<f32>(window.scale_factor());
        if size.width <= 0.0 || size.height <= 0.0 {
            return;
        }
        self.geometry = Geometry {
            position: Some((position.x, position.y)),
            size: (size.width, size.height),
            pinned,
        };
        self.write();
    }

    fn write(&mut self) {
        let Some((x, y)) = self.geometry.position else {
            return;
        };
        self.writer.write_if_changed(format!(
            "v2 {} {} {} {} {}",
            x,
            y,
            self.geometry.size.0,
            self.geometry.size.1,
            u8::from(self.geometry.pinned)
        ));
    }
}

fn parse(row: &str) -> Option<Geometry> {
    let mut parts = row.split_whitespace();
    // v1 was an in-app panel position, which is not a screen coordinate.
    if parts.next()? != "v2" {
        return None;
    }
    let x = parts.next()?.parse().ok()?;
    let y = parts.next()?.parse().ok()?;
    let w: f32 = parts.next()?.parse().ok()?;
    let h: f32 = parts.next()?.parse().ok()?;
    let pinned = match parts.next()? {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    (parts.next().is_none()
        && w.is_finite()
        && h.is_finite()
        && (360.0..=4096.0).contains(&w)
        && (360.0..=4096.0).contains(&h))
    .then_some(Geometry {
        position: Some((x, y)),
        size: (w, h),
        pinned,
    })
}

#[cfg(test)]
mod tests;

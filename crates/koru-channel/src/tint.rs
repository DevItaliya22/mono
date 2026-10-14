// SPDX-License-Identifier: MPL-2.0

//! The color a channel wears, and how to put it on the mark.
//!
//! Five channels install side by side and their icons were pixel-identical —
//! the only way to tell a nightly tray icon from a stable one was hovering for
//! the tooltip. So each segment tints the product mark: stable keeps the brand
//! teal untouched, beta turns blue-violet, nightly amber, internal-debug goes
//! greyscale and internal-release a muted purple. The treatment is "set the
//! hue, keep each pixel's saturation and lightness, keep alpha": the spiral's
//! shading survives while its teal→green gradient flattens into one
//! recognisable color, which reads better at 16 px than a rotation would.
//!
//! **The same recipe exists twice, deliberately.** `scripts/icons/set-logo.py`
//! applies it in Pillow to produce the per-channel `assets/koru-*.ico` files
//! that the four `build.rs` scripts embed (Explorer, the taskbar via the exe
//! resource, and both trays read resource 1); this module applies it at
//! runtime wherever the code already holds RGBA — the window icon, the macOS
//! dock, and `koru-draw`'s in-app mark. Change a hue here and the script's
//! `TINTS` table must move with it, or the tray disagrees with the window.
//!
//! Pure `f32` arithmetic over raw bytes, no image crate — this crate is the
//! leaf of the dependency graph and stays one.

use crate::ReleaseChannel;

/// What a channel does to the mark's pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tint {
    /// The brand color, untouched. Stable is the app; it keeps its identity.
    None,
    /// Every pixel's hue is set to `degrees` (0–360) and its saturation
    /// multiplied by `saturation`; lightness and alpha are untouched.
    Hue { degrees: f32, saturation: f32 },
    /// Fully desaturated. The dev build stops impersonating the release.
    Greyscale,
}

/// The tint for a named channel. The `debug` split mirrors
/// [`crate::segment_of`]: internal-debug and internal-release are different
/// binaries and now look it.
pub fn tint_of(channel: ReleaseChannel, debug: bool) -> Tint {
    match channel {
        ReleaseChannel::Stable => Tint::None,
        ReleaseChannel::Beta => Tint::Hue {
            degrees: 235.0,
            saturation: 1.0,
        },
        ReleaseChannel::Nightly => Tint::Hue {
            degrees: 38.0,
            saturation: 1.0,
        },
        ReleaseChannel::Internal if debug => Tint::Greyscale,
        ReleaseChannel::Internal => Tint::Hue {
            degrees: 275.0,
            saturation: 0.5,
        },
    }
}

/// This build's tint.
pub fn tint() -> Tint {
    tint_of(ReleaseChannel::of_this_build(), cfg!(debug_assertions))
}

/// Apply a tint to interleaved RGBA bytes, in place.
///
/// `Tint::None` touches nothing, so a stable build's icon is bit-identical to
/// the committed asset. Alpha is never read or written.
pub fn apply(rgba: &mut [u8], tint: Tint) {
    if tint == Tint::None {
        return;
    }
    for px in rgba.chunks_exact_mut(4) {
        let (h, s, l) = hsl_of(px[0], px[1], px[2]);
        let (h, s) = match tint {
            Tint::None => unreachable!("returned above"),
            Tint::Hue {
                degrees,
                saturation,
            } => (degrees, s * saturation),
            Tint::Greyscale => (h, 0.0),
        };
        let (r, g, b) = rgb_of(h, s, l);
        px[0] = r;
        px[1] = g;
        px[2] = b;
    }
}

fn hsl_of(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let r = r as f32 / 255.0;
    let g = g as f32 / 255.0;
    let b = b as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if max == min {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h * 60.0, s, l)
}

fn rgb_of(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    if s <= 0.0 {
        let v = (l * 255.0).round() as u8;
        return (v, v, v);
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let h = h / 360.0;
    let channel = |t: f32| {
        let t = t.rem_euclid(1.0);
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (v * 255.0).round() as u8
    };
    (channel(h + 1.0 / 3.0), channel(h), channel(h - 1.0 / 3.0))
}

#[cfg(test)]
mod tests;

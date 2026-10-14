// SPDX-License-Identifier: MPL-2.0

//! Native clipping for softly rounded utility windows.
//!
//! DX12 surfaces created from an HWND advertise only opaque composition through
//! wgpu 28. A zero-alpha Vello clear therefore becomes black on Windows. A
//! window region is the reliable boundary there: pixels outside the surface do
//! not belong to the window and cannot be composited as an opaque rectangle.

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RegionGeometry {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    diameter: i32,
}

#[cfg(any(windows, test))]
fn geometry(width: u32, height: u32, scale_factor: f64) -> Option<RegionGeometry> {
    let width = i32::try_from(width).ok()?;
    let height = i32::try_from(height).ok()?;
    if width <= 0 || height <= 0 {
        return None;
    }
    let diameter = (f64::from(koru_draw::theme::PAGE_RADIUS) * 2.0 * scale_factor)
        .round()
        .clamp(1.0, f64::from(width.min(height))) as i32;
    Some(RegionGeometry {
        left: 0,
        top: 0,
        right: width,
        bottom: height,
        diameter,
    })
}

#[cfg(windows)]
pub(crate) fn apply(window: &winit::window::Window) -> bool {
    use windows_sys::Win32::Graphics::Gdi::{CreateRoundRectRgn, DeleteObject, SetWindowRgn};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let size = window.inner_size();
    let Some(g) = geometry(size.width, size.height, window.scale_factor()) else {
        return false;
    };
    let Ok(handle) = window.window_handle() else {
        return false;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return false;
    };
    let hwnd = handle.hwnd.get() as *mut core::ffi::c_void;

    // SAFETY: this runs on the HWND's creator thread. SetWindowRgn takes
    // ownership of a successfully installed region; on failure it remains ours.
    unsafe {
        let region = CreateRoundRectRgn(g.left, g.top, g.right, g.bottom, g.diameter, g.diameter);
        if region.is_null() {
            return false;
        }
        if SetWindowRgn(hwnd, region, 1) == 0 {
            DeleteObject(region);
            return false;
        }
    }
    true
}

#[cfg(not(windows))]
pub(crate) fn apply(_window: &winit::window::Window) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_matches_the_standard_window_radius_at_fractional_dpi() {
        assert_eq!(
            geometry(540, 99, 1.5),
            Some(RegionGeometry {
                left: 0,
                top: 0,
                right: 540,
                bottom: 99,
                diameter: 24,
            })
        );
    }

    #[test]
    fn an_empty_region_is_refused() {
        assert_eq!(geometry(0, 2, 1.0), None);
        assert_eq!(geometry(2, 0, 1.0), None);
    }
}

// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn details_scroll_uses_wrapped_height_and_reaches_the_final_pixel() {
    let mut inspector = Inspector::new(900.0, 700.0);
    inspector.show_hierarchy = false;
    inspector.details_cache.height = 1400.0;
    let area = inspector.control_rect(5);
    let bar = inspector.scrollbar().unwrap();
    assert_eq!(bar.max_scroll, 1400.0 - area.h);
    inspector.pointer = (bar.gutter_x, bar.track_bottom);
    inspector.scroll_to_pointer(0.0);
    assert_eq!(inspector.scroll, (1400.0 - area.h) as usize);
    inspector.details_cache.height = area.h;
    inspector.clamp_scroll();
    assert_eq!(inspector.scroll, 0);
    assert!(inspector.scrollbar().is_none());
}

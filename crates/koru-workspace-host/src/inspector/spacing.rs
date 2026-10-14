// SPDX-License-Identifier: MPL-2.0

//! Optional spacing overlay. Geometry is from the same snapshot as Details.
use super::*;
use vello::kurbo::{Line, Point, Rect as BoxRect};

#[derive(Debug)]
struct Ray {
    side: &'static str,
    end: Point,
    gap: f64,
    label: Point,
}

fn rays(selected: BoxRect, parent: BoxRect) -> [Ray; 4] {
    let c = selected.center();
    [
        Ray {
            side: "L",
            end: Point::new(parent.x0, c.y),
            gap: selected.x0 - parent.x0,
            label: Point::new((selected.x0 + parent.x0) / 2.0, c.y),
        },
        Ray {
            side: "T",
            end: Point::new(c.x, parent.y0),
            gap: selected.y0 - parent.y0,
            label: Point::new(c.x, (selected.y0 + parent.y0) / 2.0),
        },
        Ray {
            side: "R",
            end: Point::new(parent.x1, c.y),
            gap: parent.x1 - selected.x1,
            label: Point::new((selected.x1 + parent.x1) / 2.0, c.y),
        },
        Ray {
            side: "B",
            end: Point::new(c.x, parent.y1),
            gap: parent.y1 - selected.y1,
            label: Point::new(c.x, (selected.y1 + parent.y1) / 2.0),
        },
    ]
}

impl Inspector {
    pub(super) fn draw_spacing_rays(&mut self, canvas: &mut Scene, root: Affine) {
        let Some((_, record)) = self.highlighted_record() else {
            return;
        };
        let Some(parent) = record.parent.and_then(|i| self.snapshot.records.get(i)) else {
            return;
        };
        let selected = record.rect();
        let center = selected.center();
        let clip = parent.visible().intersect(BoxRect::new(
            0.0,
            0.0,
            self.viewport.0 as f64,
            self.viewport.1 as f64,
        ));
        if clip.width() <= 0.0 || clip.height() <= 0.0 || !clip.contains(center) {
            return;
        }
        let rays = rays(selected, parent.rect());
        let color = theme::accent_tone("red").map_or(theme::p().close_hover, |tone| tone.ink);
        canvas.push_clip_layer(vello::peniko::Fill::NonZero, root, &clip);
        for ray in &rays {
            scene::stroke(canvas, root, color, 1.0, &Line::new(center, ray.end));
            let (dx, dy) = if matches!(ray.side, "L" | "R") {
                (0.0, 4.0)
            } else {
                (4.0, 0.0)
            };
            scene::stroke(
                canvas,
                root,
                color,
                1.0,
                &Line::new(
                    Point::new(ray.end.x - dx, ray.end.y - dy),
                    Point::new(ray.end.x + dx, ray.end.y + dy),
                ),
            );
            // Endpoints outside the clip are not visible parent edges. Never
            // label the clipped segment as though it were the complete gap.
            if ray.gap < 0.0 || !endpoint_visible(ray.end, clip) || !clip.contains(ray.label) {
                continue;
            }
            let horizontal = matches!(ray.side, "L" | "R");
            let (w, h) = (104.0, 22.0);
            if (horizontal && ray.gap < w + 8.0) || (!horizontal && ray.gap < h + 8.0) {
                continue; // Narrow gaps stay readable in the panel.
            }
            let x = if horizontal {
                ray.label.x - w / 2.0
            } else {
                ray.label.x + 6.0
            };
            let y = if horizontal {
                ray.label.y + 6.0
            } else {
                ray.label.y - h / 2.0
            };
            let label_box = BoxRect::new(x, y, x + w, y + h);
            if label_box.intersect(clip) != label_box {
                continue;
            }
            scene::floating_panel(
                canvas,
                root,
                x as f32,
                y as f32,
                w as f32,
                h as f32,
                &scene::Panel::tooltip(theme::PAGE_RADIUS),
            );
            self.diagnostic_text(
                canvas,
                root,
                &format!("{} {:6.1} px", ray.side, ray.gap),
                x as f32 + 5.0,
                y as f32 + 3.0,
                w as f32 - 10.0,
                theme::p().bubble_fg,
            );
        }
        canvas.pop_layer();
    }
}

fn endpoint_visible(point: Point, clip: BoxRect) -> bool {
    // Closed bounds: right/bottom parent edges are valid endpoints too.
    point.x >= clip.x0 && point.x <= clip.x1 && point.y >= clip.y0 && point.y <= clip.y1
}

#[cfg(test)]
mod tests;

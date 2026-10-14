// SPDX-License-Identifier: MPL-2.0

//! CPU-only comparison of the previous shell paint with current inspection off/on.
//! Synthetic rectangles; no fonts, Store, native windows or GPU presentation.
use koru_draw::{geometry::Rect, inspect, scene, theme};
use koru_ui::popover::PopoverShell;
use std::{hint::black_box, time::Instant};
use vello::{Scene, kurbo::Affine};

fn frame(canvas: &mut Scene, mode: usize, revision: u64) {
    canvas.reset();
    let capture = (mode == 2).then(|| {
        inspect::Frame::begin(
            revision,
            Affine::IDENTITY,
            vello::kurbo::Rect::new(0.0, 0.0, 1200.0, 800.0),
        )
    });
    for row in 0..10 {
        for col in 0..9 {
            let shell = PopoverShell {
                rect: Rect::new(col as f32 * 130.0, row as f32 * 75.0, 120.0, 65.0),
                radius: theme::overlay::popover().radius,
            };
            if mode == 0 {
                // The exact pre-QW1 draw body: baseline without the new scope.
                scene::floating_panel(
                    canvas,
                    Affine::IDENTITY,
                    shell.rect.x,
                    shell.rect.y,
                    shell.rect.w,
                    shell.rect.h,
                    &scene::Panel::popover(shell.radius).fill(theme::p().panel_bg),
                );
            } else {
                shell.draw(canvas, Affine::IDENTITY);
            }
        }
    }
    if let Some(capture) = capture {
        black_box(capture.finish());
    }
    black_box(&canvas);
}

fn main() {
    let mut canvas = Scene::new();
    let mut samples = [Vec::new(), Vec::new(), Vec::new()];
    for revision in 0..360 {
        let mode = revision as usize % 3;
        let start = Instant::now();
        frame(&mut canvas, mode, revision);
        if revision >= 60 {
            samples[mode].push(start.elapsed().as_nanos());
        }
    }
    println!(
        "CPU shell paint: 90 synthetic panels, 100 samples/mode after warmup; no GPU/input/placement cost"
    );
    for (name, times) in [
        "previous paint / off",
        "current paint / off",
        "current paint / on",
    ]
    .into_iter()
    .zip(&mut samples)
    {
        times.sort_unstable();
        println!(
            "{name}: median={:.1}us p95={:.1}us",
            times[50] as f64 / 1000.0,
            times[94] as f64 / 1000.0
        );
    }
}

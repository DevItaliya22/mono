// SPDX-License-Identifier: MPL-2.0

//! Legacy shell + content versus shared callback, with capture off/on.
//! CPU synthetic geometry only; no fonts, Store, native window or GPU.
use koru_draw::{geometry::Rect, inspect, scene, theme};
use koru_ui::PopoverShell;
use std::{hint::black_box, time::Instant};
use vello::{Scene, kurbo::Affine};

fn main() {
    let mut canvas = Scene::new();
    let mut samples = [Vec::new(), Vec::new(), Vec::new()];
    for revision in 0..360 {
        let mode = revision as usize % 3;
        let start = Instant::now();
        canvas.reset();
        let frame = (mode == 2).then(|| {
            inspect::Frame::begin(
                revision,
                Affine::IDENTITY,
                scene::rect(0.0, 0.0, 1200.0, 800.0),
            )
        });
        for i in 0..90 {
            let shell = PopoverShell {
                rect: Rect::new((i % 9) as f32 * 130.0, (i / 9) as f32 * 75.0, 120.0, 65.0),
                radius: 10.0,
            };
            let content = |canvas: &mut Scene, root| {
                scene::fill(
                    canvas,
                    root,
                    theme::p().text,
                    &scene::rect(shell.rect.x + 12.0, shell.rect.y + 12.0, 40.0, 12.0),
                )
            };
            if mode == 0 {
                shell.draw(&mut canvas, Affine::IDENTITY);
                content(&mut canvas, Affine::IDENTITY);
            } else {
                shell.draw_with_content(&mut canvas, Affine::IDENTITY, Default::default(), content);
            }
        }
        if let Some(frame) = frame {
            black_box(frame.finish());
        }
        black_box(&canvas);
        if revision >= 60 {
            samples[mode].push(start.elapsed().as_nanos());
        }
    }
    println!("CPU 90 synthetic shell+content surfaces; 100 samples/mode after warmup");
    for (name, times) in [
        "legacy shell+content / off",
        "callback / off",
        "callback / on",
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

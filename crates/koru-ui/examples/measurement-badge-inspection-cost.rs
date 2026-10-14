// SPDX-License-Identifier: MPL-2.0

//! CPU measurement+paint: old resize recipe and shared badge off/on.
use koru_draw::{inspect, scene, text::TextEngine, theme};
use koru_ui::KoruMeasurementBadge;
use std::{hint::black_box, time::Instant};
use vello::{Scene, kurbo::Affine};

fn main() {
    let mut engine = TextEngine::new();
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
                scene::rect(0.0, 0.0, 900.0, 650.0),
            )
        });
        for i in 0..12 {
            let x = 30.0 + i as f32 * 30.0;
            if mode == 0 {
                let label =
                    engine.simple_layout_in("480 × 360", theme::role::hint(), theme::code_stack());
                scene::floating_panel(
                    &mut canvas,
                    Affine::IDENTITY,
                    x,
                    40.0,
                    label.width() + 14.0,
                    label.height() + 14.0,
                    &scene::Panel::tooltip(6.0),
                );
                scene::draw_layout(
                    &mut canvas,
                    Affine::IDENTITY,
                    &label,
                    x + 7.0,
                    47.0,
                    Some(theme::p().bubble_fg),
                );
            } else {
                KoruMeasurementBadge::measure(&mut engine, "480 × 360", 1.0).draw(
                    &mut canvas,
                    Affine::IDENTITY,
                    x,
                    40.0,
                );
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
    println!(
        "CPU 12 synthetic measured badges; 100 samples/mode after warmup; no GPU/native input"
    );
    for (name, times) in ["legacy / off", "shared / off", "shared / on"]
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

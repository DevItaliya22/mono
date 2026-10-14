// SPDX-License-Identifier: MPL-2.0

//! CPU-only old compact frame paint versus shared frame with capture off/on.
//! Synthetic geometry, no fonts, native input, Store, GPU or document reflow.
use koru_draw::{inspect, scene, theme};
use koru_ui::KoruDialogFrame;
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
                scene::rect(0.0, 0.0, 900.0, 650.0),
            )
        });
        for _ in 0..90 {
            if mode == 0 {
                scene::fill(
                    &mut canvas,
                    Affine::IDENTITY,
                    theme::p().scrim,
                    &scene::rect(0.0, 0.0, 900.0, 650.0),
                );
                scene::floating_panel(
                    &mut canvas,
                    Affine::IDENTITY,
                    290.0,
                    283.0,
                    320.0,
                    84.0,
                    &scene::Panel::popover(8.0),
                );
            } else {
                KoruDialogFrame::name_prompt((900.0, 650.0)).draw(
                    &mut canvas,
                    Affine::IDENTITY,
                    Default::default(),
                    |_, _, _| {},
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
    println!("CPU 90 synthetic compact frames; 100 samples/mode after warmup");
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

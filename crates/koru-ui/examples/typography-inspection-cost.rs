// SPDX-License-Identifier: MPL-2.0

//! Current shared dialog CPU cost with inspection off/on. Synthetic text only.
//! No Store, native window, input dispatch or GPU presentation.
use koru_draw::{inspect, text::TextEngine};
use koru_ui::{Choice, Confirm};
use std::{hint::black_box, time::Instant};
use vello::{Scene, kurbo::Affine};

fn main() {
    let mut engine = TextEngine::new();
    let mut canvas = Scene::new();
    let confirm = Confirm::new("Review changes", "Keep this synthetic draft?", "Continue");
    let choice = Choice::new("Close example?", "Choose how to keep this synthetic draft.")
        .answer("Keep draft")
        .answer("Continue")
        .defaulting_to(0);
    let mut samples = [Vec::new(), Vec::new()];
    for revision in 0..240 {
        let mode = revision as usize % 2;
        let start = Instant::now();
        canvas.reset();
        let capture = (mode == 1).then(|| {
            inspect::Frame::begin(
                revision,
                Affine::IDENTITY,
                vello::kurbo::Rect::new(0.0, 0.0, 800.0, 600.0),
            )
        });
        confirm.draw(&mut canvas, Affine::IDENTITY, &mut engine, (800.0, 600.0));
        choice.draw(&mut canvas, Affine::IDENTITY, &mut engine, (800.0, 600.0));
        if let Some(frame) = capture {
            black_box(frame.finish());
        }
        black_box(&canvas);
        if revision >= 40 {
            samples[mode].push(start.elapsed().as_nanos());
        }
    }
    println!(
        "Two synthetic dialogs; current CPU draw+layout, 100 samples/mode after warmup; not a before/after regression benchmark"
    );
    for (name, times) in ["inspection off", "inspection on"]
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

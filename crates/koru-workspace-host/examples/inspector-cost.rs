// SPDX-License-Identifier: MPL-2.0

//! Reproducible CPU draw/capture comparison on synthetic shared buttons.
//! This does not measure GPU submission, native input latency or app reflow.
use koru_draw::{geometry::Rect, inspect, text::TextEngine};
use koru_ui::button::{ActionButton, ActionStyle};
use std::hint::black_box;
use std::time::Instant;
use vello::{Scene, kurbo::Affine};

fn frame(scene: &mut Scene, engine: &mut TextEngine, enabled: bool, revision: u64) -> usize {
    scene.reset();
    let capture = enabled.then(|| {
        inspect::Frame::begin(
            revision,
            Affine::IDENTITY,
            vello::kurbo::Rect::new(0.0, 0.0, 1200.0, 800.0),
        )
    });
    for row in 0..15 {
        for col in 0..6 {
            ActionButton::new("Synthetic button", ActionStyle::Ghost, None).draw_focused(
                scene,
                Affine::IDENTITY,
                engine,
                Rect::new(
                    10.0 + col as f32 * 190.0,
                    10.0 + row as f32 * 48.0,
                    180.0,
                    36.0,
                ),
                false,
                false,
            );
        }
    }
    capture.map_or(0, |capture| black_box(capture.finish()).records.len())
}

fn main() {
    let mut scene = Scene::new();
    let mut engine = TextEngine::new();
    for revision in 0..20 {
        black_box(frame(&mut scene, &mut engine, revision % 2 == 0, revision));
    }
    let mut samples = [Vec::new(), Vec::new()];
    let mut records = 0;
    // Alternate order to reduce warmup/order bias. Font loading is outside the measurement.
    for revision in 20..220 {
        let enabled = revision % 2 == 0;
        let start = Instant::now();
        let count = frame(&mut scene, &mut engine, enabled, revision);
        samples[usize::from(enabled)].push(start.elapsed().as_micros());
        records = records.max(count);
    }
    println!(
        "CPU-only synthetic draw: 90 shared buttons, 1200x800 logical, 100 samples per mode, records={records}"
    );
    for (index, times) in samples.iter_mut().enumerate() {
        times.sort_unstable();
        let mean = times.iter().sum::<u128>() / times.len() as u128;
        println!(
            "capture {}: mean={mean}us median={}us p95={}us max={}us",
            if index == 0 { "off" } else { "on" },
            times[50],
            times[94],
            times[99]
        );
    }
    println!(
        "Excludes inspector panel, GPU presentation and native input; not the main-app acceptance budget."
    );
}

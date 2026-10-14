// SPDX-License-Identifier: MPL-2.0

//! CPU-only menu paint and capture cost, using synthetic labels and the real
//! shared painter. No Store, native window, input, GPU or document reflow.
use koru_draw::{inspect, scene, text::TextEngine};
use koru_ui::{Anchor, Menu, MenuItem};
use std::{borrow::Cow, hint::black_box, time::Instant};
use vello::{Scene, kurbo::Affine};

struct Row;
impl MenuItem for Row {
    fn label(&self) -> Cow<'_, str> {
        "Synthetic menu item".into()
    }
}

fn main() {
    let mut engine = TextEngine::new();
    let mut canvas = Scene::new();
    let menu = Menu::at_width(
        (30.0, 30.0),
        (0..12).map(|_| Row).collect(),
        Anchor::Cursor,
        230.0,
    );
    let mut samples = [Vec::new(), Vec::new()];
    let mut counts = (0, false);
    for revision in 0..440 {
        let mode = revision as usize % 2;
        let start = Instant::now();
        canvas.reset();
        let frame = (mode == 1).then(|| {
            inspect::Frame::begin(
                revision,
                Affine::IDENTITY,
                scene::rect(0.0, 0.0, 1200.0, 800.0),
            )
        });
        menu.draw(
            &mut canvas,
            Affine::IDENTITY,
            &mut engine,
            (1200.0, 800.0),
            &[true; 12],
        );
        if let Some(frame) = frame {
            let snapshot = frame.finish();
            counts = (snapshot.records.len(), snapshot.truncated);
            black_box(snapshot);
        }
        black_box(&canvas);
        if revision >= 40 {
            samples[mode].push(start.elapsed().as_nanos());
        }
    }
    println!(
        "CPU shared menu paint: 12 synthetic rows; 200 samples/mode after warmup; capture records={}, truncated={}",
        counts.0, counts.1
    );
    for (name, times) in ["inspection off", "inspection on"]
        .into_iter()
        .zip(&mut samples)
    {
        times.sort_unstable();
        println!(
            "{name}: median={:.1}us p95={:.1}us",
            times[100] as f64 / 1000.0,
            times[189] as f64 / 1000.0
        );
    }
}

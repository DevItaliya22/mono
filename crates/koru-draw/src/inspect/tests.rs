// SPDX-License-Identifier: MPL-2.0

use super::*;

fn viewport() -> Rect {
    Rect::new(0.0, 0.0, 300.0, 300.0)
}

#[test]
fn painted_image_is_pickable_above_its_pane_with_transformed_clipped_bounds() {
    let image = peniko::ImageBrush::new(peniko::ImageData {
        data: peniko::Blob::new(std::sync::Arc::new(vec![255u8; 8 * 4 * 4])),
        format: peniko::ImageFormat::Rgba8,
        alpha_type: peniko::ImageAlphaType::Alpha,
        width: 8,
        height: 4,
    });
    let root = Affine::scale(2.0);
    let frame = Frame::begin(1, root, viewport());
    leaf("pane", root, viewport(), |_| {});
    let clipping = clip(root, Rect::new(0.0, 0.0, 100.0, 100.0));
    let transform = root * Affine::translate((60.0, 50.0)) * Affine::scale(10.0);
    crate::scene::draw_image(&mut vello::Scene::new(), transform, &image);
    drop(clipping);
    let snapshot = frame.finish();
    let selected = snapshot.pick(80.0, 60.0).expect("the raster has a record");
    let record = &snapshot.records[selected];
    assert_eq!(record.name, "Image");
    assert_eq!(record.bounds, [60.0, 50.0, 140.0, 90.0]);
    assert_eq!(record.visible(), Rect::new(60.0, 50.0, 100.0, 90.0));
    assert_eq!(
        snapshot.pick(120.0, 60.0),
        Some(0),
        "outside image clip picks the pane"
    );
    assert_eq!(record.properties.len(), 2);
    assert_eq!(record.properties[0].value, "8");
    assert_eq!(record.properties[1].value, "4");
    assert_eq!(
        snapshot.owner(selected),
        None,
        "raster evidence is not component approval"
    );
}

#[test]
fn contrast_requires_known_opaque_colors() {
    let white = peniko::Color::new([1.0, 1.0, 1.0, 1.0]);
    let black = peniko::Color::new([0.0, 0.0, 0.0, 1.0]);
    assert!((contrast(white, black).unwrap() - 21.0).abs() < 0.001);
    assert_eq!(contrast(white, white), Some(1.0));
    assert_eq!(contrast(white.multiply_alpha(0.5), black), None);
}

#[test]
fn disabled_seams_do_not_evaluate_metadata_or_allocate_capture() {
    assert!(!enabled());
    let _scope = scope("unused", "component", Affine::IDENTITY, viewport(), |_| {
        panic!("off metadata")
    });
    leaf("unused", Affine::IDENTITY, viewport(), |_| {
        panic!("off metadata")
    });
    CAPTURE.with_borrow(|capture| assert!(capture.is_none()));
}

#[test]
fn picking_preserves_zoom_translation_clip_and_paint_order() {
    let root = Affine::scale(3.0);
    let frame = Frame::begin(12, root, viewport());
    let clip = clip(root, Rect::new(20.0, 20.0, 80.0, 80.0));
    leaf("under", root, viewport(), |_| {});
    leaf(
        "top",
        root * Affine::translate((50.0, 50.0)),
        Rect::new(0.0, 0.0, 100.0, 100.0),
        |_| {},
    );
    drop(clip);
    let result = frame.finish();
    assert_eq!(result.pick(55.0, 55.0), Some(1));
    assert_eq!(result.pick(25.0, 25.0), Some(0));
    assert_eq!(result.pick(90.0, 55.0), None);
    assert_eq!(result.records[1].bounds, [50.0, 50.0, 150.0, 150.0]);
    assert!(!enabled());
}

#[test]
fn local_child_does_not_inherit_registered_parent_identity() {
    let frame = Frame::begin(1, Affine::IDENTITY, viewport());
    {
        let _parent = scope(
            "KoruFixture",
            "component",
            Affine::IDENTITY,
            viewport(),
            |_| {},
        );
        let _local = scope(
            "local",
            "unregistered",
            Affine::IDENTITY,
            viewport(),
            |_| {},
        );
        leaf("text", Affine::IDENTITY, viewport(), |_| {});
    }
    let result = frame.finish();
    assert_eq!(result.pick(40.0, 40.0), Some(2));
    assert_eq!(result.owner(2), None);
    assert_eq!(result.records[2].parent, Some(1));
}

#[test]
fn overflow_is_explicit_and_never_picks_an_underlying_official_control() {
    let frame = Frame::begin(1, Affine::IDENTITY, viewport());
    for _ in 0..MAX_RECORDS + 5 {
        leaf("fill", Affine::IDENTITY, viewport(), |_| {});
    }
    let result = frame.finish();
    assert_eq!(result.records.len(), MAX_RECORDS);
    assert!(result.truncated);
    assert_eq!(result.pick(20.0, 20.0), None);
}

#[test]
fn unwinding_a_frame_disarms_the_collector() {
    let _ = std::panic::catch_unwind(|| {
        let _frame = Frame::begin(1, Affine::IDENTITY, viewport());
        let _scope = scope("example", "component", Affine::IDENTITY, viewport(), |_| {});
        panic!("test unwind");
    });
    assert!(!enabled());
    CAPTURE.with_borrow(|capture| assert!(capture.is_none()));
}

#[test]
fn export_is_structured_and_has_frame_and_source_without_document_text() {
    let frame = Frame::begin(42, Affine::IDENTITY, viewport());
    leaf("shape", Affine::IDENTITY, viewport(), |r| {
        r.property("font size", 13, "shared token")
    });
    let result = frame.finish();
    let value: serde_json::Value = serde_json::from_str(&result.export(Some(0)).unwrap()).unwrap();
    assert_eq!(value["snapshot"]["revision"], 42);
    assert_eq!(value["selected"], 0);
    assert!(
        value["snapshot"]["records"][0]["source"]
            .as_str()
            .unwrap()
            .starts_with("crates/")
    );
    assert!(value["snapshot"]["records"][0].get("text").is_none());
}
#[test]
fn overflow_cannot_attach_unbounded_properties_to_previous_owner() {
    let frame = Frame::begin(1, Affine::IDENTITY, Rect::new(0.0, 0.0, 100.0, 100.0));
    let owner = scope(
        "owner",
        "container",
        Affine::IDENTITY,
        Rect::new(0.0, 0.0, 100.0, 100.0),
        |_| {},
    );
    for _ in 0..MAX_RECORDS + 10 {
        let _child = scope(
            "child",
            "component",
            Affine::IDENTITY,
            Rect::new(0.0, 0.0, 10.0, 10.0),
            |_| {},
        );
        current(|r| r.property("state", "measured", "fixture"));
    }
    drop(owner);
    let snapshot = frame.finish();
    assert!(snapshot.truncated);
    assert!(snapshot.records[0].properties.is_empty());
}

#[test]
fn a_hoisted_subtree_leads_the_hierarchy_with_remapped_parents() {
    let frame = Frame::begin(1, Affine::IDENTITY, viewport());
    leaf("before", Affine::IDENTITY, viewport(), |_| {});
    {
        let _modal = scope_hoisted(
            "KoruModal",
            "component",
            Affine::IDENTITY,
            viewport(),
            |_| {},
        );
        leaf("inside", Affine::IDENTITY, viewport(), |_| {});
    }
    leaf("after", Affine::IDENTITY, viewport(), |_| {});
    let result = frame.finish();
    let names: Vec<_> = result.records.iter().map(|r| r.name).collect();
    assert_eq!(names, ["KoruModal", "inside", "before", "after"]);
    assert_eq!(result.records[0].parent, None);
    assert!(result.records[0].hoisted);
    assert_eq!(result.records[1].parent, Some(0));
    assert_eq!(result.records[2].parent, None);
    assert_eq!(result.records[3].parent, None);
    // The vec leads the tree; z keeps the paint order the records arrived in.
    let zs: Vec<_> = result.records.iter().map(|r| r.z).collect();
    assert_eq!(zs, [1, 2, 0, 3]);
}

#[test]
fn picking_over_a_hoisted_subtree_follows_paint_order_not_vec_order() {
    let frame = Frame::begin(1, Affine::IDENTITY, viewport());
    leaf("window", Affine::IDENTITY, viewport(), |_| {});
    {
        let _modal = scope_hoisted(
            "KoruModal",
            "component",
            Affine::IDENTITY,
            viewport(),
            |_| {},
        );
        leaf("scrim", Affine::IDENTITY, viewport(), |_| {});
    }
    let result = frame.finish();
    // The window painted first but trails the hoisted modal subtree in the vec.
    let names: Vec<_> = result.records.iter().map(|r| r.name).collect();
    assert_eq!(names, ["KoruModal", "scrim", "window"]);
    // Every record covers the point; the last-painted one (the scrim) is on
    // top — vec order would name the window beneath the modal instead.
    assert_eq!(result.pick(40.0, 40.0), Some(1));
}

#[test]
fn a_hoisted_scope_inside_an_unmoved_parent_still_leads_as_a_top_level_row() {
    let frame = Frame::begin(1, Affine::IDENTITY, viewport());
    {
        let _window = scope("window", "container", Affine::IDENTITY, viewport(), |_| {});
        let _modal = scope_hoisted(
            "KoruModal",
            "component",
            Affine::IDENTITY,
            viewport(),
            |_| {},
        );
    }
    let result = frame.finish();
    assert_eq!(result.records[0].name, "KoruModal");
    // Its recorded parent stayed behind in the vec, which is exactly how the
    // hierarchy view tells a hoisted root reads as a top-level row.
    assert_eq!(result.records[0].parent, Some(1));
    assert_eq!(result.records[1].name, "window");
    assert_eq!(result.records[1].parent, None);
}

#[test]
fn a_frame_without_hoisting_keeps_append_order() {
    let frame = Frame::begin(1, Affine::IDENTITY, viewport());
    leaf("one", Affine::IDENTITY, viewport(), |_| {});
    {
        let _two = scope("two", "component", Affine::IDENTITY, viewport(), |_| {});
        leaf("three", Affine::IDENTITY, viewport(), |_| {});
    }
    leaf("four", Affine::IDENTITY, viewport(), |_| {});
    let result = frame.finish();
    let names: Vec<_> = result.records.iter().map(|r| r.name).collect();
    assert_eq!(names, ["one", "two", "three", "four"]);
    assert_eq!(result.records[2].parent, Some(1));
    assert!(result.records.iter().all(|r| !r.hoisted));
    assert!(
        result
            .records
            .iter()
            .enumerate()
            .all(|(i, r)| r.z == i as u32)
    );
    assert_eq!(result.pick(40.0, 40.0), Some(3));
}

// SPDX-License-Identifier: MPL-2.0

//! The column's rows are the explore's rows: one `ExploreRow` each, in the
//! library's treatment vocabulary, and the active row's card as its first
//! child — the same look every other explore view gives.

use super::*;

fn capture(surface: &mut ArtifactsSurface) -> Vec<koru_draw::inspect::Record> {
    let mut scene = vello::Scene::new();
    let mut engine = koru_draw::text::TextEngine::new();
    let root = vello::kurbo::Affine::IDENTITY;
    let frame =
        koru_draw::inspect::Frame::begin(1, root, koru_draw::scene::rect(0.0, 0.0, 260.0, 720.0));
    surface.draw(&mut scene, root, &mut engine, area(), TOP);
    frame.finish().records
}

#[test]
fn every_row_is_an_explore_row_and_the_selected_one_leads_with_its_card() {
    let one = family(1, "Findings", 500);
    let two = family(2, "Reports", 400);
    let v1 = history(&one, 2);
    let v2 = history(&two, 1);
    let present = all_present(&[(one.clone(), v1.clone()), (two.clone(), v2.clone())]);
    let latest = v1.last().unwrap().id;
    let mut surface = showing(Catalog::build(
        vec![(one, v1), (two, v2)],
        Vec::new(),
        Some(present),
    ));
    surface.selected = Some(latest);
    surface.hover = Some(Row::Family(1));

    let records = capture(&mut surface);
    let rows: Vec<usize> = records
        .iter()
        .enumerate()
        .filter_map(|(i, r)| (r.name == "ExploreRow").then_some(i))
        .collect();
    // Results heading, then one row per family, newest first.
    assert_eq!(rows.len(), 3, "one ExploreRow per drawn row");
    let treatment = |at: usize| {
        records[at]
            .properties
            .iter()
            .find(|p| p.name == "treatment")
            .map(|p| p.value.as_str())
    };
    assert_eq!(treatment(rows[0]), Some("Heading"));
    assert_eq!(treatment(rows[1]), Some("Card"), "the selected family");
    assert_eq!(
        treatment(rows[2]),
        Some("Hover"),
        "the family under the pointer"
    );

    // The fill that gives the active look is the first thing inside the row,
    // exactly as the library's picked rows wear it.
    let carded = rows[1];
    let first_child = records
        .iter()
        .find(|r| r.parent == Some(carded))
        .map(|r| r.name);
    assert_eq!(first_child, Some("fill"));
}

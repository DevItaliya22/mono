// SPDX-License-Identifier: MPL-2.0

use super::*;

fn three_generations() -> Provenance {
    let v1 = version(1, None);
    let v2 = version(2, Some(v1.id));
    let v3 = version(3, Some(v2.id));
    let link = |run: u8, version: ArtifactVersionId, role| RunLink {
        run: run_id(run),
        version,
        role,
        linked_at_ms: 5,
    };
    Provenance {
        subject: v3.clone(),
        ancestors: vec![v2.clone(), v1.clone()],
        runs: vec![
            link(0x20, v3.id, LinkRole::Output),
            link(0x20, v2.id, LinkRole::Input),
            link(0x10, v1.id, LinkRole::Output),
        ],
        deleted_runs: Default::default(),
    }
}

#[test]
fn a_walk_reads_nearest_ancestor_first() {
    let provenance = three_generations();
    assert_eq!(provenance.generations(), 3);
    let ordinals: Vec<_> = provenance
        .versions()
        .map(|version| version.ordinal.get())
        .collect();
    assert_eq!(ordinals, [3, 2, 1]);
}

#[test]
fn links_are_selected_by_version_and_role() {
    let provenance = three_generations();
    let consumed = provenance.links(version_id(2), LinkRole::Input);
    assert_eq!(consumed.len(), 1);
    assert_eq!(consumed[0].run, run_id(0x20));
    assert!(provenance.links(version_id(2), LinkRole::Output).is_empty());
    assert_eq!(
        provenance.links(version_id(3), LinkRole::Output)[0].run,
        run_id(0x20)
    );
}

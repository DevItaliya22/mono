// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn search_geometry_value_round_trips() {
    let geometry = Geometry {
        position: Some(PhysicalPosition::new(-1200, 80)),
        height: Some(720.0),
    };
    assert_eq!(parse_geometry(&render_geometry(geometry)), geometry);
}

#[test]
fn the_minimised_sentinel_is_neither_loaded_nor_recorded() {
    assert_eq!(parse_geometry("x=-32000;y=-32000").position, None);
    let mut geometry = SearchGeometry::disabled();
    geometry.moved(PhysicalPosition::new(-32000, -32000));
    assert_eq!(geometry.position(), None);
}

#[test]
fn an_old_position_only_row_still_loads() {
    let geometry = parse_geometry("x=10;y=20");
    assert_eq!(geometry.position, Some(PhysicalPosition::new(10, 20)));
    assert_eq!(geometry.height, None);
}

#[test]
fn duplicate_persistence_is_idle_after_the_first_row() {
    let mut geometry = SearchGeometry::disabled();
    geometry.moved(PhysicalPosition::new(10, 20));
    geometry.resized(600.0);
    geometry.persist();
    assert!(!geometry.writer.would_write("x=10;y=20;h=600"));
    geometry.persist();
    assert!(!geometry.writer.would_write("x=10;y=20;h=600"));
}

/// **The relocation, from the other side.** The height used to live in
/// `desktop-v1.json`, where the settings authority now writes; it is geometry,
/// and it was only ever in the preferences file by adjacency. This is the row
/// it moves to, and it is the row the Agent has always written.
#[test]
fn the_height_is_read_back_out_of_the_row_the_agent_already_owns() {
    let root = tempfile::tempdir().unwrap();
    let path = crate::window_geometry_path(Some(root.path())).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "global-search = x=1;y=2;h=640\n").unwrap();

    let geometry = SearchGeometry::open(Some(path));
    assert_eq!(geometry.height(), Some(640.0));
    assert_eq!(geometry.position(), Some(PhysicalPosition::new(1, 2)));
}

/// **A holder that only changes the height does not undo a move it never saw.**
///
/// Two processes show global search and the file has no lock — the last
/// completed write wins, deliberately. The shell changes the height and never
/// the position, so it takes the row's current position back off the disk
/// before it writes, and the Agent having moved its overlay in between stands.
#[test]
fn reloading_the_position_keeps_a_move_this_process_never_saw() {
    let root = tempfile::tempdir().unwrap();
    let path = crate::window_geometry_path(Some(root.path())).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "global-search = x=1;y=2;h=640\n").unwrap();

    let mut height_only = SearchGeometry::open(Some(path.clone()));
    // Somebody else moved the overlay while this one was open.
    std::fs::write(&path, "global-search = x=900;y=40;h=640\n").unwrap();
    height_only.resized(800.0);
    height_only.reload_position();

    assert_eq!(height_only.position(), Some(PhysicalPosition::new(900, 40)));
    assert_eq!(
        render_geometry(height_only.geometry),
        "x=900;y=40;h=800",
        "the height this process changed, over the position it did not"
    );
}

/// A row that does not exist yet leaves the position alone rather than
/// clearing one this process was given — an overlay that has never been moved
/// is not a reason to forget where a window was placed.
#[test]
fn reloading_from_nothing_keeps_what_is_held() {
    let mut geometry = SearchGeometry::disabled();
    geometry.moved(PhysicalPosition::new(10, 20));
    geometry.reload_position();
    assert_eq!(geometry.position(), Some(PhysicalPosition::new(10, 20)));
}

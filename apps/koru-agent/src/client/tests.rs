// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn status_names_the_routing_log_without_disclosing_the_raw_root() {
    let raw_root = r"C:\Users\Private Person\seeded-root-for-status";
    let paths = Paths::for_root(raw_root.into());

    let line = routing_log_status(&paths);

    assert_eq!(line, "  route log: agent/routing.log");
    assert!(!line.contains(raw_root));
    assert!(!line.contains("Private Person"));
}

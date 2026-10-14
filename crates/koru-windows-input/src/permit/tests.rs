// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn revocation_reaches_every_clone_and_never_runs_a_late_consequence() {
    let permit = DeliveryPermit::default();
    let worker = permit.clone();
    assert_eq!(worker.consequence(|| 7), Some(7));
    permit.revoke();
    assert!(!worker.is_live());
    assert_eq!(
        worker.consequence(|| panic!("abandoned provider injected")),
        None::<()>
    );
}

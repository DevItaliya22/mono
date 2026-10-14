// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn refusal_keeps_the_existing_agent_vocabulary() {
    for (platform, agent) in [
        (native::Refusal::NoTarget, Refusal::NoTarget),
        (native::Refusal::ProtectedTarget, Refusal::ProtectedTarget),
        (native::Refusal::TargetChanged, Refusal::TargetChanged),
        (native::Refusal::TargetDestroyed, Refusal::TargetDestroyed),
        (
            native::Refusal::TargetUnavailable,
            Refusal::TargetUnavailable,
        ),
    ] {
        assert_eq!(Refusal::from(platform), agent);
    }
}

#[test]
fn targetstate_keeps_the_existing_agent_vocabulary() {
    for (platform, agent) in [
        (native::TargetState::Editable, TargetState::Editable),
        (native::TargetState::Protected, TargetState::Protected),
        (native::TargetState::Changed, TargetState::Changed),
        (native::TargetState::Destroyed, TargetState::Destroyed),
        (native::TargetState::Unavailable, TargetState::Unavailable),
    ] {
        assert_eq!(TargetState::from(platform), agent);
    }
}

#[test]
fn directinsertion_keeps_the_existing_agent_vocabulary() {
    for (platform, agent) in [
        (native::DirectInsertion::Complete, DirectInsertion::Complete),
        (native::DirectInsertion::Rejected, DirectInsertion::Rejected),
        (native::DirectInsertion::Partial, DirectInsertion::Partial),
        (
            native::DirectInsertion::TargetUnavailable,
            DirectInsertion::TargetUnavailable,
        ),
    ] {
        assert_eq!(DirectInsertion::from(platform), agent);
    }
}

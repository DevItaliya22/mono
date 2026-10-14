// SPDX-License-Identifier: MPL-2.0

//! The seam, asserted: a run view under test reaches nothing, and what it
//! asks this window for is the three verbs it cannot send itself.

use super::*;

use std::cell::Cell;

use chrono::NaiveDateTime;
use koru_core::time::{TimeZoneName, ZoneRules};
use koru_time_zone::IanaZoneRules;

struct CountingZone<'a> {
    inner: &'a IanaZoneRules,
    wall_calls: Cell<u32>,
}

impl<'a> CountingZone<'a> {
    fn new(inner: &'a IanaZoneRules) -> Self {
        Self {
            inner,
            wall_calls: Cell::new(0),
        }
    }
}

impl ZoneRules for CountingZone<'_> {
    fn zone(&self) -> &TimeZoneName {
        self.inner.zone()
    }

    fn instant_of(&self, wall: NaiveDateTime) -> i64 {
        self.inner.instant_of(wall)
    }

    fn wall_of(&self, instant_ms: i64) -> NaiveDateTime {
        self.wall_calls.set(self.wall_calls.get() + 1);
        self.inner.wall_of(instant_ms)
    }
}

fn v4(n: u8) -> uuid::Uuid {
    format!("00000000-0000-4000-8000-0000000000{n:02x}")
        .parse()
        .unwrap()
}

#[test]
fn a_run_view_with_no_worker_holds_no_run_and_asks_for_nothing() {
    // Under test the link is inert, which is the whole point: this
    // binary's suite must not reach whatever gateway the machine has
    // running, and a view that dialled one would.
    let mut view = RunView::new();
    assert_eq!(view.run(), None);
    assert_eq!(view.session(), None);
    assert!(view.take_requests().is_empty());
    assert!(!view.poll());
}

#[test]
fn opening_a_run_names_it_and_still_reaches_nothing() {
    let mut view = RunView::new();
    let run = koru_delegate_core::RunId::from_uuid(uuid::Uuid::from_u128(7));
    let workspace = koru_delegate_core::WorkspaceId::from_uuid(uuid::Uuid::from_u128(8));
    view.open(run, workspace);
    assert_eq!(view.run(), Some(run));
    // No worker answered, so there is nothing to show and nothing to ask
    // for — a view that had invented either would be lying about a run.
    assert!(view.take_requests().is_empty());
    assert!(view.surface_mut().snapshot().is_none());
}

/// The app owns the only non-presentation step in UC7's row: convert the
/// retained epoch to the reader's local wall clock. A scripted API answer
/// followed by the ordinary maintenance input proves an open Run follows a
/// live device-zone change without another read or repeated invalidation.
#[test]
fn maintenance_reprojects_an_open_scheduled_run_when_the_device_zone_changes() {
    use koru_delegate_api::wire::{RunExecution, RunView as ApiRunView, TimelineWindow};
    use koru_delegate_core::{
        ActorRef, DelegationRequest, DeviceId, Objective, ProfileId, RunHeader, RunOrigin,
        ScheduleId, SessionProfileKey, WorkspaceScope,
    };
    use koru_i18n::time::{HourCycle, LocaleFormat};
    use koru_runs_ui::{Answer, RunLinks, Tab};

    let run = koru_delegate_core::RunId::from_uuid(v4(1));
    let workspace = koru_delegate_core::WorkspaceId::from_uuid(v4(2));
    let device = DeviceId::from_uuid(v4(3));
    let schedule = ScheduleId::from_uuid(v4(4));
    let occurrence_ms = 1_778_019_660_000;
    let header = RunHeader::from_request(
        DelegationRequest {
            run,
            requested_by: ActorRef::Person {
                profile: ProfileId::from_uuid(v4(5)),
            },
            objective: Objective::new("Read the provenance").unwrap(),
            agent: SessionProfileKey::BuiltIn("codex".parse().unwrap()),
            requested_model: None,
            scope: WorkspaceScope::Temporary,
            context: Vec::new(),
            grants: Default::default(),
            executing_device: device,
            origin: RunOrigin::Scheduled {
                schedule,
                occurrence_ms,
                revision: 7,
            },
        },
        occurrence_ms,
    );
    let (links, scripted) = RunLinks::scripted();
    let mut view = RunView {
        surface: RunsSurface::with_links(links),
        presentation_zone: None,
        presented_occurrence_ms: None,
        presentation_counts: PresentationCounts::default(),
    };
    let locale = LocaleFormat::for_system(Some("en-GB"), Some(HourCycle::H24));
    view.open(run, workspace);
    let _ = scripted.commands();
    scripted.answer(Answer::Read {
        run,
        workspace: Some(workspace),
        view: Box::new(ApiRunView {
            document: koru_delegate_core::DocumentId::from_uuid(v4(6)),
            header,
            execution: RunExecution {
                device,
                session: None,
                run_workspace: None,
            },
            context: Vec::new(),
            timeline: TimelineWindow {
                total: 0,
                from: 0,
                items: Vec::new(),
            },
        }),
        folder: None,
    });

    assert!(view.poll());
    view.surface_mut().set_tab(Tab::Activity);
    let utc = IanaZoneRules::parse("UTC").unwrap();
    let counted_utc = CountingZone::new(&utc);
    assert!(view.maintain_presentation_with_rules(&locale, &utc, &counted_utc));
    assert_eq!(counted_utc.wall_calls.get(), 1);
    assert_eq!(
        view.presentation_counts,
        PresentationCounts {
            locale_clones: 1,
            zone_clones: 1,
        },
        "the first presentation owns one copy of each environment value"
    );
    let utc_time = locale.date_time(crate::format::parts_in_zone(occurrence_ms, &utc).unwrap());
    let utc_lines = view.surface_mut().lines();
    assert!(
        utc_lines.iter().any(|line| {
            line.contains(&schedule.to_storage()) && line.contains(&utc_time) && line.contains('7')
        }),
        "the one API read and maintenance environment supplied the row: {utc_lines:?}"
    );
    let counts_after_first = view.presentation_counts;
    assert!(
        !view.maintain_presentation_with_rules(&locale, &utc, &counted_utc),
        "an identical environment must request neither repaint nor reflow"
    );
    assert_eq!(
        counted_utc.wall_calls.get(),
        1,
        "the cached zone and epoch skip wall projection"
    );
    assert_eq!(
        view.presentation_counts, counts_after_first,
        "the identical pass clones neither locale nor zone"
    );
    assert!(
        scripted.commands().is_empty(),
        "the identical pass neither polls nor rereads the Run"
    );

    let los_angeles = IanaZoneRules::parse("America/Los_Angeles").unwrap();
    let counted_los_angeles = CountingZone::new(&los_angeles);
    assert!(
        view.maintain_presentation_with_rules(&locale, &los_angeles, &counted_los_angeles,),
        "a new device zone invalidates the retained wall projection"
    );
    assert_eq!(
        counted_los_angeles.wall_calls.get(),
        1,
        "a zone change reprojects exactly once"
    );
    assert_eq!(
        view.presentation_counts,
        PresentationCounts {
            locale_clones: 1,
            zone_clones: 2,
        },
        "only the changed zone is cloned into the cache"
    );
    let local_time =
        locale.date_time(crate::format::parts_in_zone(occurrence_ms, &los_angeles).unwrap());
    assert_ne!(utc_time, local_time, "the fixture crosses a zone boundary");
    let local_lines = view.surface_mut().lines();
    let provenance: Vec<&String> = local_lines
        .iter()
        .filter(|line| line.contains(&schedule.to_storage()))
        .collect();
    assert_eq!(provenance.len(), 1, "reprojection replaces the one row");
    assert!(provenance[0].contains(&local_time));
    assert!(!provenance[0].contains(&utc_time));

    assert!(
        !view.maintain_presentation_with_rules(&locale, &los_angeles, &counted_los_angeles,),
        "an unchanged maintenance pass must not request another reflow"
    );
    assert_eq!(counted_los_angeles.wall_calls.get(), 1);
    assert_eq!(
        view.surface_mut()
            .lines()
            .iter()
            .filter(|line| line.contains(&schedule.to_storage()))
            .count(),
        1,
        "maintenance never duplicates provenance"
    );
    assert!(
        scripted.commands().is_empty(),
        "timezone reprojection neither polls nor rereads the Run"
    );
}

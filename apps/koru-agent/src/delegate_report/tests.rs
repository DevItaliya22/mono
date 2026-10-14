// SPDX-License-Identifier: MPL-2.0

//! The diagnostic, against ledgers on the disk and **no worker at all**.
//!
//! Every scenario writes `runs.db` and `artifacts.db` under a temporary
//! root named for this process, closes them, and renders. Nothing here
//! starts a worker, dials an endpoint or reaches the channel's own root —
//! which is the property the command exists for, asserted rather than
//! described.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use koru_artifact_core::{
    AttemptGeneration, NewAuthority, RunAuthorityRepository, RunWorkspace, TokenHash,
};
use koru_delegate_core::{
    ActorRef, DelegationRequest, DeviceId, DocumentVersionId, Objective, ProfileId,
    PublicationKind, RelativeFolder, RunContextItem, RunId, SessionProfileKey, WorkspaceId,
    WorkspaceScope,
};
use koru_delegate_sqlite::{AttemptPhase, RunsDb};

use super::*;

/// Text no real objective carries, so a leak is unmistakable.
const OBJECTIVE_SENTINEL: &str = "OBJECTIVE-SENTINEL summarise the board pack";

const STOPPED: &str = "not serving — the numbers below are the ledger's own";
/// The instant every scenario reads its ledger at. The fixtures are
/// written by the ledger's own system clock, so the report is rendered
/// against the same clock rather than a made-up one; what is asserted is
/// the shape of each line, never a particular age.
fn now() -> i64 {
    super::now_ms()
}

// ------------------------------------------------------------- fixtures

/// A temporary channel root: `<root>/store` is the store root, so
/// `runs_db_path` and `artifacts_root_beside` place the two files exactly
/// where a `--root R` stack would.
struct Root(PathBuf);

impl Root {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "koru-delegate-report-{}-{label}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn store(&self) -> PathBuf {
        self.0.join("store")
    }

    fn ledger(&self) -> PathBuf {
        koru_delegate_sqlite::runs_db_path(&self.store())
    }

    fn runs_db(&self) -> RunsDb {
        RunsDb::open(&self.ledger()).unwrap()
    }

    fn artifacts_dir(&self) -> PathBuf {
        koru_channel::artifacts_root_beside(&self.store())
    }

    fn render(&self, target: Option<&str>) -> String {
        render(&self.store(), STOPPED, now(), target).expect("the ledger reads")
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).ok();
    }
}

fn v4(n: u8) -> uuid::Uuid {
    format!("00000000-0000-4000-8000-0000000000{n:02x}")
        .parse()
        .unwrap()
}

fn run_id(n: u8) -> RunId {
    RunId::from_uuid(v4(n))
}

fn workspace(n: u8) -> WorkspaceId {
    WorkspaceId::from_uuid(v4(n))
}

/// A request whose objective and folder are text no rendering may carry.
fn request(n: u8, objective: &str, folder: &str) -> DelegationRequest {
    DelegationRequest {
        run: run_id(n),
        requested_by: ActorRef::Person {
            profile: ProfileId::from_uuid(v4(0xa0)),
        },
        objective: Objective::new(objective).unwrap(),
        agent: SessionProfileKey::BuiltIn("codex".parse().unwrap()),
        requested_model: None,
        scope: WorkspaceScope::ProjectFolder {
            project: v4(0xa1).hyphenated().to_string().parse().unwrap(),
            folder: RelativeFolder::new(folder).unwrap(),
        },
        context: vec![RunContextItem::Document {
            workspace: workspace(0xa2),
            document: koru_delegate_core::DocumentId::from_uuid(v4(0xa3)),
            version: DocumentVersionId::from_uuid(v4(0xa4)),
        }],
        grants: BTreeSet::from([workspace(0xa2)]),
        executing_device: DeviceId::from_uuid(v4(0xa5)),
        origin: Default::default(),
    }
}

fn created(db: &mut RunsDb, n: u8) -> RunId {
    db.record_intent(
        &request(n, OBJECTIVE_SENTINEL, "src/private"),
        workspace(0xa2),
    )
    .unwrap();
    db.satisfy_intent(
        run_id(n),
        koru_delegate_core::DocumentId::from_uuid(v4(n.wrapping_add(0x40))),
    )
    .unwrap();
    run_id(n)
}

fn running(db: &mut RunsDb, run: RunId, n: u8) -> koru_delegate_sqlite::AttemptRow {
    let generation = db.open_attempt(run).unwrap().generation;
    db.advance(run, generation, AttemptPhase::Starting).unwrap();
    db.bind_session(
        run,
        generation,
        koru_delegate_core::ProviderSessionKey {
            idempotency: v4(n).hyphenated().to_string().parse().unwrap(),
            session: None,
        },
        koru_delegate_core::HostInstanceId::from_uuid(v4(0xd0)),
    )
    .unwrap();
    db.advance(run, generation, AttemptPhase::Running).unwrap()
}

/// One run authority, so the disk report has a folder to walk.
fn issue_authority(root: &Root, run: RunId, workspace_path: &std::path::Path, token: u8) {
    fs::create_dir_all(workspace_path).unwrap();
    let mut repository = koru_artifact_sqlite::Repository::open(&root.artifacts_dir()).unwrap();
    repository
        .issue_authority(NewAuthority {
            run,
            workspace: workspace(0xa2),
            generation: AttemptGeneration::FIRST,
            grants: BTreeSet::new(),
            run_workspace: RunWorkspace::new(workspace_path).unwrap(),
            token_hash: TokenHash::from_hex(format!("{token:02x}").repeat(32)).unwrap(),
            created_at_ms: 1,
        })
        .unwrap();
}

// ---------------------------------------------------------- the scenarios

/// The strong one: a ledger a worker wrote and then stopped, read whole.
#[test]
fn the_whole_report_reads_with_no_worker_running() {
    let root = Root::new("stopped");
    let mut db = root.runs_db();
    let live = created(&mut db, 1);
    let stuck = created(&mut db, 2);
    running(&mut db, live, 1);
    let generation = running(&mut db, stuck, 2).generation;
    db.advance(stuck, generation, AttemptPhase::Unknown)
        .unwrap();
    db.record_intent(&request(3, OBJECTIVE_SENTINEL, "docs"), workspace(0xa2))
        .unwrap();
    db.enqueue(
        koru_delegate_core::EventId(v4(1)),
        live,
        PublicationKind::ArtifactRegistration,
        "{}",
    )
    .unwrap();
    let schedule_publication = koru_delegate_sqlite::SchedulePublicationRef {
        schedule: koru_delegate_core::ScheduleId::from_uuid(v4(0x61)),
        workspace: workspace(0xa2),
        document: koru_delegate_core::DocumentId::from_uuid(v4(0x62)),
    };
    db.prepare_schedule_publication(
        koru_delegate_core::EventId(v4(0x63)),
        schedule_publication,
        koru_delegate_sqlite::SchedulePublicationFact::Created { revision: 1 },
        "{}",
    )
    .unwrap();
    db.mark_schedule_publication_ready(koru_delegate_core::EventId(v4(0x63)))
        .unwrap();
    db.prepare_schedule_publication(
        koru_delegate_core::EventId(v4(0x64)),
        schedule_publication,
        koru_delegate_sqlite::SchedulePublicationFact::Updated { revision: 2 },
        "{}",
    )
    .unwrap();
    db.claim_slot(
        koru_delegate_core::ScheduleId::from_uuid(v4(0x51)),
        1_700_000_000_000,
        4,
    )
    .unwrap();
    let fired = koru_delegate_core::ScheduleId::from_uuid(v4(0x52));
    db.claim_slot(fired, 1_700_000_060_000, 5).unwrap();
    db.record_outcome(
        fired,
        1_700_000_060_000,
        koru_delegate_sqlite::SlotOutcome::Fired,
    )
    .unwrap();
    let missed_planned = run_id(0xe1);
    db.claim_skip(
        koru_delegate_core::ScheduleId::from_uuid(v4(0x53)),
        1_700_000_120_000,
        6,
        missed_planned,
        koru_delegate_sqlite::SlotOutcome::SkippedMissed,
        None,
    )
    .unwrap();
    let busy_planned = run_id(0xe2);
    let busy_holder = run_id(0xe3);
    db.claim_skip(
        koru_delegate_core::ScheduleId::from_uuid(v4(0x54)),
        1_700_000_180_000,
        7,
        busy_planned,
        koru_delegate_sqlite::SlotOutcome::SkippedBusy,
        Some(busy_holder),
    )
    .unwrap();
    let retired = koru_delegate_core::ScheduleId::from_uuid(v4(0x55));
    db.claim_slot(retired, 1_700_000_240_000, 8).unwrap();
    drop(db);
    rusqlite::Connection::open(root.ledger())
        .unwrap()
        .execute(
            "UPDATE schedule_claims SET outcome = 'skipped_retired'
              WHERE schedule_id = ?1 AND occurrence_ms = 1700000240000",
            [retired.to_storage()],
        )
        .unwrap();

    let rendered = root.render(None);
    assert!(rendered.contains(STOPPED), "{rendered}");
    assert!(rendered.contains("1 live, 1 unknown"), "{rendered}");
    assert!(rendered.contains("1 pending"), "{rendered}");
    assert!(rendered.contains("1 open, oldest waiting"), "{rendered}");
    assert!(
        rendered.contains("schedule queue 1 prepared") && rendered.contains("1 ready"),
        "{rendered}"
    );
    // Every section a person acts on, by name.
    assert!(rendered.contains(&live.to_string()), "{rendered}");
    assert!(rendered.contains(&stuck.to_string()), "{rendered}");
    assert!(rendered.contains("<- needs a person"), "{rendered}");
    assert!(
        rendered.contains("requests that are not yet runs"),
        "{rendered}"
    );
    assert!(rendered.contains("artifact_registration"), "{rendered}");
    assert!(
        rendered.contains("schedule slots claimed and not settled"),
        "{rendered}"
    );
    assert!(
        rendered.contains("1 fired, 1 missed, 1 busy, 1 retired, 1 unsettled"),
        "{rendered}"
    );
    assert!(rendered.contains("recent schedule skips"), "{rendered}");
    assert!(rendered.contains("newest claim first"), "{rendered}");
    assert!(rendered.contains("reason skipped_missed"), "{rendered}");
    assert!(rendered.contains("reason skipped_busy"), "{rendered}");
    assert!(rendered.contains("reason skipped_retired"), "{rendered}");
    assert!(
        rendered.contains(&format!("busy holder {busy_holder}")),
        "{rendered}"
    );
    assert!(
        !rendered.contains(&missed_planned.to_string()),
        "{rendered}"
    );
    assert!(!rendered.contains(&busy_planned.to_string()), "{rendered}");
    // Nothing has registered here, so there is no artifacts.db to walk and
    // the report says so rather than failing.
    assert!(
        rendered.contains("no run has registered on this root"),
        "{rendered}"
    );
}

/// D37's disk, by run id and by nothing else.
#[test]
fn the_disk_held_by_retained_results_is_reported_by_run() {
    let root = Root::new("disk");
    let mut db = root.runs_db();
    let run = created(&mut db, 1);
    let quiet = created(&mut db, 2);
    running(&mut db, run, 1);
    drop(db);

    let workspace_path = root.0.join("checkout");
    issue_authority(&root, run, &workspace_path, 0x11);
    issue_authority(&root, quiet, &root.0.join("other"), 0x22);
    let folder = koru_artifact_core::artifacts_folder(&workspace_path, run);
    fs::create_dir_all(folder.join("nested")).unwrap();
    fs::write(folder.join("result.md"), vec![b'a'; 2_048]).unwrap();
    fs::write(folder.join("nested").join("more.bin"), vec![b'b'; 1_024]).unwrap();

    let rendered = root.render(None);
    let line = rendered
        .lines()
        .find(|line| line.contains(&run.to_string()) && line.contains("file(s)"))
        .unwrap_or_else(|| panic!("no disk line for the run: {rendered}"));
    assert!(line.contains("2 file(s)"), "{line}");
    assert!(line.contains("3.0 KB"), "{line}");
    // The run that wrote nothing has no line, and no total line is printed
    // for a single measured run.
    assert!(
        !rendered.contains(&format!("{quiet} ")),
        "a run holding nothing was listed: {rendered}"
    );
    // The folder is never named — only the run that holds it.
    assert!(!rendered.contains("checkout"), "{rendered}");
    assert!(!rendered.contains(".artifacts"), "{rendered}");
}

/// Emptiness is an answer, and so is a root nothing has been delegated on.
#[test]
fn an_empty_ledger_and_an_absent_one_both_answer() {
    let root = Root::new("empty");
    drop(root.runs_db());
    let rendered = root.render(None);
    assert!(rendered.contains("Nothing outstanding"), "{rendered}");
    assert!(rendered.contains("0 live, 0 unknown"), "{rendered}");

    // No ledger at all is an answer with the worker line still on it, not
    // a failure: an idle machine must not read as a broken one.
    let bare = Root::new("bare");
    let rendered = bare.render(None);
    assert!(
        rendered.contains("nothing has been delegated on this root"),
        "{rendered}"
    );
    assert!(rendered.contains(STOPPED), "{rendered}");
    assert!(!bare.ledger().exists(), "reporting created a ledger");
    assert!(
        bare.render(Some(&run_id(1).to_string()))
            .contains("nothing has been delegated on this root")
    );
    // And an idle root still refuses an argument it does not understand,
    // rather than letting a typo read as accepted.
    assert_eq!(
        render(&bare.store(), STOPPED, now(), Some("nonsense")).expect_err("refused"),
        "nonsense is not a run id"
    );
}

/// A ledger a newer build stamped is refused with the two numbers, and
/// nothing about it is changed.
#[test]
fn a_newer_ledger_refuses_cleanly() {
    let root = Root::new("newer");
    fs::create_dir_all(root.ledger().parent().unwrap()).unwrap();
    rusqlite::Connection::open(root.ledger())
        .unwrap()
        .execute_batch("PRAGMA journal_mode = DELETE; PRAGMA user_version = 99;")
        .unwrap();
    let error = render(&root.store(), STOPPED, now(), None).expect_err("refused");
    assert!(error.contains("99"), "{error}");
    let mode: String = rusqlite::Connection::open(root.ledger())
        .unwrap()
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mode.to_lowercase(), "delete");
}

/// The per-run form, the way `projection <id>` prints one document.
#[test]
fn one_run_prints_its_generations_and_an_unknown_id_says_so() {
    let root = Root::new("one");
    let mut db = root.runs_db();
    let run = created(&mut db, 7);
    let first = running(&mut db, run, 7).generation;
    db.advance(run, first, AttemptPhase::Interrupted).unwrap();
    db.retire(run, first).unwrap();
    running(&mut db, run, 8);
    drop(db);

    let rendered = root.render(Some(&run.to_string()));
    assert!(
        rendered.contains(&format!("run        {run}")),
        "{rendered}"
    );
    assert!(rendered.contains("gen 1"), "{rendered}");
    assert!(rendered.contains("gen 2"), "{rendered}");
    assert!(rendered.contains("interrupted"), "{rendered}");
    assert!(rendered.contains(", retired"), "{rendered}");

    let stranger = root.render(Some(&run_id(0x5f).to_string()));
    assert!(
        stranger.contains("is not a run this device created"),
        "{stranger}"
    );

    let refused = render(&root.store(), STOPPED, now(), Some("not-an-id")).expect_err("refused");
    assert_eq!(refused, "not-an-id is not a run id");
}

/// §13.7, structurally: the store's eight-language fixtures and a sentinel
/// objective go into the ledger, the scope, the run's folder name and the
/// names of the files it retained — and none of them is anywhere in what a
/// person sees.
#[test]
fn nothing_a_person_wrote_reaches_the_rendering() {
    let root = Root::new("privacy");
    let samples = koru_store_core::fixtures::SAMPLES;
    let filename = koru_store_core::fixtures::UNICODE_FILENAME;

    let mut db = root.runs_db();
    // The objective and the scope folder carry fixture text.
    let folder = format!("{}/{}", samples[5].title, samples[7].title);
    db.record_intent(
        &request(
            1,
            &format!("{OBJECTIVE_SENTINEL} {}", samples[0].body),
            &folder,
        ),
        workspace(0xa2),
    )
    .unwrap();
    db.satisfy_intent(
        run_id(1),
        koru_delegate_core::DocumentId::from_uuid(v4(0x41)),
    )
    .unwrap();
    let run = run_id(1);
    running(&mut db, run, 1);
    // A publication body carries fixture text too.
    db.enqueue(
        koru_delegate_core::EventId(v4(1)),
        run,
        PublicationKind::Lifecycle,
        &serde_json::to_string(&serde_json::json!({ "note": samples[6].body })).unwrap(),
    )
    .unwrap();
    drop(db);

    // The run's working folder is named for a fixture, and the files it
    // retained are named for the Unicode filename fixture.
    let workspace_path = root.0.join(samples[3].title);
    issue_authority(&root, run, &workspace_path, 0x11);
    let artifacts = koru_artifact_core::artifacts_folder(&workspace_path, run);
    fs::create_dir_all(&artifacts).unwrap();
    fs::write(artifacts.join(filename), b"bytes").unwrap();

    let rendered = format!(
        "{}{}",
        root.render(None),
        root.render(Some(&run.to_string()))
    );
    let mut sentinels = koru_store_core::fixtures::all_strings();
    sentinels.push(OBJECTIVE_SENTINEL);
    sentinels.push(filename);
    for sentinel in sentinels {
        assert!(
            !rendered.contains(sentinel),
            "{sentinel:?} reached the report:\n{rendered}"
        );
    }
    // It did report the disk, which is the point of asserting the rest.
    assert!(rendered.contains("1 file(s)"), "{rendered}");
}

// ------------------------------------------------------------ formatting

#[test]
fn ages_and_sizes_read_the_same_on_every_machine() {
    assert_eq!(age_of(-5_000), "0s");
    assert_eq!(age_of(0), "0s");
    assert_eq!(age_of(41_000), "41s");
    assert_eq!(age_of(3_599_000), "59m 59s");
    assert_eq!(age_of(3_600_000), "1h 00m");
    assert_eq!(age_of(90_000_000), "1d 01h");

    assert_eq!(bytes_of(0), "0 B");
    assert_eq!(bytes_of(1_023), "1023 B");
    assert_eq!(bytes_of(1_024), "1.0 KB");
    assert_eq!(bytes_of(1_048_576), "1.0 MB");
    assert_eq!(bytes_of(2_147_483_648), "2.00 GB");
}

/// The counter block is the worker's own answer and is printed whole, in
/// tokens and numbers.
#[test]
fn the_counter_block_names_every_number_and_carries_no_content() {
    let block = counter_lines(&WorkerCounters {
        intents_recorded: 4,
        attempts_opened: 5,
        contexts_captured: 6,
        captures_failed: 1,
        publications_queued: 9,
        artifacts_registered: 2,
        publications_acknowledged: 8,
        publication_retries: 3,
        restarts: 1,
        restart_completed_forward: 1,
        restart_started: 0,
        restart_reattached: 2,
        restart_reconciled: 0,
        restart_interrupted: 3,
        restart_unresolved: 0,
        schedule_wakes: 12,
        schedule_evaluations: 9,
        schedule_documents_evaluated: 27,
        schedule_evaluation_failures: 2,
        schedule_fired: 4,
        schedule_skipped_missed: 3,
        schedule_skipped_busy: 1,
    });
    for expected in [
        "requests recorded      4",
        "attempts opened        5",
        "contexts captured      6",
        "captures failed        1",
        "results registered     2",
        "9 queued, 8 acknowledged, 3 retried",
        "1 forward, 0 started, 2 reattached, 0 reconciled",
        "3 interrupted, 0 unresolved",
        "12 wakes, 9 passes, 27 documents, 2 failures",
        "4 fired, 3 missed, 1 busy",
    ] {
        assert!(
            block.contains(expected),
            "{expected:?} missing from\n{block}"
        );
    }
}

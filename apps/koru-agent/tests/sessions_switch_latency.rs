// SPDX-License-Identifier: MPL-2.0

//! **SP-0's latency baseline: what a raw session switch costs today.**
//!
//! ```sh
//! cargo test -p koru-agent --test sessions_switch_latency -- --nocapture --test-threads=1
//! ```
//!
//! ## What is being measured
//!
//! Exactly the tab-switch sequence a session host client performs (first
//! written for the deleted Terminal app's window; koru.exe's session tabs do
//! the same), and nothing else: **detach the current session, attach the next, request the
//! window's grid, and wait for the first snapshot that is for the right session
//! at the right size.** That last clause is the whole of the measurement — a
//! delta carries no `size` and can never end the wait, so what is timed is the
//! interval during which the previous session's pixels are still on screen
//! under a title that has already changed. Anything shorter would be measuring
//! a round trip; anything longer would be measuring a GPU.
//!
//! It runs headless, through `koru-session-client` against real
//! `koru-agent.exe --worker sessions` child processes on real PTYs, and it **needs no
//! environment flag**. SP-0 preferred that to instrumenting the app: a flag is
//! a permanent surface in `koru-env`'s registry, and a baseline that has to be
//! switched on is a baseline nobody reproduces.
//!
//! ## What is asserted, and what is only printed
//!
//! `apps/koru-store-server/tests/perf.rs` set the rule and it holds here: a
//! wall-clock threshold is a promise about the machine running the test, so it
//! passes on a laptop, fails on a loaded CI box, and gets raised until it
//! asserts nothing. **The assertions here are shapes** — switching does not get
//! dearer the more you switch, and the tail is a tail rather than a second
//! mode. The numbers are printed for the owner to set a bound against; SP-0
//! proposes one and does not claim it is accepted.
//!
//! The fixture is `tests/sessions_ipc.rs`'s, copied rather than shared: an
//! integration test is its own binary, and a shared extraction is a change to a
//! file `sessions_ipc.rs` owns. SP-0 returns artifacts, so it keeps to its own
//! file.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::str::FromStr;
use std::time::{Duration, Instant};

use koru_session_client::{ClientError, SessionClient, Subscription};
use koru_session_protocol::{
    BuiltInProfileKey, CapabilitySet, GridSize, IdempotencyKey, ProfileKind, RawCommand,
    RenderUpdate, SessionCommand, SessionId, SessionMode, SessionProfileKey, ViewportGeneration,
};
use koru_session_wire::{CreateSession, EnvironmentSpec, LaunchBootstrap, SubscriptionUpdate};

/// Fifty, per the packet. Above `koru-voice-bench`'s `MINIMUM_N` of 30, which
/// is the tree's own floor for printing a quantile at all.
const RUNS: usize = 50;

/// The grid both sessions are created at and switched to. A real window
/// switches between two sessions it has already sized, so the resize the
/// Terminal issues on every `Attached` is a no-op at the emulator — and that is
/// the honest common case, not a contrived one.
const COLUMNS: u16 = 100;
const ROWS: u16 = 30;
const OVERSCAN: u16 = 2;

// ------------------------------------------------------------- the fixture
// Copied from `tests/sessions_ipc.rs`; see the module note.

struct Host {
    child: Child,
    bootstrap_pipe: Option<std::process::ChildStdin>,
    endpoint: koru_ipc::Endpoint,
    authority: String,
}

impl Host {
    fn start(root: &Path) -> Self {
        std::fs::create_dir_all(root).unwrap();
        let launch_id = uuid::Uuid::new_v4().to_string();
        let authority = uuid::Uuid::new_v4().to_string();
        let bootstrap = LaunchBootstrap::new(
            launch_id.clone(),
            koru_channel::segment().into(),
            root.to_path_buf(),
            root.join("agent/sessions"),
            root.join("store"),
            true,
            authority.clone(),
        );
        let mut command = Command::new(PathBuf::from(env!("CARGO_BIN_EXE_koru-agent")));
        command
            .args(["--worker", "sessions", "--root"])
            .arg(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        let mut child = command.spawn().unwrap();
        let mut bootstrap_pipe = child.stdin.take().unwrap();
        bootstrap_pipe
            .write_all(&serde_json::to_vec(&bootstrap).unwrap())
            .unwrap();
        bootstrap_pipe.write_all(b"\n").unwrap();
        bootstrap_pipe.flush().unwrap();
        let endpoint = koru_ipc::Endpoint::for_service_channel_root(
            koru_ipc::SESSION_HOST,
            koru_channel::segment(),
            root,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match client(&endpoint, "koru-session-switch-latency") {
                Ok(client) => {
                    let health = client.health().unwrap();
                    assert_eq!(health.supervisor_launch_id, launch_id);
                    break;
                }
                Err(_) if Instant::now() < deadline => {
                    assert!(
                        child.try_wait().unwrap().is_none(),
                        "host exited during startup"
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => panic!("host did not start: {error}"),
            }
        }
        Self {
            child,
            bootstrap_pipe: Some(bootstrap_pipe),
            endpoint,
            authority,
        }
    }

    fn client(&self, product: &str) -> SessionClient {
        client(&self.endpoint, product).unwrap()
    }

    fn stop(mut self) {
        let _ = self
            .client("koru-agent-supervisor")
            .stop_host(self.authority.clone());
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.child.try_wait().unwrap().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        self.bootstrap_pipe.take();
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn client(endpoint: &koru_ipc::Endpoint, product: &str) -> Result<SessionClient, ClientError> {
    SessionClient::connect(endpoint, product, koru_session_host::protocol_offer())
}

/// The cheapest session that stays alive: a platform shell holding still. No
/// output means no deltas competing with the snapshot we are waiting for.
fn create(root: &Path, name: &str) -> CreateSession {
    #[cfg(windows)]
    let argv = vec![
        "powershell.exe".into(),
        "-NoLogo".into(),
        "-NoProfile".into(),
        "-NonInteractive".into(),
        "-Command".into(),
        "Start-Sleep -Seconds 120".to_owned(),
    ];
    #[cfg(unix)]
    let argv = vec!["/bin/sh".into(), "-c".into(), "sleep 120".to_owned()];
    CreateSession {
        profile: SessionProfileKey::BuiltIn(BuiltInProfileKey::from_str("platform-shell").unwrap()),
        parent_session_id: None,
        profile_kind: ProfileKind::Shell,
        display_name: name.into(),
        working_directory: root.to_path_buf(),
        mode: SessionMode::RawPty,
        attachment_delivery: koru_session_protocol::AttachmentDeliveryPolicy::Unsupported,
        capabilities: CapabilitySet::default(),
        argv,
        environment: EnvironmentSpec::Inherit,
        size: GridSize::new(COLUMNS, ROWS).unwrap(),
        project_id: None,
    }
}

// -------------------------------------------------------------- the measure

fn target() -> GridSize {
    GridSize::new(COLUMNS, ROWS).unwrap()
}

/// Wait for the first snapshot that is **for this session at this size**, the
/// exact gate `window.rs` applies. Returns `None` if the stream closes or the
/// deadline passes; a `Delta` or a `Gap` never satisfies it, because neither
/// carries a size and both belong to the old grid.
fn first_sized_snapshot(
    subscription: &Subscription,
    session: SessionId,
    size: GridSize,
) -> Option<Duration> {
    let began = Instant::now();
    let deadline = began + Duration::from_secs(5);
    while Instant::now() < deadline {
        match subscription.recv_timeout(Duration::from_millis(250)) {
            Ok(SubscriptionUpdate::Render {
                update: RenderUpdate::Snapshot { snapshot },
            }) if snapshot.session_id == session && snapshot.size == size => {
                return Some(began.elapsed());
            }
            Ok(SubscriptionUpdate::Closed { .. }) => return None,
            Ok(_) | Err(ClientError::TimedOut) => {}
            Err(_) => return None,
        }
    }
    None
}

/// One switch: detach the current session, attach the next, request the
/// window's grid, and wait for the frame that is allowed to be presented.
#[allow(clippy::too_many_arguments)]
fn switch(
    client: &SessionClient,
    from: Option<(SessionId, koru_session_protocol::ControllerLeaseToken)>,
    to: SessionId,
    generation: u64,
) -> (
    Duration,
    koru_session_protocol::ControllerLeaseToken,
    Subscription,
) {
    let began = Instant::now();
    if let Some((session, lease)) = from {
        client.detach(session, lease).expect("detach");
    }
    let (lease, subscription) = client
        .attach_live(
            to,
            ViewportGeneration::new(generation).unwrap(),
            COLUMNS,
            OVERSCAN,
        )
        .expect("attach");
    // The Terminal issues this on every `Attached`, unconditionally, because
    // `attach_live` clamps columns to the grid the emulator still owns.
    client
        .command(SessionCommand {
            idempotency_key: IdempotencyKey::mint(),
            session_id: to,
            controller_lease: lease.token,
            action: RawCommand::Resize { size: target() },
        })
        .expect("resize");
    first_sized_snapshot(&subscription, to, target()).expect("a correctly-sized snapshot");
    (began.elapsed(), lease.token, subscription)
}

// ------------------------------------------------------------- the reporting

/// Order statistics with no interpolation, matching `koru-voice-bench`'s
/// `percentile`: a quantile here is an observation that happened, not an
/// average of two that did not.
fn quantile(sorted: &[Duration], q: f64) -> Duration {
    let index = ((sorted.len() as f64 - 1.0) * q).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

fn ms(value: Duration) -> f64 {
    value.as_secs_f64() * 1_000.0
}

fn report(what: &str, samples: &mut [Duration]) {
    samples.sort();
    println!(
        "  {what:<34} n={n:<4} min {min:>7.2}  p50 {p50:>7.2}  p95 {p95:>7.2}  \
         p99 {p99:>7.2}  max {max:>7.2}  (ms)",
        n = samples.len(),
        min = ms(samples[0]),
        p50 = ms(quantile(samples, 0.50)),
        p95 = ms(quantile(samples, 0.95)),
        p99 = ms(quantile(samples, 0.99)),
        max = ms(samples[samples.len() - 1]),
    );
}

// ------------------------------------------------------------------ the test

/// The baseline. Prints the distribution; asserts only shapes.
#[test]
fn a_raw_session_switch_does_not_get_dearer_the_more_you_switch() {
    let root = tempfile::tempdir().unwrap();
    let host = Host::start(root.path());
    let client = host.client("koru-workspace-latency");

    let a = client.create(create(root.path(), "a")).expect("session a");
    let b = client.create(create(root.path(), "b")).expect("session b");

    // The cold attach: the first time this client has ever held a lease, with
    // no previous session to detach. Context only — see the assertions.
    let mut cold = Vec::new();
    let (taken, cold_lease, cold_subscription) = switch(&client, None, a.id, 1);
    cold.push(taken);

    // One discarded warm-up: the first switch pays for the second session's
    // first controller attachment, which no later switch repeats.
    let (_, mut lease, mut subscription) = switch(&client, Some((a.id, cold_lease)), b.id, 2);
    drop(cold_subscription);
    let mut current = b.id;

    let mut switches = Vec::with_capacity(RUNS);
    for run in 0..RUNS {
        let next = if current == a.id { b.id } else { a.id };
        let (taken, new_lease, new_subscription) =
            switch(&client, Some((current, lease)), next, 3 + run as u64);
        // The previous subscription is dropped here, which is what the window
        // does when it detaches: the queue is bounded at sixteen and a reader
        // that stops reading is closed rather than allowed to back up.
        drop(std::mem::replace(&mut subscription, new_subscription));
        lease = new_lease;
        current = next;
        switches.push(taken);
    }

    // Kept before `report` sorts, because the drift check below is about the
    // order the switches happened in, not about their distribution.
    let switches_in_order = switches.clone();
    println!("\nraw session switch — detach, attach, first correctly-sized snapshot");
    report("cold attach (context only)", &mut cold);
    report("switch", &mut switches);
    println!(
        "  {:<34} {COLUMNS}x{ROWS}, overscan {OVERSCAN}, platform-shell, no output\n",
        "grid"
    );

    let median = quantile(&switches, 0.50);
    let worst = switches[switches.len() - 1];

    // **Shapes, not milliseconds.**
    //
    // The first shape SP-0 tried was "a switch costs no more than the cold
    // attach it reuses", and measurement refused it: a switch is reliably
    // *dearer* than a cold attach, because it carries a `Detach` round trip and
    // the previous session's controller teardown that a first attach has no
    // equivalent of. That is a finding rather than a bug, and it is why the
    // proposed bound below is expressed against the switch's own distribution.
    //
    // What is asserted instead is the thing a fifty-run baseline is actually
    // for and the thing that is true on any machine: **the cost does not grow
    // with the number of switches.** A lease, a subscription or a scrollback
    // page retained per switch would show up here and nowhere else, and it
    // would show up before SP-10 rather than after it.
    let half = switches_in_order.len() / 2;
    let mut early: Vec<Duration> = switches_in_order[..half].to_vec();
    let mut late: Vec<Duration> = switches_in_order[half..].to_vec();
    early.sort();
    late.sort();
    let early_median = quantile(&early, 0.50);
    let late_median = quantile(&late, 0.50);
    println!(
        "  {:<34} first half p50 {:>7.2}   second half p50 {:>7.2}  (ms)\n",
        "drift",
        ms(early_median),
        ms(late_median)
    );
    assert!(
        late_median <= early_median * 2 + Duration::from_millis(10),
        "switching got dearer as it went: {:.2} ms in the first half, \
         {:.2} ms in the second — something is being retained per switch",
        ms(early_median),
        ms(late_median)
    );

    // And the tail is a tail, not a second mode. Ten times the median would
    // mean a switch sometimes waits on something the median never waits on.
    assert!(
        worst <= median * 10 + Duration::from_millis(50),
        "the tail ({:.2} ms) is a second mode beside the median ({:.2} ms)",
        ms(worst),
        ms(median)
    );
    assert_eq!(switches.len(), RUNS);

    drop(subscription);
    let _ = client.detach(current, lease);
    drop(client);
    host.stop();
}

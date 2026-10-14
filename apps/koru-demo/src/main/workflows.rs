// SPDX-License-Identifier: MPL-2.0

//! Runtime file ownership and Windows attachment; the ordinary suite uses the
//! pure fixture/capture/repetition modules and never executes this boundary.
use koru_demo::{
    cli::Options,
    execute::Executor,
    fixtures::{Environment, MAX_MANIFEST_BYTES, Manifest},
    inspect::{Handle, Request, Response},
    instance::{self, RootElement, Target},
    prepare::{Navigation, Reader},
    repeat,
    report::RunReport,
    supervise::{CancelToken, Limits, Outcome, Start, Supervisor},
    wait::{Clock, Patience, SystemClock},
};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
};
type Failure = (&'static str, &'static str);

fn persist(file: &mut File, report: &RunReport) -> Result<(), Failure> {
    let mut bytes = Vec::new();
    report
        .write_json(&mut bytes)
        .map_err(|_| ("report", "report-serialization-failed"))?;
    file.seek(SeekFrom::Start(0))
        .and_then(|_| file.write_all(&bytes))
        .and_then(|_| file.set_len(bytes.len() as u64))
        .and_then(|_| file.sync_all())
        .map_err(|_| ("report", "report-write-failed"))
}
fn read_manifest(path: &Path) -> Result<Vec<u8>, Failure> {
    let file = File::open(path).map_err(|_| ("manifest", "fixture-manifest-open-failed"))?;
    let mut bytes = Vec::new();
    file.take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ("manifest", "fixture-manifest-read-failed"))?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(("manifest", "fixture-manifest-too-large"));
    }
    Ok(bytes)
}
fn open_manifest(path: &Path, root: &Path) -> Result<Manifest, Failure> {
    Manifest::parse(&read_manifest(path)?, root).map_err(|code| ("manifest", code))
}

pub fn run(options: Options) -> ExitCode {
    let root = instance::resolved(&options.root);
    let cancel = CancelToken::new();
    if options.prepare {
        let result = koru_demo::console::install(&cancel)
            .map_err(|_| ("cancel", "cancel-handler-unavailable"))
            .and_then(|_| prepare(&options, &root, &cancel));
        return finish(result);
    }
    if options.sample_record || options.record {
        let result = koru_demo::console::install(&cancel)
            .map_err(|_| ("cancel", "cancel-handler-unavailable"))
            .and_then(|_| record_compiled(&options, &root, &cancel));
        return finish(result);
    }
    let mut report = RunReport::new(
        options.case.as_deref().unwrap(),
        &root,
        options.process_id,
        options.window_handle,
        options.repeat,
    );
    let mut file = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(options.report.as_ref().expect("CLI requires report"))
    {
        Ok(file) => file,
        Err(_) => {
            eprintln!("koru-demo: report-exists-or-cannot-be-created");
            return ExitCode::from(2);
        }
    };
    if let Err(failure) = persist(&mut file, &report) {
        return finish(Err(failure));
    }
    let result = koru_demo::console::install(&cancel)
        .map_err(|_| ("cancel", "cancel-handler-unavailable"))
        .and_then(|_| execute(&options, &root, &cancel, &mut file, &mut report));
    if let Err((stage, code)) = result {
        report.fail(stage, code);
    }
    if let Err(failure) = persist(&mut file, &report) {
        return finish(Err(failure));
    }
    if report.passed() {
        println!("koru-demo: passed; report saved");
        ExitCode::SUCCESS
    } else {
        eprintln!("koru-demo: failed; report saved");
        ExitCode::from(1)
    }
}
fn finish(result: Result<(), Failure>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err((stage, code)) => {
            eprintln!("koru-demo: {stage}: {code}");
            ExitCode::from(1)
        }
    }
}

fn bind(
    options: &Options,
    root: &Path,
    cancel: &CancelToken,
) -> Result<(Supervisor, Handle, u64), Failure> {
    let owner =
        koru_demo::input_owner::prove_input_target_checked(root, options.process_id, || {
            cancel.is_cancelled()
        })
        .map_err(|_| ("ownership", "root-owner-refused"))?;
    let start: Start = Arc::new(move |generation| {
        koru_demo::uia::Client::open(generation).map(|client| {
            Box::new(client.with_owner(owner.clone())) as Box<dyn koru_demo::supervise::Mechanism>
        })
    });
    let mut supervisor = Supervisor::new(start, Limits::default());
    let bound = match supervisor.call(
        Request::BindWindow {
            native_handle: options.window_handle,
        },
        cancel,
    ) {
        Outcome::Answered(Response::Bound(bound)) => *bound,
        Outcome::Cancelled => return Err(("bind", "cancelled")),
        _ => return Err(("bind", "window-bind-failed")),
    };
    let target = Target {
        root: root.to_owned(),
        process_id: options.process_id,
        window_handle: options.window_handle,
    };
    let element = RootElement {
        automation_id: bound.facts.locator.clone().unwrap_or_default(),
        class_name: bound.class_name,
        framework: bound.framework,
        process_id: bound.facts.process_id,
    };
    let identified = instance::identify(
        &target,
        root.is_dir(),
        instance::root_is_serving(root),
        &element,
    )
    .map_err(|_| ("identity", "instance-identity-refused"))?;
    Ok((supervisor, bound.facts.handle, identified.window_slot))
}

fn await_foreground(
    options: &Options,
    cancel: &CancelToken,
    clock: &dyn Clock,
) -> Result<(), Failure> {
    println!("koru-demo: awaiting-foreground; activate the named isolated Koru window");
    let started = clock.now();
    loop {
        if cancel.is_cancelled() {
            return Err(("foreground", "cancelled"));
        }
        if unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() }.0 as i64
            == options.window_handle
        {
            break;
        }
        if clock.now().duration_since(started) >= options.timeout {
            return Err(("foreground", "foreground-timeout"));
        }
        clock.rest(std::time::Duration::from_millis(50));
    }
    Ok(())
}

fn execute(
    options: &Options,
    root: &Path,
    cancel: &CancelToken,
    file: &mut File,
    report: &mut RunReport,
) -> Result<(), Failure> {
    let name = options.case.as_deref().unwrap();
    let fixtures = if let Some(path) = &options.manifest {
        let manifest = open_manifest(path, root)?;
        report.set_environment(
            &manifest.environment.ui_language,
            &manifest.environment.locale,
            &manifest.environment.hour_cycle,
            &manifest.environment.time_zone,
        );
        (1..=options.repeat)
            .map(|iteration| {
                manifest
                    .fixture(name, iteration)
                    .map_err(|code| ("manifest", code))
            })
            .collect::<Result<Vec<_>, _>>()?
    } else {
        vec![
            options
                .fixture
                .clone()
                .ok_or(("manifest", "fixture-missing"))?,
        ]
    };
    let (mut supervisor, handle, slot) = bind(options, root, cancel)?;
    let clock = SystemClock;
    await_foreground(options, cancel, &clock)?;
    let mut executor = Executor {
        supervisor: &mut supervisor,
        cancel,
        root: handle,
        window_slot: slot,
        window_handle: options.window_handle,
        process_id: options.process_id,
        patience: Patience {
            limit: options.timeout,
            ..Patience::default()
        },
        clock: &clock,
    };
    let mut report_error = None;
    repeat::run(name, &fixtures, report, |iteration, phase, case, report| {
        let mut results = Vec::new();
        for step in &case.steps {
            let result = executor.step(step);
            let failed = result.failure.is_some();
            println!(
                "run={iteration} phase={phase} step={} {}",
                step.id,
                result.failure.unwrap_or("passed")
            );
            results.push(result);
            report.checkpoint_case(iteration, phase, case, &results);
            if let Err(error) = persist(file, report) {
                report_error = Some(error);
                return Err("report-write-failed");
            }
            if failed {
                break;
            }
        }
        Ok(results)
    });
    drop(executor);
    if let Some(error) = report_error {
        return Err(error);
    }
    persist(file, report)?;
    // Run-end diagnostics, iteration 0. Only explicitly declared fixture IDs;
    // these do not claim to be snapshots of each earlier iteration.
    if options.snapshot_content {
        let mut reader = Reader {
            supervisor: &mut supervisor,
            cancel,
            root: handle,
            process_id: options.process_id,
            window_slot: slot,
        };
        let mut seen = std::collections::HashSet::new();
        for fixture in &fixtures {
            for pattern in std::iter::once(&fixture.subject).chain(fixture.entry.iter()) {
                if !seen.insert(format!("{pattern:?}")) {
                    continue;
                }
                let facts = reader
                    .one(pattern.clone())
                    .map_err(|code| ("snapshot", code))?;
                let snapshot = reader
                    .snapshot(facts.handle)
                    .map_err(|code| ("snapshot", code))?;
                report.add_snapshot(0, snapshot);
                persist(file, report)?;
            }
        }
    }
    Ok(())
}

fn record_compiled(options: &Options, root: &Path, cancel: &CancelToken) -> Result<(), Failure> {
    let stage = if options.record { "record" } else { "sample" };
    let clip = options.clip.as_ref().ok_or((stage, "clip-missing"))?;
    let actions_path = options
        .actions_out
        .as_ref()
        .ok_or((stage, "actions-missing"))?;
    let stills = options.stills.as_ref().ok_or((stage, "stills-missing"))?;
    let result_path = koru_demo::recording::result_path(stills, options.result_out.as_deref());
    koru_demo::recording::refuse_existing(&koru_demo::recording::Outputs {
        clip: clip.clone(),
        actions: actions_path.clone(),
        stills: stills.clone(),
        result: result_path.clone(),
    })
    .map_err(|code| (stage, code))?;
    let name = options.case.as_deref().unwrap();
    if options.sample_record && name != "document-edit" {
        return Err((stage, "unknown-case"));
    }
    let fixture = if let Some(path) = &options.manifest {
        open_manifest(path, root)?
            .fixture(name, 1)
            .map_err(|code| ("manifest", code))?
    } else {
        options
            .fixture
            .clone()
            .ok_or(("manifest", "fixture-missing"))?
    };
    let case = koru_demo::cases::compile(name, &fixture).map_err(|code| ("case", code))?;
    let (mut supervisor, handle, slot) = bind(options, root, cancel)?;
    let clock = SystemClock;
    await_foreground(options, cancel, &clock)?;
    let mut capture_error = None;
    let mut geometry = None;
    let mut session =
        match koru_demo::recording::capture::start(koru_demo::recording::capture::StartConfig {
            window_handle: options.window_handle,
            process_id: options.process_id,
            clip: clip.clone(),
            stills: Some(stills.clone()),
            cursor: false,
            frame_rate: None,
            bitrate_bps: None,
        }) {
            Ok((session, started)) => {
                geometry = Some(started);
                Some(session)
            }
            Err(code) => {
                capture_error = Some(code);
                None
            }
        };
    let mut origin = None;
    if let Some(session) = session.as_ref() {
        match session.wait_first_frame(koru_demo::recording::wait_timeout()) {
            Ok(ready) => origin = Some(ready.instant),
            Err(code) => capture_error = Some(code),
        }
    }
    let mut results = Vec::new();
    if capture_error.is_none() {
        let mut executor = Executor {
            supervisor: &mut supervisor,
            cancel,
            root: handle,
            window_slot: slot,
            window_handle: options.window_handle,
            process_id: options.process_id,
            patience: Patience {
                limit: options.timeout,
                ..Patience::default()
            },
            clock: &clock,
        };
        for step in &case.steps {
            println!("{stage} waiting step={}", step.id);
            let result = executor.step(step);
            let failed = result.failure.is_some();
            println!(
                "{stage} step={} {}",
                step.id,
                result.failure.unwrap_or("passed")
            );
            if let (Some(session), Some(origin), Some(delivered)) =
                (session.as_ref(), origin, result.delivered_at)
            {
                session.request_still(
                    step.id,
                    delivered.saturating_duration_since(origin).as_millis() as u64,
                );
            }
            results.push(result);
            if failed {
                break;
            }
        }
        drop(executor);
    }
    let mut frames = Vec::new();
    if let Some(session) = session.take() {
        clock.rest(std::time::Duration::from_millis(120));
        let (captured, finished) = session.finish();
        frames = captured;
        if let Err(code) = finished {
            capture_error = capture_error.or(Some(code));
        }
    }
    let clip_name = clip.to_string_lossy();
    let actions_name = actions_path.to_string_lossy();
    let conclusion = koru_demo::recording::conclude(koru_demo::recording::Attempt {
        case: name,
        process_id: options.process_id,
        window_handle: options.window_handle,
        geometry,
        frames: &frames,
        results: &results,
        origin,
        capture_error,
        clip: clip_name.as_ref(),
        actions: actions_name.as_ref(),
    });
    std::fs::create_dir_all(stills).map_err(|_| (stage, "stills-unwritable"))?;
    write_json_file(actions_path, &conclusion.actions_json)
        .map_err(|_| (stage, "actions-write-failed"))?;
    write_json_file(&result_path, &conclusion.manifest)
        .map_err(|_| (stage, "result-write-failed"))?;
    let stills_index: Vec<(String, String)> = results
        .iter()
        .filter(|result| stills.join(format!("{}.png", result.step)).is_file())
        .map(|result| (result.step.to_string(), format!("{}.png", result.step)))
        .collect();
    if let Some(artifact) = &conclusion.artifact {
        std::fs::write(stills.join("timeline.md"), artifact.timeline_markdown())
            .map_err(|_| (stage, "timeline-write-failed"))?;
        std::fs::write(
            stills.join("validation.html"),
            artifact.validation_html(&stills_index),
        )
        .map_err(|_| (stage, "validation-write-failed"))?;
    }
    write_recording_notes(stills, name, options, geometry.as_ref())
        .map_err(|_| (stage, "notes-write-failed"))?;
    if !conclusion.passed {
        return Err((stage, conclusion.failure.unwrap_or("case-failed")));
    }
    println!("koru-demo: {stage} wrote clip, actions and result");
    Ok(())
}

fn write_json_file(path: &Path, value: &serde_json::Value) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    serde_json::to_writer_pretty(&mut file, value).map_err(std::io::Error::other)?;
    file.write_all(b"\n")?;
    file.sync_all()
}

fn write_recording_notes(
    stills: &Path,
    name: &str,
    options: &Options,
    geometry: Option<&koru_demo::recording::CaptureGeometry>,
) -> std::io::Result<()> {
    let (width, height) = geometry
        .map(|geometry| (geometry.width, geometry.height))
        .unwrap_or((0, 0));
    std::fs::write(
        stills.join("NOTES.md"),
        format!(
            "# Recording notes\n\n\
             Contract `koru-demo-recording/1`. Times measure native input delivery, not application processing.\n\n\
             - Origin: first encoded frame (`origin.instant`), not capture-start.\n\
             - Codec requested: H.264. Audio disabled. Cursor disabled.\n\
             - `alignment_ms` is nearest-frame arrival minus event time on that origin.\n\
             - `capture_bounds` subtract the window top-left from UIA screen bounds.\n\
             - `title_bar_height` and `dpi_scale_milli` are recorded and not applied.\n\
             - Shortcut targets are the focused control.\n\
             - A resize or DPI change fails as `capture-geometry-changed` with partial artifacts.\n\
             - Frame intervals are listed; they are not treated as a constant frame rate.\n\
             - Timing acceptance from sample-20260912-000036: click 16 ms, chord/text 33 ms.\n\
             - Case: {name}. Window {hwnd}. Process {pid}. Capture {w}x{h}.\n",
            name = name,
            hwnd = options.window_handle,
            pid = options.process_id,
            w = width,
            h = height,
        ),
    )
}

fn prepare(options: &Options, root: &Path, cancel: &CancelToken) -> Result<(), Failure> {
    let path = options.manifest.as_ref().unwrap();
    let environment: Environment = options.environment.clone().unwrap();
    let previous = match File::open(path) {
        Ok(_) => Some(read_manifest(path)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err(("manifest", "fixture-manifest-open-failed")),
    };
    let mut manifest = match &previous {
        Some(bytes) => Manifest::parse(bytes, root).map_err(|code| ("manifest", code))?,
        None => Manifest::new(root, environment.clone()).map_err(|code| ("manifest", code))?,
    };
    if manifest.environment != environment {
        return Err(("manifest", "fixture-environment-mismatch"));
    }
    let name = options.case.as_deref().unwrap();
    if (name == "document-edit" && manifest.document_edit.is_some())
        || (name == "task-reminder" && manifest.task_reminder.is_some())
    {
        return Err(("manifest", "fixture-half-already-captured"));
    }
    let (mut supervisor, handle, slot) = bind(options, root, cancel)?;
    println!(
        "koru-demo: preparing fixture through visible focus and page navigation; keep the named document open"
    );
    let clock = SystemClock;
    await_foreground(options, cancel, &clock)?;
    Reader {
        supervisor: &mut supervisor,
        cancel,
        root: handle,
        process_id: options.process_id,
        window_slot: slot,
    }
    .capture(
        &mut manifest,
        name,
        options.capture_date.as_deref(),
        options.capture_time.as_deref(),
        &Navigation {
            window_handle: options.window_handle,
            patience: Patience {
                limit: options.timeout,
                ..Patience::default()
            },
            clock: &clock,
        },
    )
    .map_err(|code| ("capture", code))?;
    let bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|_| ("manifest", "fixture-serialization-failed"))?;
    if let Some(previous) = previous {
        if read_manifest(path)? != previous {
            return Err(("manifest", "fixture-manifest-changed"));
        }
        let mut temporary: PathBuf = path.as_os_str().to_owned().into();
        temporary.set_file_name(format!(
            "{}.next-{}",
            path.file_name()
                .and_then(|p| p.to_str())
                .ok_or(("manifest", "fixture-path-invalid"))?,
            std::process::id()
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| ("manifest", "fixture-staging-create-failed"))?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| ("manifest", "fixture-staging-write-failed"))?;
        drop(file);
        std::fs::rename(temporary, path).map_err(|_| ("manifest", "fixture-publish-failed"))?;
    } else {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|_| ("manifest", "fixture-manifest-exists-or-unwritable"))?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| ("manifest", "fixture-manifest-write-failed"))?;
    }
    println!("koru-demo: fixture half captured; manifest contains IDs and environment only");
    Ok(())
}

// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::fixtures::Environment;

fn error(message: &str) -> CliError {
    CliError {
        message: message.into(),
        suggestion: None,
    }
}

pub fn parse<I: IntoIterator<Item = String>>(arguments: I) -> Result<Options, CliError> {
    let mut source = arguments.into_iter().peekable();
    let prepare = source.peek().is_some_and(|s| s == "prepare");
    let sample_record = source.peek().is_some_and(|s| s == "sample-record");
    let record = source.peek().is_some_and(|s| s == "record");
    let mut base = Vec::new();
    if prepare {
        source.next();
        base.push("test".to_owned());
    }
    if sample_record {
        source.next();
        match source.peek().map(String::as_str) {
            Some(name) if crate::cases::names().contains(&name) => {}
            _ => return Err(error("sample-record document-edit is required")),
        }
        base.push("test".to_owned());
    }
    if record {
        source.next();
        match source.peek().map(String::as_str) {
            Some(name) if crate::cases::names().contains(&name) => {}
            _ => return Err(error("record <document-edit|task-reminder> is required")),
        }
        base.push("test".to_owned());
    }
    let mut manifest = None;
    let mut repeat = None;
    let mut report = None;
    let mut snapshot = false;
    let mut clip = None;
    let mut actions_out = None;
    let mut stills = None;
    let mut result_out = None;
    let (mut language, mut locale, mut cycle, mut zone) = (None, None, None, None);
    while let Some(token) = source.next() {
        let extra = matches!(
            token.as_str(),
            "--fixture"
                | "--repeat"
                | "--report"
                | "--snapshot-content"
                | "--ui-language"
                | "--locale"
                | "--hour-cycle"
                | "--time-zone"
                | "--clip"
                | "--actions"
                | "--stills"
                | "--result"
        );
        if !extra {
            let takes_value = FLAGS
                .iter()
                .find(|flag| flag.name == token)
                .is_some_and(|flag| flag.argument.is_some());
            base.push(token);
            if takes_value {
                if let Some(value) = source.next() {
                    base.push(value);
                }
            }
            continue;
        }
        if token == "--snapshot-content" {
            snapshot = true;
            continue;
        }
        let value = source
            .next()
            .ok_or_else(|| error("repeatability flag requires a value"))?;
        match token.as_str() {
            "--fixture" => {
                if manifest.replace(PathBuf::from(value)).is_some() {
                    return Err(error("duplicate --fixture"));
                }
            }
            "--report" => {
                if report.replace(PathBuf::from(value)).is_some() {
                    return Err(error("duplicate --report"));
                }
            }
            "--repeat" => {
                let count = value
                    .parse::<usize>()
                    .ok()
                    .filter(|n| (1..=10).contains(n))
                    .ok_or_else(|| error("--repeat must be 1..10"))?;
                if repeat.replace(count).is_some() {
                    return Err(error("duplicate --repeat"));
                }
            }
            "--ui-language" => {
                if language.replace(value).is_some() {
                    return Err(error("duplicate --ui-language"));
                }
            }
            "--locale" => {
                if locale.replace(value).is_some() {
                    return Err(error("duplicate --locale"));
                }
            }
            "--hour-cycle" => {
                if cycle.replace(value).is_some() {
                    return Err(error("duplicate --hour-cycle"));
                }
            }
            "--time-zone" => {
                if zone.replace(value).is_some() {
                    return Err(error("duplicate --time-zone"));
                }
            }
            "--clip" => {
                if clip.replace(PathBuf::from(value)).is_some() {
                    return Err(error("duplicate --clip"));
                }
            }
            "--actions" => {
                if actions_out.replace(PathBuf::from(value)).is_some() {
                    return Err(error("duplicate --actions"));
                }
            }
            "--stills" => {
                if stills.replace(PathBuf::from(value)).is_some() {
                    return Err(error("duplicate --stills"));
                }
            }
            "--result" => {
                if result_out.replace(PathBuf::from(value)).is_some() {
                    return Err(error("duplicate --result"));
                }
            }
            _ => unreachable!(),
        }
    }
    let mut options = parse_base(base, manifest.is_some())?;
    if prepare {
        if manifest.is_none() || !snapshot {
            return Err(error(
                "prepare requires --fixture and explicit --snapshot-content",
            ));
        }
        if repeat.is_some() || report.is_some() {
            return Err(error(
                "prepare does not execute cases or write a run report",
            ));
        }
        if clip.is_some() || actions_out.is_some() || stills.is_some() || result_out.is_some() {
            return Err(error("prepare does not record"));
        }
        let environment = Environment {
            ui_language: language.ok_or_else(|| error("prepare requires --ui-language en"))?,
            locale: locale.ok_or_else(|| error("prepare requires --locale"))?,
            hour_cycle: cycle.ok_or_else(|| error("prepare requires --hour-cycle"))?,
            time_zone: zone.ok_or_else(|| error("prepare requires --time-zone"))?,
        };
        environment.validate().map_err(error)?;
        if options.case.as_deref() == Some("task-reminder")
            && (options.capture_date.is_none() || options.capture_time.is_none())
        {
            return Err(error(
                "prepare task-reminder requires --reminder-date and --reminder-time",
            ));
        }
        options.environment = Some(environment);
    } else {
        if language.is_some() || locale.is_some() || cycle.is_some() || zone.is_some() {
            return Err(error("environment flags are recorded by prepare only"));
        }
        if options.case.is_some() {
            if sample_record || record {
                if report.is_some() {
                    return Err(error(
                        "record writes clip, action JSON and a result manifest, not a test --report",
                    ));
                }
                if repeat.is_some() {
                    return Err(error("record runs one case; use test for repetition"));
                }
                if sample_record && options.case.as_deref() != Some("document-edit") {
                    return Err(error("sample-record records document-edit only"));
                }
                if clip.is_none() || actions_out.is_none() || stills.is_none() {
                    return Err(error("record requires --clip, --actions and --stills"));
                }
                if record && result_out.is_none() {
                    return Err(error("record requires --result"));
                }
                if sample_record && result_out.is_some() {
                    return Err(error("--result requires record"));
                }
            } else if report.is_none() {
                return Err(error("test requires an explicit new --report file"));
            }
            if repeat.unwrap_or(1) > 1 && manifest.is_none() {
                return Err(error("repetition requires a captured --fixture manifest"));
            }
            if manifest.is_some()
                && (options.capture_date.is_some() || options.capture_time.is_some())
            {
                return Err(error("manifest reminder fields cannot be overridden"));
            }
        } else if manifest.is_some() || repeat.is_some() || report.is_some() || snapshot {
            return Err(error("fixture and report flags require test or prepare"));
        }
        if !sample_record
            && !record
            && (clip.is_some() || actions_out.is_some() || stills.is_some())
        {
            return Err(error(
                "--clip, --actions and --stills require record or sample-record",
            ));
        }
        if !record && result_out.is_some() {
            return Err(error("--result requires record"));
        }
    }
    if prepare && (sample_record || record) {
        return Err(error("prepare and record cannot be combined"));
    }
    if sample_record && record {
        return Err(error("sample-record and record cannot be combined"));
    }
    options.prepare = prepare;
    options.sample_record = sample_record;
    options.record = record;
    options.manifest = manifest;
    options.repeat = repeat.unwrap_or(1);
    options.report = report;
    options.snapshot_content = snapshot;
    options.clip = clip;
    options.actions_out = actions_out;
    options.stills = stills;
    options.result_out = result_out;
    Ok(options)
}

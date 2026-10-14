// SPDX-License-Identifier: MPL-2.0

//! Plan 4.3 recording outcome: never succeed if capture or finalization failed.

use super::{
    Artifact, CaptureGeometry, FrameStamp, from_results, unpaired_json, unpaired_manifest,
};
use crate::execute::StepResult;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outputs {
    pub clip: PathBuf,
    pub actions: PathBuf,
    pub stills: PathBuf,
    pub result: PathBuf,
}

pub fn refuse_existing_paths(
    clip_exists: bool,
    actions_exists: bool,
    result_exists: bool,
) -> Result<(), &'static str> {
    if clip_exists {
        return Err("clip-exists");
    }
    if actions_exists {
        return Err("actions-exists");
    }
    if result_exists {
        return Err("result-exists");
    }
    Ok(())
}

pub fn refuse_existing(outputs: &Outputs) -> Result<(), &'static str> {
    refuse_existing_paths(
        outputs.clip.exists(),
        outputs.actions.exists(),
        outputs.result.exists(),
    )
}

pub fn result_path(stills: &Path, explicit: Option<&Path>) -> PathBuf {
    explicit.map_or_else(|| stills.join("result.json"), Path::to_path_buf)
}

pub struct Attempt<'a> {
    pub case: &'a str,
    pub process_id: u32,
    pub window_handle: i64,
    pub geometry: Option<CaptureGeometry>,
    pub frames: &'a [FrameStamp],
    pub results: &'a [StepResult],
    pub origin: Option<Instant>,
    /// Encoder, capture-loss, or finalize error. Wins over a passing UI case.
    pub capture_error: Option<&'static str>,
    pub clip: &'a str,
    pub actions: &'a str,
}

pub struct Conclusion {
    pub passed: bool,
    pub failure: Option<&'static str>,
    pub artifact: Option<Artifact>,
    pub actions_json: Value,
    pub manifest: Value,
}

pub fn conclude(attempt: Attempt<'_>) -> Conclusion {
    let case_error = attempt.results.iter().find_map(|result| result.failure);
    let paired = match (attempt.origin, attempt.geometry) {
        (Some(origin), Some(geometry)) => from_results(
            attempt.case,
            attempt.process_id,
            attempt.window_handle,
            geometry,
            attempt.frames,
            attempt.results,
            origin,
        ),
        _ => Err(attempt.capture_error.unwrap_or("capture-not-ready")),
    };
    let pairing_error = paired.as_ref().err().copied();
    let failure = attempt.capture_error.or(pairing_error).or(case_error);
    match paired {
        Ok(artifact) => {
            let manifest =
                artifact.result_manifest(attempt.clip, attempt.actions, failure.is_none(), failure);
            Conclusion {
                passed: failure.is_none(),
                failure,
                actions_json: artifact.to_json(),
                artifact: Some(artifact),
                manifest,
            }
        }
        Err(_) => {
            let code = failure.unwrap_or("capture-not-ready");
            Conclusion {
                passed: false,
                failure: Some(code),
                artifact: None,
                actions_json: unpaired_json(
                    attempt.case,
                    attempt.process_id,
                    attempt.window_handle,
                    attempt.geometry.as_ref(),
                    attempt.frames,
                    code,
                ),
                manifest: unpaired_manifest(attempt.case, attempt.clip, attempt.actions, code),
            }
        }
    }
}

#[cfg(test)]
mod tests;

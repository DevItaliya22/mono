// SPDX-License-Identifier: MPL-2.0

//! Mutation paths must fail closed on gateways predating model selection.
use super::*;

#[test]
fn selected_models_and_all_resumes_require_model_aware_routes() {
    assert_eq!(create_path(false), "/v1/runs");
    assert_eq!(create_path(true), "/v2/runs");
    let run = RunId::mint();
    assert_eq!(
        resume_path(run),
        format!("/v2/runs/{}/resume", run.to_storage())
    );
}

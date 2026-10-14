// SPDX-License-Identifier: MPL-2.0

use super::*;

const VECTORS: &str =
    include_str!("../../../../contracts/vectors/regional-bootstrap-v1/foundation.json");

struct Repository(Result<Option<Region>, RepositoryError>);

impl AccountHomeRegionRepository for Repository {
    fn account_home_region(
        &self,
        _: KoruIdentityId,
    ) -> PortFuture<'_, Result<Option<Region>, RepositoryError>> {
        Box::pin(async move { self.0 })
    }
}

#[test]
fn frozen_deployment_vectors_admit_only_us_before_any_effect() {
    let vectors: serde_json::Value = serde_json::from_str(VECTORS).unwrap();
    for case in vectors["deployment"].as_array().unwrap() {
        let region = serde_json::from_value(case["region"].clone()).unwrap();
        match AccountDeployment::prepare(region) {
            Ok(deployment) => assert_eq!(
                serde_json::to_value(deployment.region()).unwrap(),
                case["result"]
            ),
            Err(error) => assert_eq!(error.code(), case["error"].as_str().unwrap()),
        }
    }
}

#[test]
fn frozen_active_account_vectors_never_infer_authority_from_deployment() {
    let vectors: serde_json::Value = serde_json::from_str(VECTORS).unwrap();
    let deployment = AccountDeployment::prepare(Region::Us).unwrap();
    let identity = "00000000-0000-4000-8000-000000000001".parse().unwrap();
    for case in vectors["account_home_region"].as_array().unwrap() {
        let value = match case["observation"].as_str().unwrap() {
            "active_us" => Ok(Some(Region::Us)),
            "active_eu" => Ok(Some(Region::Eu)),
            "missing" | "disabled" => Ok(None),
            "repository_unavailable" => Err(RepositoryError::Unavailable),
            "repository_corrupt" => Err(RepositoryError::Corrupt),
            other => panic!("unimplemented frozen vector {other}"),
        };
        match pollster::block_on(deployment.account_home_region(&Repository(value), identity)) {
            Ok(region) => assert_eq!(serde_json::to_value(region).unwrap(), case["result"]),
            Err(error) => assert_eq!(error.code(), case["error"].as_str().unwrap()),
        }
    }
}

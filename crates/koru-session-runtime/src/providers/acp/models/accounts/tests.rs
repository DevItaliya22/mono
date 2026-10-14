// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn explicit_connections_intersect_resolved_models_and_preserve_custom_ids() {
    let connections = parse(br#"{
      "deepseek":{"type":"api","key":"private-key-sentinel"},
      "custom-provider":{"type":"oauth","access":"private-access-sentinel","refresh":"private-refresh-sentinel","expires":0},
      "configured-without-models":{"type":"api","key":"private-unused-sentinel"}
    }"#).unwrap();
    let models = super::super::parse_catalog(
        b"amazon-bedrock/model\nopencode/free\ndeepseek/chat\ncustom-provider/nested/model\n",
    )
    .unwrap();
    let filtered = intersect(models, &connections).unwrap();
    assert_eq!(
        filtered
            .models()
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        ["custom-provider/nested/model", "deepseek/chat"]
    );
    assert!(!format!("{filtered:?}").contains("private"));
    assert!(!format!("{filtered:?}").contains("custom-provider"));
}

#[test]
fn account_values_are_shape_checked_without_retaining_secret_strings() {
    assert!(
        parse(br#"{"provider":{"type":"api","key":"secret","metadata":{"custom":"secret"}}}"#)
            .is_ok()
    );
    assert!(parse(br#"{"provider":{"type":"oauth","access":"secret","refresh":"secret","expires":123,"accountId":"secret"}}"#).is_ok());
    for bytes in [
        br#"{"provider":{"type":"api"}}"#.as_slice(),
        br#"{"provider":{"type":"api","key":123}}"#,
        br#"{"provider":{"type":"oauth","access":"secret","refresh":"secret","expires":-1}}"#,
        br#"{"provider":{"type":"api","key":"secret","metadata":{"value":5}}}"#,
        br#"{"provider":{"type":"unknown","key":"secret"}}"#,
        br#"{"provider":null}"#,
        br#"{"provider":{"type":"oauth","access":"secret","refresh":"secret","expires":123,"accountId":null}}"#,
        br#"{"provider":{"type":"api","key":"secret"}} trailing"#,
        br#"{"provider":{"type":"api","key":"private-secret-sentinel"}"#,
    ] {
        let failure = parse(bytes).unwrap_err();
        assert!(!format!("{failure:?}").contains("secret"));
    }
}

#[test]
fn empty_foreign_and_invalid_connection_maps_do_not_become_broad_catalogs() {
    assert!(parse(b"{}").unwrap().is_empty());
    assert!(
        parse(br#"{"https://example.test":{"type":"wellknown","key":"secret","token":"secret"}}"#)
            .unwrap()
            .is_empty()
    );
    for bytes in [
        b"[]".as_slice(),
        b"",
        br#"{"unsafe name":{"type":"api","key":"secret"}}"#,
        br#"{"provider":{"type":"api","key":"secret"},"provider":{"type":"api","key":"secret"}}"#,
    ] {
        assert!(parse(bytes).is_err());
    }
    let oversized = [b"{}".as_slice(), &vec![b' '; MAX_ACCOUNT_BYTES as usize]].concat();
    assert!(parse(&oversized).is_err());
    let models = super::super::parse_catalog(b"provider/model\n").unwrap();
    assert!(
        intersect(models, &parse(b"{}").unwrap())
            .unwrap()
            .models()
            .is_empty()
    );
}

#[test]
fn paths_use_provider_reported_absolute_data_directory_and_reject_ambiguity() {
    #[cfg(windows)]
    let path = "C:\\private path\\opencode";
    #[cfg(unix)]
    let path = "/private path/opencode";
    let output = format!("home       ignored\ndata       {path}\ncache      ignored\n");
    assert_eq!(data_path(output.as_bytes()).unwrap(), PathBuf::from(path));
    assert!(data_path(b"data       relative\n").is_err());
    assert!(data_path(b"cache      ignored\n").is_err());
    assert!(data_path(format!("{output}{output}").as_bytes()).is_err());
    assert!(data_path(format!("data       {path}\x1b[0m\n").as_bytes()).is_err());
}

#[test]
fn route_auth_override_wins_and_empty_override_disables_inherited_content() {
    let mut route = AcpRoute::opencode();
    route.environment.push((
        "OPENCODE_AUTH_CONTENT".into(),
        "invalid-private-sentinel".into(),
    ));
    route
        .environment
        .push(("OPENCODE_AUTH_CONTENT".into(), "{}".into()));
    assert!(
        parse(environment_content(&route).unwrap().unwrap().as_bytes())
            .unwrap()
            .is_empty()
    );
    route
        .environment
        .push(("OPENCODE_AUTH_CONTENT".into(), "".into()));
    assert!(environment_content(&route).unwrap().is_none());
    route.environment.push((
        "OPENCODE_AUTH_CONTENT".into(),
        "x".repeat(MAX_ACCOUNT_BYTES as usize + 1).into(),
    ));
    assert!(environment_content(&route).is_err());
}

#[cfg(windows)]
#[test]
fn windows_override_names_match_case_insensitively() {
    let mut route = AcpRoute::opencode();
    route
        .environment
        .push(("OPENCODE_AUTH_CONTENT".into(), "invalid".into()));
    route
        .environment
        .push(("opencode_auth_content".into(), "{}".into()));
    assert!(
        parse(environment_content(&route).unwrap().unwrap().as_bytes())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn oversized_connection_count_fails_without_returning_a_partial_catalog() {
    let entries = (0..=MAX_CONNECTIONS)
        .map(|index| format!(r#""provider-{index}":{{"type":"api","key":"private"}}"#))
        .collect::<Vec<_>>();
    assert!(parse(format!("{{{}}}", entries.join(",")).as_bytes()).is_err());
}

#[test]
fn unrelated_plugin_fields_do_not_reject_valid_connection_variants() {
    assert!(parse(br#"{"provider":{"type":"oauth","access":"secret","refresh":"secret","expires":0,"metadata":{"nested":{"token":"secret"}},"key":12}}"#).is_ok());
    assert!(
        parse(
            br#"{"provider":{"type":"api","key":"secret","accountId":null,"expires":"extension"}}"#
        )
        .is_ok()
    );
}

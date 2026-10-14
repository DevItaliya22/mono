// SPDX-License-Identifier: MPL-2.0

use super::*;

fn request(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
}

fn prompt(peer: &mut FakeAcpPeer) {
    peer.send(&request(
        3,
        "session/prompt",
        json!({
            "sessionId":SESSION,"prompt":[{"type":"text","text":"<prompt>"}]
        }),
    ))
    .unwrap();
}

fn cancel(peer: &mut FakeAcpPeer) {
    peer.send(&json!({"jsonrpc":"2.0","method":"session/cancel",
        "params":{"sessionId":SESSION}}))
        .unwrap();
}

fn frame(peer: &mut FakeAcpPeer) -> Value {
    match peer.receive(Duration::ZERO).unwrap() {
        TransportReceive::Frame(frame) => frame,
        _ => panic!("expected a fixture frame"),
    }
}

fn option(id: &'static str, kind: acp::PermissionOptionKind) -> acp::PermissionOption {
    acp::PermissionOption::new(id, "<label>", kind)
}

#[test]
fn handshake_frames_and_fixtures_use_pinned_v1_types() {
    let (mut peer, _) = FakeAcpPeer::new(FakeAcpConfig::default());
    peer.send(&request(
        1,
        "initialize",
        json!({
            "protocolVersion":1,"clientCapabilities":{}
        }),
    ))
    .unwrap();
    let initialized = decode::<acp::InitializeResponse>(&frame(&mut peer)["result"]).unwrap();
    assert_eq!(
        serde_json::to_value(initialized.protocol_version).unwrap(),
        1
    );
    assert!(initialized.agent_capabilities.mcp_capabilities.http);
    peer.send(&request(
        2,
        "session/new",
        json!({
            "cwd":"/fixture","mcpServers":[]
        }),
    ))
    .unwrap();
    decode::<acp::NewSessionResponse>(&frame(&mut peer)["result"]).unwrap();

    let fixtures: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/acp/1.7.0/payloads.json"
    )))
    .unwrap();
    decode::<acp::InitializeRequest>(&fixtures["initialize_request"]).unwrap();
    decode::<acp::InitializeResponse>(&fixtures["initialize_response"]).unwrap();
    decode::<acp::NewSessionRequest>(&fixtures["new_session_request"]).unwrap();
    decode::<acp::NewSessionResponse>(&fixtures["new_session_response"]).unwrap();
    decode::<acp::PromptRequest>(&fixtures["prompt_request"]).unwrap();
    decode::<acp::PromptResponse>(&fixtures["prompt_response"]).unwrap();
    decode::<acp::CancelNotification>(&fixtures["cancel_notification"]).unwrap();
    decode::<acp::SessionNotification>(&fixtures["session_notification"]).unwrap();
    decode::<acp::RequestPermissionRequest>(&fixtures["permission_request"]).unwrap();
    decode::<acp::RequestPermissionResponse>(&fixtures["permission_response"]).unwrap();
}

#[test]
fn every_provider_stage_has_a_content_free_fault() {
    for stage in [
        ProviderStage::Discovery,
        ProviderStage::Version,
        ProviderStage::Spawn,
        ProviderStage::Initialize,
        ProviderStage::Read,
        ProviderStage::Write,
        ProviderStage::Submit,
        ProviderStage::Cancel,
        ProviderStage::Approval,
        ProviderStage::Shutdown,
    ] {
        let (peer, _) = FakeAcpPeer::new(FakeAcpConfig {
            fail_at: Some(stage),
            ..Default::default()
        });
        let failure = peer.check_stage(stage).unwrap_err();
        assert_eq!(failure.stage, stage);
        assert_eq!(failure.diagnostic, ProviderDiagnostic::Io);
        assert!(!format!("{failure:?}").contains("<"));
    }
}

#[test]
fn io_submit_cancel_and_shutdown_faults_are_reached_by_transport() {
    for stage in [
        ProviderStage::Initialize,
        ProviderStage::Read,
        ProviderStage::Write,
        ProviderStage::Submit,
        ProviderStage::Cancel,
        ProviderStage::Shutdown,
    ] {
        let (mut peer, _) = FakeAcpPeer::new(FakeAcpConfig {
            fail_at: Some(stage),
            ..Default::default()
        });
        let result = match stage {
            ProviderStage::Read => peer.receive(Duration::ZERO).map(|_| ()),
            ProviderStage::Shutdown => peer.shutdown(Duration::ZERO),
            ProviderStage::Initialize | ProviderStage::Write => {
                peer.send(&request(1, "initialize", json!({"protocolVersion":1})))
            }
            ProviderStage::Submit => peer.send(&request(
                3,
                "session/prompt",
                json!({"sessionId":SESSION,"prompt":[]}),
            )),
            ProviderStage::Cancel => {
                peer.send(&json!({"method":"session/cancel","params":{"sessionId":SESSION}}))
            }
            _ => unreachable!(),
        };
        assert_eq!(result.unwrap_err().stage, stage);
    }
}

#[test]
fn pending_permission_preserves_arbitrary_asymmetric_remembered_and_duplicate_options() {
    use acp::PermissionOptionKind::*;
    for options in [
        vec![],
        vec![option("<allow>", AllowOnce)],
        vec![
            option("<remember-allow>", AllowAlways),
            option("<remember-reject>", RejectAlways),
        ],
        vec![
            option("<allow-a>", AllowOnce),
            option("<allow-b>", AllowOnce),
        ],
    ] {
        for selected in options
            .iter()
            .map(|o| Some(o.option_id.clone()))
            .chain(std::iter::once(None))
        {
            let (mut peer, state) = FakeAcpPeer::new(FakeAcpConfig {
                permission_options: Some(options.clone()),
                ..Default::default()
            });
            prompt(&mut peer);
            let offered =
                decode::<acp::RequestPermissionRequest>(&frame(&mut peer)["params"]).unwrap();
            assert_eq!(offered.options.len(), options.len());
            for (actual, expected) in offered.options.iter().zip(&options) {
                assert!(actual.option_id == expected.option_id && actual.kind == expected.kind);
            }
            assert!(matches!(
                peer.receive(Duration::ZERO).unwrap(),
                TransportReceive::Timeout
            ));
            assert!(state.lock().unwrap().permission_pending);
            let outcome = selected.map_or(
                json!({"outcome":"cancelled"}),
                |id| json!({"outcome":"selected","optionId":id}),
            );
            peer.send(&json!({"jsonrpc":"2.0","id":PERMISSION_ID,"result":{"outcome":outcome}}))
                .unwrap();
            assert!(!state.lock().unwrap().permission_pending);
            assert_eq!(state.lock().unwrap().prompts, 1);
        }
    }
}

#[test]
fn unoffered_and_replayed_permission_answers_are_rejected() {
    let (mut peer, state) = FakeAcpPeer::new(FakeAcpConfig {
        permission_options: Some(vec![option(
            "<allow>",
            acp::PermissionOptionKind::AllowOnce,
        )]),
        ..Default::default()
    });
    prompt(&mut peer);
    frame(&mut peer);
    let unoffered = json!({"id":PERMISSION_ID,"result":{"outcome":{"outcome":"selected","optionId":"<wrong>"}}});
    assert!(peer.send(&unoffered).is_err());
    assert!(state.lock().unwrap().permission_pending);
    let cancelled = json!({"id":PERMISSION_ID,"result":{"outcome":{"outcome":"cancelled"}}});
    peer.send(&cancelled).unwrap();
    assert!(peer.send(&cancelled).is_err());
    assert!(!state.lock().unwrap().permission_pending);
}

#[test]
fn permission_failure_leaves_the_request_pending() {
    let (mut peer, state) = FakeAcpPeer::new(FakeAcpConfig {
        fail_at: Some(ProviderStage::Approval),
        permission_options: Some(vec![]),
        ..Default::default()
    });
    prompt(&mut peer);
    frame(&mut peer);
    let error = peer
        .send(&json!({"id":PERMISSION_ID,"result":{"outcome":{"outcome":"cancelled"}}}))
        .unwrap_err();
    assert_eq!(error.stage, ProviderStage::Approval);
    assert!(state.lock().unwrap().permission_pending);
}

#[test]
fn ignored_cancel_can_stay_silent_or_flood_forever_without_queue_growth() {
    for flood in [false, true] {
        let (mut peer, state) = FakeAcpPeer::new(FakeAcpConfig {
            ignore_cancel: true,
            flood_after_cancel: flood,
            ..Default::default()
        });
        prompt(&mut peer);
        cancel(&mut peer);
        for _ in 0..1024 {
            let received = peer.receive(Duration::ZERO).unwrap();
            assert_eq!(matches!(received, TransportReceive::Frame(_)), flood);
            assert!(peer.inbound.is_empty());
        }
        assert!(peer.prompt_id.is_some());
        assert_eq!(state.lock().unwrap().cancels, 1);
        peer.shutdown(Duration::ZERO).unwrap();
        assert_eq!(state.lock().unwrap().shutdowns, 1);
        assert_eq!(
            peer.receive(Duration::ZERO).err().unwrap().diagnostic,
            ProviderDiagnostic::UnexpectedExit
        );
    }
}

#[test]
fn completion_cancel_late_frames_extensions_and_exit_are_scriptable() {
    let (mut peer, _) = FakeAcpPeer::new(FakeAcpConfig::default());
    prompt(&mut peer);
    cancel(&mut peer);
    let response = decode::<acp::PromptResponse>(&frame(&mut peer)["result"]).unwrap();
    assert_eq!(response.stop_reason, acp::StopReason::Cancelled);
    peer.queue(late_update()).unwrap();
    decode::<acp::SessionNotification>(&frame(&mut peer)["params"]).unwrap();
    peer.queue(json!({"jsonrpc":"2.0","id":"<extension>","method":"_future","params":{}}))
        .unwrap();
    assert_eq!(frame(&mut peer)["method"], "_future");
    peer.exit();
    assert_eq!(
        peer.receive(Duration::ZERO).err().unwrap().diagnostic,
        ProviderDiagnostic::UnexpectedExit
    );

    let (mut peer, _) = FakeAcpPeer::new(FakeAcpConfig {
        exit_after_prompt: true,
        ..Default::default()
    });
    prompt(&mut peer);
    assert_eq!(
        peer.receive(Duration::ZERO).err().unwrap().diagnostic,
        ProviderDiagnostic::UnexpectedExit
    );
}

#[test]
fn every_stable_stop_reason_roundtrips() {
    for stop in [
        acp::StopReason::EndTurn,
        acp::StopReason::Cancelled,
        acp::StopReason::Refusal,
        acp::StopReason::MaxTokens,
        acp::StopReason::MaxTurnRequests,
    ] {
        let (mut peer, _) = FakeAcpPeer::new(FakeAcpConfig {
            stop_reason: Some(stop),
            ..Default::default()
        });
        prompt(&mut peer);
        let response = decode::<acp::PromptResponse>(&frame(&mut peer)["result"]).unwrap();
        assert_eq!(response.stop_reason, stop);
    }
}

#[test]
fn malformed_standard_payloads_fail_and_debug_never_renders_private_wire_content() {
    let (mut peer, state) = FakeAcpPeer::new(FakeAcpConfig::default());
    assert!(
        peer.queue(json!({"method":"session/update","params":{"sessionId":17}}))
            .is_err()
    );
    assert!(
        peer.send(&request(
            1,
            "initialize",
            json!({"protocolVersion":"secret"})
        ))
        .is_err()
    );
    peer.send(&request(
        2,
        "session/new",
        json!({"cwd":"/secret-path","mcpServers":[{
            "type":"http","name":"koru","url":"http://127.0.0.1:1/mcp",
            "headers":[{"name":"Authorization","value":"Bearer SECRET-SENTINEL"}]
        }]}),
    ))
    .unwrap();
    assert_eq!(state.lock().unwrap().sent().len(), 1);
    let diagnostics = format!("{peer:?} {:?}", state.lock().unwrap());
    for secret in [
        "SECRET-SENTINEL",
        "secret-path",
        "Authorization",
        "127.0.0.1",
    ] {
        assert!(!diagnostics.contains(secret));
    }
}

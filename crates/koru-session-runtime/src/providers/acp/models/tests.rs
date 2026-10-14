// SPDX-License-Identifier: MPL-2.0

use super::*;
#[test]
fn catalog_uses_first_slash_preserves_nested_ids_and_rejects_noise() {
    let catalog =
        parse_catalog(b"openrouter/deepseek/deepseek-chat\ndeepseek/deepseek-chat\n").unwrap();
    assert_eq!(catalog.models().len(), 2);
    assert_eq!(catalog.models()[1].group.as_str(), "openrouter");
    assert!(parse_catalog(b"debug configuration leaked\nprovider/model\n").is_err());
    assert!(parse_catalog(b"provider/\n").is_err());
    assert!(parse_catalog(b"provider/model\x1b[31m").is_err());
    assert!(!format!("{catalog:?}").contains("deepseek"));
}
#[test]
fn selection_requires_advertised_value_and_confirmed_current_value() {
    let requested = "deepseek/chat".parse().unwrap();
    let advertised = json!({"configOptions":[{"id":"model","type":"select","currentValue":"other/model",
        "options":[{"group":"deepseek","options":[{"value":"deepseek/chat","name":"Chat"}]}]}]});
    assert!(validate_selection(&advertised, &requested).is_ok());
    assert!(validate_applied(&advertised, &requested).is_err());
    assert!(validate_selection(&advertised, &"missing/model".parse().unwrap()).is_err());
    let applied = json!({"configOptions":[{"id":"model","type":"select","currentValue":"deepseek/chat","options":[]}]});
    assert!(validate_applied(&applied, &requested).is_ok());
}

#[test]
fn config_models_keep_flat_values_under_the_kind_and_grouped_ones_under_their_group() {
    let flat = json!({"configOptions":[{"id":"model","type":"select","currentValue":"opus",
        "options":[{"value":"opus","name":"Opus"},{"value":"sonnet","name":"Sonnet"},{"value":"opus","name":"again"},{"value":"has space","name":"bad"}]}]});
    let models = config_models(&flat, "claude");
    let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["opus", "sonnet"],
        "a repeat and an unnameable value are left out"
    );
    assert!(models.iter().all(|model| model.group.as_str() == "claude"));

    let grouped = json!({"configOptions":[{"id":"model","type":"select","currentValue":"deepseek/chat",
        "options":[{"group":"deepseek","name":"DeepSeek","options":[{"value":"deepseek/chat","name":"Chat"}]},
                   {"group":"openai","name":"OpenAI","options":[{"value":"gpt-5","name":"GPT-5"}]}]}]});
    let models = config_models(&grouped, "opencode");
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].group.as_str(), "deepseek");
    assert_eq!(models[1].group.as_str(), "openai");

    assert!(config_models(&json!({"configOptions":[]}), "kimi").is_empty());
    assert!(
        config_models(
            &json!({"configOptions":[{"id":"mode","type":"select","options":[]}]}),
            "kimi"
        )
        .is_empty()
    );
}

struct SelectionPeer {
    selected: Option<String>,
    offer: bool,
    refuse: bool,
    sent: Arc<std::sync::Mutex<Vec<Value>>>,
    incoming: VecDeque<Value>,
}
impl JsonRpcTransport for SelectionPeer {
    fn send(&mut self, message: &Value) -> Result<(), ProviderFailure> {
        self.sent.lock().unwrap().push(message.clone());
        let result = match message["method"].as_str().unwrap() {
            "initialize" => {
                json!({"protocolVersion":1,"agentCapabilities":{"mcpCapabilities":{"http":true}},"authMethods":[]})
            }
            "session/new" => json!({"sessionId":"private-session-sentinel","configOptions":[{
                "id":"model","type":"select","name":"Model","currentValue":"provider/default",
                "options":if self.offer { json!([{"value":"private-provider/private-model-sentinel","name":"Private model"}]) } else { json!([]) }
            }]}),
            "session/set_config_option" => {
                if self.refuse {
                    self.incoming.push_back(json!({"jsonrpc":"2.0","id":message["id"],"error":{"code":-32602,"message":"private-error-sentinel"}}));
                    return Ok(());
                }
                self.selected = message["params"]["value"].as_str().map(str::to_owned);
                json!({"configOptions":[{"id":"model","type":"select","currentValue":self.selected,"options":[]}]})
            }
            "session/prompt" => json!({"stopReason":"end_turn"}),
            _ => panic!("unexpected method"),
        };
        self.incoming
            .push_back(json!({"jsonrpc":"2.0","id":message["id"],"result":result}));
        Ok(())
    }
    fn receive(&mut self, _: Duration) -> Result<TransportReceive, ProviderFailure> {
        Ok(self
            .incoming
            .pop_front()
            .map_or(TransportReceive::Timeout, TransportReceive::Frame))
    }
    fn shutdown(&mut self, _: Duration) -> Result<(), ProviderFailure> {
        Ok(())
    }
}
pub(in crate::providers::acp) fn selected_control(
    offer: bool,
    refuse: bool,
) -> (AcpControl, Arc<std::sync::Mutex<Vec<Value>>>) {
    let sent = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut config = AcpConfig::new(
        AcpRoute::opencode(),
        "fixture-opencode",
        // Only validated, never opened — but absolute on the host, and a drive
        // letter is not on a Mac.
        PathBuf::from(if cfg!(windows) {
            "C:/project"
        } else {
            "/project"
        }),
        "1.18.27".parse().unwrap(),
        Arc::new(crate::NoopAttachmentPathResolver),
    )
    .unwrap();
    config.delegation = Some(koru_session_protocol::DelegationLaunch {
        model: Some("private-provider/private-model-sentinel".parse().unwrap()),
        mcp_endpoint: "http://localhost:1/mcp".parse().unwrap(),
        credential: None,
        tools: Default::default(),
        resume: None,
    });
    (
        AcpControl::with_transport(
            config,
            Some(Box::new(SelectionPeer {
                selected: None,
                offer,
                refuse,
                sent: sent.clone(),
                incoming: VecDeque::new(),
            })),
        ),
        sent,
    )
}
#[test]
fn model_is_applied_before_prompt_and_never_claimed_as_reported() {
    let (mut control, sent) = selected_control(true, false);
    let capabilities = control
        .start(AgentStart::fresh(SessionId::mint(), None))
        .unwrap();
    assert_eq!(
        capabilities
            .delegation
            .economics
            .model
            .requested
            .as_ref()
            .unwrap()
            .as_str(),
        "private-provider/private-model-sentinel"
    );
    assert!(capabilities.delegation.economics.model.reported.is_none());
    control
        .submit(AgentInput {
            turn_id: AgentTurnId::mint(),
            text: "private-prompt-sentinel".into(),
            attachments: vec![],
        })
        .unwrap();
    let frames = sent.lock().unwrap();
    let methods: Vec<_> = frames
        .iter()
        .map(|frame| frame["method"].as_str().unwrap())
        .collect();
    assert_eq!(
        methods,
        [
            "initialize",
            "session/new",
            "session/set_config_option",
            "session/prompt"
        ]
    );
}
#[test]
fn missing_or_refused_model_fails_before_any_prompt() {
    for (offer, refuse) in [(false, false), (true, true)] {
        let (mut control, sent) = selected_control(offer, refuse);
        let error = control
            .start(AgentStart::fresh(SessionId::mint(), None))
            .unwrap_err();
        assert!(!format!("{error:?}").contains("private"));
        assert!(
            !sent
                .lock()
                .unwrap()
                .iter()
                .any(|frame| frame["method"] == "session/prompt")
        );
        control.shutdown().unwrap();
    }
}

#[test]
fn a_stdio_bridge_rides_session_new_as_acps_untagged_server() {
    let sent = Arc::new(std::sync::Mutex::new(Vec::new()));
    let config = AcpConfig::new(
        AcpRoute::kimi(),
        "fixture-kimi",
        PathBuf::from(if cfg!(windows) {
            "C:/project"
        } else {
            "/project"
        }),
        "0.42.0".parse().unwrap(),
        Arc::new(crate::NoopAttachmentPathResolver),
    )
    .unwrap()
    .with_mcp_servers(vec![crate::providers::McpServerLaunch {
        name: "koru".into(),
        command: "C:/koru/koru-mcp.exe".into(),
        args: vec!["serve".into(), "--stdio".into()],
        env: Vec::new(),
    }]);
    let mut control = AcpControl::with_transport(
        config,
        Some(Box::new(SelectionPeer {
            selected: None,
            offer: false,
            refuse: false,
            sent: sent.clone(),
            incoming: VecDeque::new(),
        })),
    );
    control
        .start(AgentStart::fresh(SessionId::mint(), None))
        .unwrap();
    let sent = sent.lock().unwrap();
    let new_session = sent
        .iter()
        .find(|message| message["method"] == "session/new")
        .expect("session/new sent");
    let servers = new_session["params"]["mcpServers"].as_array().unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0]["name"], "koru");
    assert_eq!(servers[0]["command"], "C:/koru/koru-mcp.exe");
    assert_eq!(servers[0]["args"], json!(["serve", "--stdio"]));
    assert!(
        servers[0].get("type").is_none(),
        "stdio is the untagged shape"
    );
}

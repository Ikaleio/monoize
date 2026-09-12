use super::*;

fn request() -> urp::UrpRequest {
    urp::decode::openai_responses::decode_request(&json!({
        "model": "gpt-6-astra", "stream": true,
        "tools": [
            {"type":"function","name":"functions_apply_patch_1","parameters":{"type":"object"}},
            {"type":"namespace","name":"functions","tools":[
                {"type":"custom","name":"apply_patch","description":"Apply a patch","format":{"type":"text"}}
            ]},
            {"type":"namespace","name":"other","tools":[
                {"type":"function","name":"apply_patch","parameters":{"type":"object"}}
            ]}
        ],
        "tool_choice":{"type":"custom","namespace":"functions","name":"apply_patch"},
        "input":[
            {"type":"custom_tool_call","call_id":"patch1","namespace":"functions","name":"apply_patch","input":"*** Begin Patch\n*** End Patch"},
            {"type":"custom_tool_call_output","call_id":"patch1","output":"ok"},
            {"type":"additional_tools","tools":[
                {"type":"namespace","name":"functions","tools":[
                    {"type":"custom","name":"apply_patch","description":"duplicate"}
                ]}
            ]}
        ]
    })).unwrap()
}

#[test]
fn namespace_messages_custom_patch_roundtrip() {
    let mut req = request();
    promote_responses_additional_tools(&mut req, ProviderType::Messages);
    filter_tools_for_provider(
        &mut req,
        ProviderType::Messages,
        DownstreamProtocol::Responses,
    );
    let tools = req.tools.as_ref().unwrap();
    assert_eq!(tools.len(), 3);
    let aliases = tool_namespace_aliases(&req);
    assert_eq!(aliases.len(), 2);
    let alias = aliases
        .iter()
        .find(|(_, identity)| identity["namespace"] == "functions")
        .unwrap()
        .0;
    assert_ne!(alias, "functions_apply_patch_1");
    let names: HashSet<_> = tools.iter().filter_map(tool_wire_name).collect();
    assert_eq!(names.len(), 3);
    let urp::Node::ToolCall {
        tool_type,
        name,
        arguments,
        extra_body,
        ..
    } = &req.input[0]
    else {
        panic!()
    };
    assert_eq!(*tool_type, urp::ToolCallType::Function);
    assert_eq!(name, alias);
    assert_eq!(
        serde_json::from_str::<Value>(arguments).unwrap()["input"],
        "*** Begin Patch\n*** End Patch"
    );
    assert!(!extra_body.contains_key("namespace"));
    let urp::ToolChoice::Specific(choice) = req.tool_choice.as_ref().unwrap() else {
        panic!()
    };
    assert_eq!(choice["function"]["name"], *alias);
    let mut response = urp::decode::anthropic::decode_response(&json!({
        "id":"msg_test","type":"message","role":"assistant","model":"gpt-6-astra",
        "content":[{"type":"tool_use","id":"patch2","name":alias,"input":{"input":"*** Begin Patch\n*** End Patch"}}],
        "stop_reason":"tool_use","usage":{"input_tokens":1,"output_tokens":1}
    })).unwrap();
    restore_messages_custom_tool_calls(&req, &mut response);
    for node in &mut response.output {
        restore_tool_namespace_node(node, &aliases);
    }
    let wire = urp::encode::openai_responses::encode_response(&response, "gpt-6-astra");
    assert_eq!(wire["output"][0]["type"], "custom_tool_call");
    assert_eq!(wire["output"][0]["namespace"], "functions");
    assert_eq!(wire["output"][0]["name"], "apply_patch");
    assert_eq!(wire["output"][0]["call_id"], "patch2");
    assert_eq!(wire["output"][0]["input"], "*** Begin Patch\n*** End Patch");
}

#[test]
fn namespace_stream_start_done_and_terminal_restore() {
    let mut req = request();
    promote_responses_additional_tools(&mut req, ProviderType::ChatCompletion);
    let aliases = tool_namespace_aliases(&req);
    let alias = aliases
        .iter()
        .find(|(_, identity)| identity["namespace"] == "functions")
        .unwrap()
        .0;
    let node = urp::Node::ToolCall {
        namespace: None,
        signature: None,

        id: None,
        tool_type: urp::ToolCallType::Custom,
        call_id: "patch1".into(),
        name: alias.clone(),
        arguments: "patch".into(),
        extra_body: HashMap::new(),
    };
    let mut events = [
        urp::UrpStreamEvent::NodeStart {
            node_index: 0,
            header: urp::NodeHeader::ToolCall {
                namespace: None,
                signature: None,

                id: None,
                tool_type: urp::ToolCallType::Custom,
                call_id: "patch1".into(),
                name: alias.clone(),
            },
            extra_body: HashMap::new(),
        },
        urp::UrpStreamEvent::NodeDone {
            node_index: 0,
            node: node.clone(),
            usage: None,
            extra_body: HashMap::new(),
        },
        urp::UrpStreamEvent::ResponseDone {
            finish_reason: None,
            usage: None,
            output: vec![node],
            extra_body: HashMap::new(),
        },
    ];
    for event in &mut events {
        restore_tool_namespace_event(event, &aliases);
    }
    for event in events {
        let value = serde_json::to_value(event).unwrap();
        let item = match value["event"].as_str().unwrap() {
            "node_start" => {
                assert_eq!(value["header"]["namespace"], "functions");
                value["header"].clone()
            }
            "node_done" => {
                assert_eq!(value["node"]["namespace"], "functions");
                value["node"].clone()
            }
            _ => {
                assert_eq!(value["output"][0]["namespace"], "functions");
                value["output"][0].clone()
            }
        };
        assert_eq!(item["name"], "apply_patch");
        assert_eq!(item["call_id"], "patch1");
    }
}

#[test]
fn namespace_native_responses_retains_named_choice() {
    let mut req = request();
    let original = serde_json::to_value(&req).unwrap();
    promote_responses_additional_tools(&mut req, ProviderType::Responses);
    filter_tools_for_provider(
        &mut req,
        ProviderType::Responses,
        DownstreamProtocol::Responses,
    );
    assert_eq!(serde_json::to_value(&req).unwrap(), original);
    let wire = urp::encode::openai_responses::encode_request(&req, "gpt-6-astra");
    assert_eq!(wire["tools"][1]["type"], "namespace");
    assert_eq!(wire["tool_choice"]["namespace"], "functions");
    assert_eq!(wire["tool_choice"]["name"], "apply_patch");
}

#[test]
fn promotion_preserves_native_config_and_toolset_namespace() {
    let mut request=urp::decode::anthropic::decode_request(&json!({"model":"claude","max_tokens":100,"tools":[{"type":"computer_20251124","name":"computer","display_width_px":1440,"display_height_px":900}],"messages":[{"role":"assistant","content":[{"type":"tool_use","id":"call","name":"screenshot","toolset_name":"computer","input":{}}]}]})).unwrap();
    let before = serde_json::to_value(&request).unwrap();
    promote_responses_additional_tools(&mut request, ProviderType::Messages);
    assert_eq!(serde_json::to_value(&request).unwrap(), before);
}
#[test]
fn canonical_allowed_tools_namespace_selector_is_bridged() {
    let mut request = request();
    request.tool_choice = Some(urp::ToolChoice::Specific(
        json!({"type":"allowed_tools","mode":"required","tools":[{"type":"custom","namespace":"functions","name":"apply_patch"}]}),
    ));
    promote_responses_additional_tools(&mut request, ProviderType::ChatCompletion);
    let aliases = tool_namespace_aliases(&request);
    let alias = aliases
        .iter()
        .find(|(_, identity)| identity["namespace"] == "functions")
        .unwrap()
        .0;
    assert_eq!(
        serde_json::to_value(request.tool_choice).unwrap()["tools"][0]["custom"]["name"],
        *alias
    );
}

#[test]
fn gemini_function_namespaces_use_collision_free_aliases() {
    let mut request=urp::decode::openai_responses::decode_request(&json!({"model":"gemini","input":[{"type":"function_call","call_id":"c","namespace":"one","name":"f","arguments":"{}"}],"tools":[{"type":"namespace","name":"one","tools":[{"type":"function","name":"f","parameters":{"type":"object"}}]},{"type":"namespace","name":"two","tools":[{"type":"function","name":"f","parameters":{"type":"object"}}]}]})).unwrap();
    promote_responses_additional_tools(&mut request, ProviderType::Gemini);
    filter_tools_for_provider(
        &mut request,
        ProviderType::Gemini,
        DownstreamProtocol::Responses,
    );
    let aliases = tool_namespace_aliases(&request);
    assert_eq!(aliases.len(), 2);
    let encoded = urp::encode::gemini::encode_request(&request, "gemini");
    let declarations = encoded["tools"][0]["functionDeclarations"]
        .as_array()
        .unwrap();
    assert_eq!(declarations.len(), 2);
    assert_ne!(declarations[0]["name"], declarations[1]["name"]);
    let mut response=urp::decode::gemini::decode_response(&json!({"candidates":[{"content":{"parts":[{"functionCall":{"id":"c","name":declarations[0]["name"],"args":{}}}]},"finishReason":"STOP"}]})).unwrap();
    restore_tool_namespace_node(&mut response.output[0], &aliases);
    assert!(
        matches!(&response.output[0],urp::Node::ToolCall {namespace:Some(namespace),name,..} if namespace=="one" && name=="f")
    );
}

#[test]
fn gemini_namespaced_result_names_follow_call_aliases() {
    for namespace in [Some("one"), None] {
        let mut request: urp::UrpRequest = serde_json::from_value(json!({"model":"gemini","input":[
            {"type":"tool_call","call_id":"c","namespace":"one","name":"f","arguments":"{}"},
            {"type":"tool_result","call_id":"c","namespace":namespace,"name":"f","content":[{"type":"text","text":"done"}]}
        ],"tools":[{"type":"namespace","name":"one","tools":[{"type":"function","function":{"name":"f","parameters":{"type":"object"}}}]}]})).unwrap();
        promote_responses_additional_tools(&mut request, ProviderType::Gemini);
        let encoded = urp::encode::gemini::encode_request(&request, "gemini");
        let name = &encoded["tools"][0]["functionDeclarations"][0]["name"];
        assert_ne!(name, "f");
        assert_eq!(
            &encoded["contents"][0]["parts"][0]["functionCall"]["name"],
            name
        );
        assert_eq!(
            &encoded["contents"][1]["parts"][0]["functionResponse"]["name"],
            name
        );
    }
    let mut request: urp::UrpRequest = serde_json::from_value(json!({"model":"m","input":[],"tools":[{
        "type":"native_future_tool","name":"native","namespace":"native_space","origin_protocol":"messages","config":{"future":true}
    }]})).unwrap();
    let before = serde_json::to_value(&request).unwrap();
    promote_responses_additional_tools(&mut request, ProviderType::Messages);
    assert_eq!(serde_json::to_value(request).unwrap(), before);
}

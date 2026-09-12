use super::{FinishReason, ImageSource, Node, ProviderProtocol, UrpResponse, UrpStreamEvent};
use super::{decode::openai_responses as decode, encode::openai_responses as encode};
use axum::response::IntoResponse;
use serde_json::{Value, json};
use std::collections::HashSet;
use tokio::sync::mpsc;

fn response(output: Vec<Value>) -> Value {
    json!({"id":"resp_feature", "object":"response", "created_at": 123,
        "model":"feature-model", "status":"completed", "output":output,
        "usage":{"input_tokens":4,"output_tokens":3,"total_tokens":7}})
}

fn frame(event: &str, mut data: Value) -> String {
    data["type"] = json!(event);
    format!("event: {event}\ndata: {data}\n\n")
}

fn fixture_stream(item: &Value, progress: Option<&str>) -> String {
    let mut wire = frame("response.created", json!({"response": response(vec![])}));
    let mut start = item.clone();
    start["status"] = json!("in_progress");
    let kind = item["type"].as_str().unwrap();
    if matches!(kind, "function_call" | "custom_tool_call") {
        start[if kind == "function_call" {
            "arguments"
        } else {
            "input"
        }] = json!("");
    }
    if kind == "message" {
        start["content"] = json!([]);
    }
    wire.push_str(&frame(
        "response.output_item.added",
        json!({"output_index":0,"item":start}),
    ));
    if let Some(progress) = progress {
        wire.push_str(&frame(
            progress,
            json!({"output_index":0,"item_id":item["id"],"probe":"progress"}),
        ));
    }
    if matches!(kind, "function_call" | "custom_tool_call") {
        let field = if kind == "function_call" {
            "arguments"
        } else {
            "input"
        };
        let event = if kind == "function_call" {
            "response.function_call_arguments.delta"
        } else {
            "response.custom_tool_call_input.delta"
        };
        wire.push_str(&frame(
            event,
            json!({"output_index":0,"item_id":item["id"],"delta":item[field]}),
        ));
    }
    if kind == "message" {
        for (index, part) in item["content"].as_array().unwrap().iter().enumerate() {
            let mut stub = part.clone();
            if part["type"] == "output_text" {
                stub["text"] = json!("");
            }
            if part["type"] == "refusal" {
                stub["refusal"] = json!("");
            }
            wire.push_str(&frame(
                "response.content_part.added",
                json!({"output_index":0,"item_id":item["id"],"content_index":index,"part":stub}),
            ));
            let event = match part["type"].as_str() {
                Some("output_text") => Some(("response.output_text.delta", "text")),
                Some("refusal") => Some(("response.refusal.delta", "refusal")),
                _ => None,
            };
            if let Some((event, field)) = event {
                wire.push_str(&frame(event, json!({"output_index":0,"item_id":item["id"],"content_index":index,"delta":part[field]})));
            }
            wire.push_str(&frame(
                "response.content_part.done",
                json!({"output_index":0,"item_id":item["id"],"content_index":index,"part":part}),
            ));
        }
    }
    wire.push_str(&frame(
        "response.output_item.done",
        json!({"output_index":0,"item":item}),
    ));
    wire.push_str(&frame(
        "response.completed",
        json!({"response":response(vec![item.clone()])}),
    ));
    wire.push_str("data: [DONE]\n\n");
    wire
}

async fn decode_stream(wire: String) -> Vec<UrpStreamEvent> {
    let response = reqwest::Response::from(
        axum::http::Response::builder()
            .header("content-type", "text/event-stream")
            .body(wire)
            .unwrap(),
    );
    let request = crate::handlers::UrpRequest {
        model: "feature-model".into(),
        max_multiplier: None,
        audio_output_format: None,
        server_tool_usage_classes: vec![],
        messages_custom_tool_names: HashSet::new(),
        affinity_explicit: None,
        affinity_prefix_hash: String::new(),
    };
    let (tx, mut rx) = mpsc::channel(4096);
    super::stream_decode::openai_responses::stream_responses_to_urp_events(
        &request, None, response, tx, None, None, 1000,
    )
    .await
    .unwrap();
    let mut result = Vec::new();
    while let Some(event) = rx.recv().await {
        result.push(event);
    }
    result
}

async fn encode_events(events: Vec<UrpStreamEvent>) -> (String, Vec<Value>) {
    let (tx, rx) = mpsc::channel(4096);
    for event in events {
        tx.send(event).await.unwrap();
    }
    drop(tx);
    let (wire_tx, mut wire_rx) = mpsc::channel(4096);
    super::stream_encode::openai_responses::encode_urp_stream_as_responses(
        rx,
        wire_tx,
        "feature-model",
        std::time::Instant::now(),
        None,
        false,
    )
    .await
    .unwrap();
    let mut frames = Vec::new();
    while let Some(event) = wire_rx.recv().await {
        frames.push(Ok::<_, std::convert::Infallible>(event));
    }
    let body = axum::response::Sse::new(futures_util::stream::iter(frames))
        .into_response()
        .into_body();
    let wire = String::from_utf8(
        axum::body::to_bytes(body, usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    let values = wire
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter(|data| *data != "[DONE]")
        .map(|data| serde_json::from_str(data).unwrap())
        .collect();
    (wire, values)
}

fn terminal_output(events: &[UrpStreamEvent]) -> &[Node] {
    events
        .iter()
        .find_map(|event| match event {
            UrpStreamEvent::ResponseDone { output, .. } => Some(output.as_slice()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("terminal output missing: {events:#?}"))
}

async fn assert_provider_roundtrip(tool: Value, item: Value, progress: Option<&str>) {
    for stream in [false, true] {
        let request = decode::decode_request(&json!({"model":"feature-model","stream":stream,
            "tools":[tool.clone()],"input":[item.clone()]}))
        .unwrap();
        let encoded = encode::encode_request(&request, "feature-model");
        assert_eq!(encoded["tools"][0], tool);
        assert_eq!(encoded["input"][0], item);
        assert_eq!(encoded["stream"], stream);
    }
    let decoded = decode::decode_response(&response(vec![item.clone()])).unwrap();
    assert!(
        matches!(&decoded.output[0], Node::ProviderItem { origin_protocol: ProviderProtocol::Responses, body, .. } if body == &item)
    );
    assert_eq!(
        encode::encode_response(&decoded, "feature-model")["output"][0],
        item
    );
    let events = decode_stream(fixture_stream(&item, progress)).await;
    assert!(
        matches!(&terminal_output(&events)[0], Node::ProviderItem { origin_protocol: ProviderProtocol::Responses, body, .. } if body == &item)
    );
    let (wire, frames) = encode_events(events).await;
    if let Some(progress) = progress {
        assert!(
            frames
                .iter()
                .any(|frame| frame["type"] == progress && frame["probe"] == "progress")
        );
    }
    let terminal = frames
        .iter()
        .find(|frame| frame["type"] == "response.completed")
        .unwrap();
    assert_eq!(terminal["response"]["output"][0], item);
    assert_eq!(
        frames
            .iter()
            .filter(|frame| frame["type"] == "response.output_item.done")
            .count(),
        1
    );
    let back = decode_stream(wire).await;
    assert!(matches!(&terminal_output(&back)[0], Node::ProviderItem { body, .. } if body == &item));
}

macro_rules! provider_feature {
    ($name:ident, $tool:expr, $item:expr, $progress:expr) => {
        #[tokio::test]
        async fn $name() {
            assert_provider_roundtrip($tool, $item, $progress).await;
        }
    };
}

provider_feature!(
    computer_use,
    json!({"type":"computer"}),
    json!({"type":"computer_call","id":"cu_1","call_id":"call_1","actions":[{"type":"click","button":"left","x":10,"y":20},{"type":"type","text":"query"}],"pending_safety_checks":[],"status":"completed"}),
    None
);
provider_feature!(
    computer_use_preview,
    json!({"type":"computer_use_preview","display_width":1024,"display_height":768,"environment":"browser"}),
    json!({"type":"computer_call","id":"cu_1","call_id":"call_1","action":{"type":"screenshot"},"pending_safety_checks":[],"status":"completed"}),
    None
);
provider_feature!(
    computer_screenshot_result,
    json!({"type":"computer"}),
    json!({"type":"computer_call_output","id":"cuo_1","call_id":"call_1","output":{"type":"computer_screenshot","image_url":"data:image/png;base64,YQ==","detail":"original"},"acknowledged_safety_checks":[{"id":"check_1","code":"x","message":"checked"}],"status":"completed"}),
    None
);
provider_feature!(
    web_search,
    json!({"type":"web_search","search_context_size":"low","filters":{"allowed_domains":["example.com"]}}),
    json!({"type":"web_search_call","id":"ws_1","status":"completed","action":{"type":"search","query":"example","sources":[{"type":"url","url":"https://example.com"}]}}),
    Some("response.web_search_call.searching")
);
provider_feature!(
    file_search,
    json!({"type":"file_search","vector_store_ids":["vs_1"],"max_num_results":2}),
    json!({"type":"file_search_call","id":"fs_1","status":"completed","queries":["example"],"results":[{"file_id":"file_1","filename":"example.txt","score":0.9,"text":"match"}]}),
    Some("response.file_search_call.searching")
);
provider_feature!(
    code_interpreter,
    json!({"type":"code_interpreter","container":{"type":"auto","memory_limit":"4g"}}),
    json!({"type":"code_interpreter_call","id":"ci_1","status":"completed","code":"print(1)","container_id":"cntr_1","outputs":[{"type":"logs","logs":"1"}]}),
    Some("response.code_interpreter_call.interpreting")
);
provider_feature!(
    mcp_call,
    json!({"type":"mcp","server_label":"docs","server_url":"https://example.com/mcp","require_approval":"always"}),
    json!({"type":"mcp_call","id":"mcp_1","status":"completed","server_label":"docs","name":"lookup","arguments":"{}","output":"answer","error":null}),
    Some("response.mcp_call.in_progress")
);
provider_feature!(
    mcp_approval_request,
    json!({"type":"mcp","server_label":"docs","server_url":"https://example.com/mcp"}),
    json!({"type":"mcp_approval_request","id":"mcpr_1","server_label":"docs","name":"lookup","arguments":"{}"}),
    None
);
provider_feature!(
    mcp_approval_response,
    json!({"type":"mcp","server_label":"docs","server_url":"https://example.com/mcp"}),
    json!({"type":"mcp_approval_response","id":"mcpa_1","approval_request_id":"mcpr_1","approve":true,"reason":"allowed"}),
    None
);
provider_feature!(
    mcp_list_tools,
    json!({"type":"mcp","server_label":"docs","server_url":"https://example.com/mcp"}),
    json!({"type":"mcp_list_tools","id":"mcpl_1","server_label":"docs","tools":[{"name":"lookup","input_schema":{"type":"object"}}]}),
    Some("response.mcp_list_tools.in_progress")
);
provider_feature!(
    shell_call,
    json!({"type":"shell","environment":{"type":"local"}}),
    json!({"type":"shell_call","id":"sh_1","call_id":"call_sh","status":"completed","action":{"commands":["pwd"],"timeout_ms":1000,"max_output_length":500}}),
    None
);
provider_feature!(
    shell_output,
    json!({"type":"shell"}),
    json!({"type":"shell_call_output","id":"sho_1","call_id":"call_sh","output":[{"stdout":"/tmp","stderr":"","outcome":{"type":"exit","exit_code":0}}]}),
    None
);
provider_feature!(
    apply_patch_call,
    json!({"type":"apply_patch"}),
    json!({"type":"apply_patch_call","id":"ap_1","call_id":"call_ap","status":"completed","operation":{"type":"update_file","path":"a.txt","diff":"@@\n-a\n+b"}}),
    None
);
provider_feature!(
    apply_patch_output,
    json!({"type":"apply_patch"}),
    json!({"type":"apply_patch_call_output","id":"apo_1","call_id":"call_ap","status":"completed","output":"patched"}),
    None
);
provider_feature!(
    local_shell_call,
    json!({"type":"local_shell"}),
    json!({"type":"local_shell_call","id":"ls_1","call_id":"call_ls","status":"completed","action":{"type":"exec","command":["pwd"],"env":{},"timeout_ms":1000}}),
    None
);
provider_feature!(
    compaction,
    json!({"type":"web_search"}),
    json!({"type":"compaction","id":"cmp_1","encrypted_content":"opaque"}),
    None
);

provider_feature!(
    programmatic_program,
    json!({"type":"programmatic_tool_calling"}),
    json!({"type":"program","id":"prog_1","call_id":"call_program","code":"return await tools.lookup({});","fingerprint":"opaque-program-fingerprint"}),
    None
);
provider_feature!(
    programmatic_program_output,
    json!({"type":"programmatic_tool_calling"}),
    json!({"type":"program_output","id":"prog_out_1","call_id":"call_program","result":"{\"answer\":42}","status":"completed"}),
    None
);
provider_feature!(
    tool_search_call,
    json!({"type":"tool_search","execution":"client","parameters":{"type":"object","properties":{"query":{"type":"string"}}}}),
    json!({"type":"tool_search_call","id":"search_1","call_id":"call_search","execution":"client","status":"completed","arguments":{"query":"lookup","limit":2}}),
    None
);
provider_feature!(
    tool_search_output,
    json!({"type":"tool_search","execution":"server"}),
    json!({"type":"tool_search_output","id":"search_out_1","call_id":null,"execution":"server","status":"completed","tools":[{"type":"namespace","name":"inventory","tools":[{"type":"function","name":"lookup","defer_loading":true,"parameters":{"type":"object"}}]}]}),
    None
);
provider_feature!(
    additional_tools,
    json!({"type":"tool_search","execution":"server"}),
    json!({"type":"additional_tools","id":"additional_1","role":"developer","tools":[{"type":"function","name":"lookup","defer_loading":true,"parameters":{"type":"object"}},{"type":"mcp","server_label":"catalog","server_url":"https://example.com/mcp","defer_loading":true}]}),
    None
);

#[tokio::test]
async fn programmatic_function_callers_and_deferred_tool_config_roundtrip() {
    let caller = json!({"type":"program","caller_id":"call_program","future":"retained"});
    let call = json!({"type":"function_call","id":"fc_program","call_id":"call_child","name":"lookup","arguments":"{}","status":"completed","caller":caller});
    let result = json!({"type":"function_call_output","id":"fco_program","call_id":"call_child","output":"42","caller":caller});
    for stream in [false, true] {
        let mut request = decode::decode_request(&json!({"model":"feature-model","stream":stream,"input":[call.clone(),result.clone()],"tools":[
            {"type":"programmatic_tool_calling"},
            {"type":"function","name":"lookup","parameters":{"type":"object"},"allowed_callers":["direct","program"],"output_schema":{"type":"number"},"defer_loading":true},
            {"type":"mcp","server_label":"catalog","server_url":"https://example.com/mcp","defer_loading":true}
        ]})).unwrap();
        let encoded = encode::encode_request(&request, "feature-model");
        assert_eq!(encoded["input"][0]["caller"], caller);
        assert_eq!(encoded["input"][1]["caller"], caller);
        assert_eq!(
            encoded["tools"][1]["allowed_callers"],
            json!(["direct", "program"])
        );
        assert_eq!(
            encoded["tools"][1]["output_schema"],
            json!({"type":"number"})
        );
        assert_eq!(encoded["tools"][1]["defer_loading"], true);
        assert_eq!(encoded["tools"][2]["defer_loading"], true);
        let tools = request.tools.as_mut().unwrap();
        tools[1].function.as_mut().unwrap().parameters =
            Some(json!({"type":"object","properties":{"key":{"type":"string"}}}));
        assert_eq!(tools[2].origin_protocol, Some(ProviderProtocol::Responses));
        assert!(tools[2].extra_body.is_empty());
        tools[2]
            .config
            .as_mut()
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("defer_loading");
        for node in &mut request.input {
            if let Node::ToolCall { extra_body, .. } | Node::ToolResult { extra_body, .. } = node {
                extra_body.remove("caller");
            }
        }
        let encoded = encode::encode_request(&request, "feature-model");
        assert_eq!(
            encoded["tools"][1]["parameters"]["properties"]["key"]["type"],
            "string"
        );
        assert!(encoded["tools"][2].get("defer_loading").is_none());
        assert!(
            encoded["input"]
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item.get("caller").is_none())
        );
    }
    for item in [call, result] {
        let decoded = decode::decode_response(&response(vec![item.clone()])).unwrap();
        assert_eq!(
            encode::encode_response(&decoded, "feature-model")["output"][0]["caller"],
            caller
        );
        let events = decode_stream(fixture_stream(&item, None)).await;
        let (wire, frames) = encode_events(events).await;
        let terminal = frames
            .iter()
            .find(|frame| frame["type"] == "response.completed")
            .unwrap();
        assert_eq!(terminal["response"]["output"][0]["caller"], caller);
        assert!(
            terminal_output(&decode_stream(wire).await)
                .iter()
                .any(|node| match node {
                    Node::ToolCall { extra_body, .. } | Node::ToolResult { extra_body, .. } =>
                        extra_body.get("caller") == Some(&caller),
                    _ => false,
                })
        );
    }
}

#[test]
fn structured_format_controls_are_single_owned_and_deletable() {
    for stream in [false, true] {
        let mut request = decode::decode_request(&json!({"model":"feature-model","input":"hello","stream":stream,
            "instructions":"be precise","temperature":0.3,"top_p":0.8,"max_output_tokens":123,
            "parallel_tool_calls":false,"user":"caller","reasoning":{"effort":"high","summary":"auto","future":1},
            "text":{"verbosity":"low","format":{"type":"json_schema","name":"answer","schema":{"type":"object"},"strict":true},"future":"kept"}})).unwrap();
        assert_eq!(request.extra_body["text"], json!({"future":"kept"}));
        let encoded = encode::encode_request(&request, "feature-model");
        assert_eq!(encoded["text"]["format"]["name"], "answer");
        assert_eq!(encoded["text"]["verbosity"], "low");
        assert_eq!(encoded["max_output_tokens"], 123);
        request.verbosity = None;
        request.response_format = None;
        request.reasoning.as_mut().unwrap().summary = None;
        request.input.retain(|node| {
            !matches!(
                node,
                Node::Text {
                    role: super::OrdinaryRole::Developer,
                    ..
                }
            )
        });
        let encoded = encode::encode_request(&request, "feature-model");
        assert_eq!(encoded["text"], json!({"future":"kept"}));
        assert!(encoded["reasoning"].get("summary").is_none());
        assert_eq!(encoded["instructions"], "");
    }
}

#[test]
fn flat_tool_definition_namespaces_are_typed_and_deletable() {
    for stream in [false, true] {
        let mut request = decode::decode_request(&json!({"model":"feature-model","stream":stream,"input":[],"tools":[
            {"type":"function","name":"lookup","namespace":"inventory","parameters":{"type":"object"}},
            {"type":"custom","name":"script","namespace":"inventory","format":{"type":"text"}},
            {"type":"web_search","namespace":"inventory","search_context_size":"low"}
        ]})).unwrap();
        assert!(
            request
                .tools
                .as_ref()
                .unwrap()
                .iter()
                .all(|tool| tool.namespace.as_deref() == Some("inventory"))
        );
        let initial = encode::encode_request(&request, "feature-model");
        assert!(
            initial["tools"]
                .as_array()
                .unwrap()
                .iter()
                .all(|tool| tool["namespace"] == "inventory")
        );
        for tool in request.tools.as_mut().unwrap() {
            tool.namespace = Some("renamed".to_string());
            tool.extra_body.insert("namespace".into(), json!("stale"));
            if let Some(function) = &mut tool.function {
                function
                    .extra_body
                    .insert("namespace".into(), json!("stale"));
            }
            if let Some(custom) = &mut tool.custom {
                custom.extra_body.insert("namespace".into(), json!("stale"));
            }
            if let Some(config) = &mut tool.config {
                config["namespace"] = json!("stale");
            }
        }
        let changed = encode::encode_request(&request, "feature-model");
        assert!(
            changed["tools"]
                .as_array()
                .unwrap()
                .iter()
                .all(|tool| tool["namespace"] == "renamed")
        );
        let roundtrip = decode::decode_request(&changed).unwrap();
        assert!(
            roundtrip
                .tools
                .as_ref()
                .unwrap()
                .iter()
                .all(|tool| tool.namespace.as_deref() == Some("renamed"))
        );
        for tool in request.tools.as_mut().unwrap() {
            tool.namespace = None;
        }
        let cleared = encode::encode_request(&request, "feature-model");
        assert!(
            cleared["tools"]
                .as_array()
                .unwrap()
                .iter()
                .all(|tool| tool.get("namespace").is_none())
        );
        let roundtrip = decode::decode_request(&cleared).unwrap();
        assert!(
            roundtrip
                .tools
                .as_ref()
                .unwrap()
                .iter()
                .all(|tool| tool.namespace.is_none())
        );
    }
}

#[tokio::test]
async fn function_and_custom_calls_preserve_namespace_and_mutation() {
    for (kind, field, payload) in [
        ("function_call", "arguments", "{\"x\":1}"),
        ("custom_tool_call", "input", "print(1)\n"),
    ] {
        let mut item = json!({"type":kind,"id":"fc_1","call_id":"call_1","name":"run","namespace":"tools","status":"completed"});
        item[field] = json!(payload);
        let mut decoded = decode::decode_response(&response(vec![item.clone()])).unwrap();
        let Node::ToolCall {
            namespace,
            arguments,
            extra_body,
            ..
        } = &mut decoded.output[0]
        else {
            panic!("typed call")
        };
        assert_eq!(namespace.as_deref(), Some("tools"));
        assert!(!extra_body.contains_key("namespace"));
        *namespace = None;
        *arguments = "changed".into();
        let encoded = encode::encode_response(&decoded, "feature-model");
        assert!(encoded["output"][0].get("namespace").is_none());
        assert_eq!(encoded["output"][0][field], "changed");
        let events = decode_stream(fixture_stream(&item, None)).await;
        assert!(terminal_output(&events).iter().any(|node| matches!(node, Node::ToolCall { namespace:Some(ns), arguments, .. } if ns=="tools" && arguments==payload)));
        let (wire, frames) = encode_events(events).await;
        let added = frames
            .iter()
            .find(|frame| frame["type"] == "response.output_item.added")
            .unwrap();
        assert_eq!(added["item"]["namespace"], "tools");
        let back = decode_stream(wire).await;
        assert!(
            terminal_output(&back).iter().any(
                |node| matches!(node, Node::ToolCall { namespace:Some(ns), .. } if ns=="tools")
            )
        );
    }
}

#[tokio::test]
async fn tool_results_keep_streamed_payloads() {
    for kind in ["function_call_output", "custom_tool_call_output"] {
        let item = json!({"type":kind,"id":"fco_1","call_id":"call_1","output":[{"type":"input_text","text":"result"},{"type":"input_image","image_url":"https://example.com/a.png"},{"type":"input_file","file_url":"https://example.com/a.pdf","filename":"source.pdf","detail":"low"},{"type":"input_file","file_data":"data:application/pdf;base64,JVBERi0x","filename":"inline.pdf"}]});
        let decoded = decode::decode_response(&response(vec![item.clone()])).unwrap();
        assert!(matches!(&decoded.output[0],Node::ToolResult { content, .. } if content.len()==4));
        let events = decode_stream(fixture_stream(&item, None)).await;
        assert!(events.iter().any(|event| matches!(event,UrpStreamEvent::NodeDone { node: Node::ToolResult { content, .. }, .. } if content.len()==4)));
        for (_, frames) in [
            encode_events(events).await,
            synthetic_events(&decoded).await,
        ] {
            let completed = frames
                .iter()
                .find(|frame| frame["type"] == "response.completed")
                .unwrap();
            assert_eq!(completed["response"]["output"][0]["output"], item["output"]);
            assert_eq!(
                frames
                    .iter()
                    .filter(|frame| frame["type"] == "response.output_item.done")
                    .count(),
                1
            );
            assert!(
                frames
                    .iter()
                    .all(|frame| frame["type"] != "response.content_part.added")
            );
            let done = frames
                .iter()
                .find(|frame| frame["type"] == "response.output_item.done")
                .unwrap();
            assert_eq!(
                done["item"]["output"], item["output"],
                "live item.done must use input_* media too"
            );
            let back = decode_stream(
                frames
                    .iter()
                    .map(|value| frame(value["type"].as_str().unwrap(), value.clone()))
                    .collect(),
            )
            .await;
            assert!(
                matches!(&terminal_output(&back)[0], Node::ToolResult { content, .. } if content.len() == 4)
            );
        }
    }
}

#[tokio::test]
async fn generated_image_has_no_payload_snapshot() {
    let item = json!({"type":"image_generation_call","id":"ig_1","status":"completed","result":"YQ==","output_format":"png","size":"1024x1024"});
    let mut decoded = decode::decode_response(&response(vec![item.clone()])).unwrap();
    let Node::Image {
        id,
        source,
        extra_body,
        ..
    } = &mut decoded.output[0]
    else {
        panic!("typed image")
    };
    assert_eq!(
        extra_body[super::RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY],
        json!({})
    );
    *id = Some("ig_changed".into());
    *source = ImageSource::Base64 {
        media_type: "image/webp".into(),
        data: "Yg==".into(),
    };
    let encoded = encode::encode_response(&decoded, "feature-model");
    assert_eq!(encoded["output"][0]["id"], "ig_changed");
    assert_eq!(encoded["output"][0]["result"], "Yg==");
    assert_eq!(encoded["output"][0]["output_format"], "webp");
    let events = decode_stream(fixture_stream(&item, None)).await;
    for node in terminal_output(&events) {
        if let Node::Image { extra_body, .. } = node {
            assert_eq!(
                extra_body[super::RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY],
                json!({})
            );
            assert!(!extra_body.contains_key("result"));
        }
    }
    let (wire, frames) = encode_events(events).await;
    let terminal = frames
        .iter()
        .find(|frame| frame["type"] == "response.completed")
        .unwrap();
    assert_eq!(terminal["response"]["output"][0]["result"], "YQ==");
    assert!(terminal_output(&decode_stream(wire).await).iter().any(|node|matches!(node,Node::Image { source:ImageSource::Base64 { data,.. },.. } if data=="YQ==")));
}

#[test]
fn native_terminal_details_survive_only_compatible_finish_reason() {
    for (status, details) in [
        ("incomplete", json!({"reason":"max_messages","limit":4})),
        ("incomplete", json!({"reason":"future_reason"})),
        ("cancelled", Value::Null),
        ("failed", Value::Null),
    ] {
        let mut wire = response(vec![]);
        wire["status"] = json!(status);
        wire["incomplete_details"] = details.clone();
        wire["error"] = json!({"code":"provider_code","message":"provider message"});
        let mut decoded = decode::decode_response(&wire).unwrap();
        assert_eq!(
            decoded.extra_body[super::RESPONSES_RESPONSE_SOURCE_EXTRA_KEY],
            json!({})
        );
        let encoded = encode::encode_response(&decoded, "feature-model");
        assert_eq!(encoded["status"], status);
        assert_eq!(encoded["incomplete_details"], details);
        decoded.finish_reason = Some(FinishReason::Stop);
        let encoded = encode::encode_response(&decoded, "feature-model");
        assert_eq!(encoded["status"], "completed");
        assert!(encoded["incomplete_details"].is_null());
        assert!(encoded["error"].is_null());
    }
}

async fn synthetic_events(response: &UrpResponse) -> (String, Vec<Value>) {
    let (tx, mut rx) = mpsc::channel(4096);
    super::stream_encode::openai_responses::emit_synthetic_responses_stream(
        "feature-model",
        response,
        None,
        None,
        tx,
    )
    .await
    .unwrap();
    let mut frames = Vec::new();
    while let Some(frame) = rx.recv().await {
        frames.push(Ok::<_, std::convert::Infallible>(frame));
    }
    let body = axum::response::Sse::new(futures_util::stream::iter(frames))
        .into_response()
        .into_body();
    let wire = String::from_utf8(
        axum::body::to_bytes(body, usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    let values = wire
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter(|data| *data != "[DONE]")
        .map(|data| serde_json::from_str(data).unwrap())
        .collect();
    (wire, values)
}

#[tokio::test]
async fn reasoning_summary_content_and_encrypted_roundtrip() {
    let item = json!({"type":"reasoning","id":"rs_1","content":[{"type":"reasoning_text","text":"分析"}],
        "summary":[{"type":"summary_text","text":"first"},{"type":"summary_text","text":"second"}],
        "encrypted_content":"opaque","status":"completed"});
    let mut decoded = decode::decode_response(&response(vec![item.clone()])).unwrap();
    let Node::Reasoning {
        content,
        summary,
        encrypted,
        extra_body,
        ..
    } = &mut decoded.output[0]
    else {
        panic!("typed reasoning")
    };
    assert_eq!(content.as_deref(), Some("分析"));
    assert_eq!(summary.as_deref(), Some("firstsecond"));
    assert!(!extra_body.contains_key("content"));
    assert!(!extra_body.contains_key("summary"));
    *summary = Some("changed".into());
    *encrypted = None;
    let encoded = encode::encode_response(&decoded, "feature-model");
    assert_eq!(encoded["output"][0]["summary"][0]["text"], "changed");
    assert!(encoded["output"][0].get("encrypted_content").is_none());
    let events = decode_stream(fixture_stream(&item, None)).await;
    assert!(terminal_output(&events).iter().any(|node|matches!(node,Node::Reasoning { content:Some(content),summary:Some(summary),encrypted:Some(_),.. } if content=="分析" && summary=="firstsecond")));
    let (wire, _) = encode_events(events).await;
    assert!(
        terminal_output(&decode_stream(wire).await)
            .iter()
            .any(|node| matches!(
                node,
                Node::Reasoning {
                    encrypted: Some(_),
                    ..
                }
            ))
    );
    let (wire, _) = synthetic_events(&decoded).await;
    assert!(terminal_output(&decode_stream(wire).await).iter().any(|node|matches!(node,Node::Reasoning { summary:Some(summary),encrypted:None,.. } if summary=="changed")));
}

#[tokio::test]
async fn refusal_uses_native_lifecycles_in_both_stream_encoders() {
    let item = json!({"type":"message","id":"msg_refusal","role":"assistant","status":"completed", "content":[{"type":"refusal","refusal":"cannot comply"}]});
    let decoded = decode::decode_response(&response(vec![item.clone()])).unwrap();
    assert!(
        decoded
            .output
            .iter()
            .any(|node| matches!(node,Node::Refusal { content,.. } if content=="cannot comply"))
    );
    assert_eq!(
        encode::encode_response(&decoded, "feature-model")["output"][0]["content"][0]["refusal"],
        "cannot comply"
    );
    let stream = decode_stream(fixture_stream(&item, None)).await;
    assert!(stream.iter().any(|event|matches!(event,UrpStreamEvent::NodeDelta { delta:super::NodeDelta::Refusal { content },.. } if content=="cannot comply")));
    for (wire, frames) in [
        encode_events(stream).await,
        synthetic_events(&decoded).await,
    ] {
        assert!(
            frames
                .iter()
                .any(|frame| frame["type"] == "response.refusal.delta"
                    && frame["delta"] == "cannot comply")
        );
        assert!(
            frames
                .iter()
                .any(|frame| frame["type"] == "response.refusal.done"
                    && frame["refusal"] == "cannot comply")
        );
        assert!(
            !frames
                .iter()
                .any(|frame| frame["type"] == "response.output_text.delta")
        );
        assert!(
            terminal_output(&decode_stream(wire).await).iter().any(
                |node| matches!(node,Node::Refusal { content,.. } if content=="cannot comply")
            )
        );
    }
}

#[tokio::test]
async fn citations_are_typed_and_deleted_values_do_not_reappear() {
    let citation = json!({"type":"url_citation","url":"https://example.com","title":"Example","start_index":0,"end_index":6});
    let item = json!({"type":"message","id":"msg_cite","role":"assistant","status":"completed", "content":[{"type":"output_text","text":"answer","annotations":[citation.clone()]}]});
    let mut decoded = decode::decode_response(&response(vec![item.clone()])).unwrap();
    let Node::Text {
        citations,
        extra_body,
        ..
    } = decoded
        .output
        .iter_mut()
        .find(|node| matches!(node, Node::Text { .. }))
        .unwrap()
    else {
        unreachable!()
    };
    assert_eq!(citations, &vec![citation.clone()]);
    assert!(!extra_body.contains_key("annotations"));
    citations.clear();
    assert_eq!(
        encode::encode_response(&decoded, "feature-model")["output"][0]["content"][0]["annotations"],
        json!([])
    );
    let mut wire = fixture_stream(&item, None);
    let insert = frame(
        "response.output_text.annotation.added",
        json!({"output_index":0,"content_index":0,"item_id":"msg_cite","annotation_index":0,"annotation":citation}),
    );
    wire = wire.replacen(
        "event: response.content_part.done",
        &(insert + "event: response.content_part.done"),
        1,
    );
    let events = decode_stream(wire).await;
    assert!(events.iter().any(|event|matches!(event,UrpStreamEvent::NodeDelta { delta:super::NodeDelta::Text { citations,.. },.. } if citations.len()==1)));
    let (wire, frames) = encode_events(events).await;
    assert_eq!(
        frames
            .iter()
            .filter(|frame| frame["type"] == "response.output_text.annotation.added")
            .count(),
        1
    );
    assert!(
        terminal_output(&decode_stream(wire).await)
            .iter()
            .any(|node| matches!(node,Node::Text { citations,.. } if citations.len()==1))
    );
}

#[tokio::test]
async fn missing_terminal_output_keeps_typed_phase_and_citations_without_replay_copies() {
    let citation = json!({"type":"url_citation","url":"https://example.com","title":"Example","start_index":0,"end_index":6});
    for with_part_start in [false, true] {
        let mut wire = frame("response.created", json!({"response":response(vec![])}));
        wire.push_str(&frame("response.output_item.added", json!({"output_index":0,"item":{"type":"message","id":"msg_fallback","role":"assistant","phase":"final_answer","status":"in_progress","content":[],"future":"kept"}})));
        if with_part_start {
            wire.push_str(&frame("response.content_part.added", json!({"output_index":0,"content_index":0,"item_id":"msg_fallback","part":{"type":"output_text","text":"","annotations":[]}})));
        }
        wire.push_str(&frame(
            "response.output_text.delta",
            json!({"output_index":0,"content_index":0,"item_id":"msg_fallback","delta":"answer"}),
        ));
        wire.push_str(&frame("response.output_text.annotation.added", json!({"output_index":0,"content_index":0,"item_id":"msg_fallback","annotation_index":0,"annotation":citation})));
        let mut terminal = response(vec![]);
        terminal.as_object_mut().unwrap().remove("output");
        wire.push_str(&frame("response.completed", json!({"response":terminal})));
        let mut events = decode_stream(wire).await;
        for node in terminal_output(&events) {
            match node {
                Node::Text {
                    id,
                    phase,
                    citations,
                    extra_body,
                    ..
                } => {
                    assert_eq!(id.as_deref(), Some("msg_fallback"));
                    assert_eq!(phase.as_deref(), Some("final_answer"));
                    assert_eq!(citations, &vec![citation.clone()]);
                    assert!(!extra_body.contains_key("phase"));
                    assert!(!extra_body.contains_key("annotations"));
                }
                Node::NextDownstreamEnvelopeExtra { extra_body } => {
                    assert!(!extra_body.contains_key("id"));
                    assert!(!extra_body.contains_key("phase"));
                    assert_eq!(extra_body.get("future"), Some(&json!("kept")));
                }
                _ => {}
            }
        }
        assert!(events.iter().any(|event| matches!(event, UrpStreamEvent::NodeStart { header:super::NodeHeader::Text { phase:Some(phase),.. },extra_body,.. } if phase=="final_answer" && !extra_body.contains_key("phase"))));
        let (_, full_frames) = encode_events(events.clone()).await;
        let full_terminal = full_frames
            .iter()
            .find(|frame| frame["type"] == "response.completed")
            .unwrap();
        assert_eq!(
            full_terminal["response"]["output"][0]["phase"],
            "final_answer"
        );
        assert_eq!(
            full_terminal["response"]["output"][0]["content"][0]["annotations"],
            json!([citation.clone()])
        );

        let clear_node = |node: &mut Node| {
            if let Node::Text {
                phase, citations, ..
            } = node
            {
                *phase = None;
                citations.clear();
            }
        };
        for event in &mut events {
            match event {
                UrpStreamEvent::NodeStart {
                    header:
                        super::NodeHeader::Text {
                            phase, citations, ..
                        },
                    ..
                } => {
                    *phase = None;
                    citations.clear();
                }
                UrpStreamEvent::NodeDelta {
                    delta: super::NodeDelta::Text { citations, .. },
                    ..
                } => citations.clear(),
                UrpStreamEvent::NodeDone { node, .. } => clear_node(node),
                UrpStreamEvent::ResponseDone { output, .. } => {
                    output.iter_mut().for_each(clear_node)
                }
                _ => {}
            }
        }
        let canonical = UrpResponse {
            output: terminal_output(&events).to_vec(),
            ..decode::decode_response(&response(vec![])).unwrap()
        };
        assert!(
            encode::encode_response(&canonical, "feature-model")["output"][0]
                .get("phase")
                .is_none()
        );
        for (wire, frames) in [
            encode_events(events).await,
            synthetic_events(&canonical).await,
        ] {
            let terminal = frames
                .iter()
                .find(|frame| frame["type"] == "response.completed")
                .unwrap();
            assert!(terminal["response"]["output"][0].get("phase").is_none());
            assert_eq!(
                terminal["response"]["output"][0]["content"][0]["annotations"],
                json!([])
            );
            assert!(
                terminal_output(&decode_stream(wire).await)
                    .iter()
                    .all(|node| !matches!(node, Node::Text { phase: Some(_), .. }))
            );
        }
    }
}

#[tokio::test]
async fn responses_nonofficial_media_output_returns_explicit_error() {
    for block in [
        json!({"type":"output_image","url":"https://example.com/a.png"}),
        json!({"type":"output_file","url":"https://example.com/a.pdf"}),
    ] {
        let item = json!({"type":"message","id":"msg_media","role":"assistant","status":"completed","content":[block]});
        let decoded = decode::decode_response(&response(vec![item.clone()])).unwrap();
        assert!(
            decoded
                .output
                .iter()
                .any(|node| matches!(node, Node::Image { .. } | Node::File { .. }))
        );
        assert!(encode::encode_response_checked(&decoded, "feature-model").is_err());
        assert_eq!(
            encode::encode_response(&decoded, "feature-model")["error"]["code"],
            "unsupported_media"
        );
        let events = decode_stream(fixture_stream(&item, None)).await;
        assert_responses_media_stream_errors(&decoded, events).await;
    }
}

async fn assert_responses_media_stream_errors(response: &UrpResponse, events: Vec<UrpStreamEvent>) {
    for synthetic in [false, true] {
        let (out_tx, mut out_rx) = mpsc::channel(4096);
        let result = if synthetic {
            super::stream_encode::openai_responses::emit_synthetic_responses_stream(
                "feature-model",
                response,
                None,
                None,
                out_tx,
            )
            .await
        } else {
            let (tx, rx) = mpsc::channel(4096);
            for event in events.clone() {
                tx.send(event).await.unwrap();
            }
            drop(tx);
            super::stream_encode::openai_responses::encode_urp_stream_as_responses(
                rx,
                out_tx,
                "feature-model",
                std::time::Instant::now(),
                None,
                false,
            )
            .await
        };
        assert_eq!(result.unwrap_err().code, "unsupported_media");
        let mut frames = Vec::new();
        while let Some(event) = out_rx.recv().await {
            frames.push(Ok::<_, std::convert::Infallible>(event));
        }
        let body = axum::response::Sse::new(futures_util::stream::iter(frames))
            .into_response()
            .into_body();
        let wire = String::from_utf8(
            axum::body::to_bytes(body, usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        let values: Vec<Value> = wire
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .filter(|data| *data != "[DONE]")
            .map(|data| serde_json::from_str(data).unwrap())
            .collect();
        assert!(values.iter().any(|frame| frame["type"] == "response.failed"
            && frame["response"]["error"]["code"] == "unsupported_media"));
        assert!(
            values
                .iter()
                .all(|frame| frame["type"] != "response.completed"
                    && frame["type"] != "response.content_part.added")
        );
        assert!(!wire.contains("output_image"));
        assert!(!wire.contains("output_file"));
    }
}

#[test]
fn namespace_definitions_and_selectors_are_owned_by_urp() {
    for stream in [false, true] {
        let mut request=decode::decode_request(&json!({"model":"feature-model","input":"run","stream":stream,
            "tools":[{"type":"namespace","name":"functions","description":"commands","tools":[
                {"type":"function","name":"lookup","description":"Find","parameters":{"type":"object"},"strict":true},
                {"type":"custom","name":"patch","format":{"type":"grammar","syntax":"lark","definition":"start: /.+/"}}]}],
            "tool_choice":{"type":"allowed_tools","mode":"required","tools":[{"type":"function","name":"lookup","namespace":"functions"},{"type":"custom","name":"patch","namespace":"functions"}]}})).unwrap();
        let wrapper = &mut request.tools.as_mut().unwrap()[0];
        assert!(wrapper.extra_body.get("tools").is_none());
        assert_eq!(wrapper.tools.as_ref().unwrap().len(), 2);
        wrapper.tools.as_mut().unwrap()[0]
            .function
            .as_mut()
            .unwrap()
            .name = "changed".into();
        wrapper.tools.as_mut().unwrap().remove(1);
        let encoded = encode::encode_request(&request, "feature-model");
        assert_eq!(encoded["tools"][0]["tools"].as_array().unwrap().len(), 1);
        assert_eq!(encoded["tools"][0]["tools"][0]["name"], "changed");
        assert_eq!(encoded["tool_choice"]["tools"][0]["namespace"], "functions");
        let back = decode::decode_request(&encoded).unwrap();
        assert_eq!(
            back.tools.unwrap()[0].tools.as_ref().unwrap()[0]
                .function
                .as_ref()
                .unwrap()
                .name,
            "changed"
        );
    }
}

#[tokio::test]
async fn native_incomplete_and_cancelled_terminal_events_remain_terminal() {
    for (status, reason) in [("incomplete", "max_messages"), ("cancelled", "")] {
        let mut value = response(vec![]);
        value["status"] = json!(status);
        if !reason.is_empty() {
            value["incomplete_details"] = json!({"reason":reason});
        }
        let mut wire = frame("response.created", json!({"response":response(vec![])}));
        wire.push_str(&frame(
            &format!("response.{status}"),
            json!({"response":value}),
        ));
        wire.push_str("data: [DONE]\n\n");
        let events = decode_stream(wire).await;
        let (_, frames) = encode_events(events).await;
        assert!(
            frames
                .iter()
                .any(|frame| frame["type"] == format!("response.{status}")),
            "{frames:#?}"
        );
        assert!(
            !frames
                .iter()
                .any(|frame| frame["type"] == "response.completed")
        );
    }
}

#[tokio::test]
async fn custom_input_remains_byte_exact_when_it_looks_like_json() {
    for input in ["1.0", "{\"amount\":1.0}", "  [2.0] \n"] {
        let item = json!({"type":"custom_tool_call","id":"ctc_exact","call_id":"call_exact","name":"raw","input":input,"status":"completed"});
        let decoded = decode::decode_response(&response(vec![item.clone()])).unwrap();
        assert_eq!(
            encode::encode_response(&decoded, "feature-model")["output"][0]["input"],
            input
        );
        let request =
            decode::decode_request(&json!({"model":"feature-model","input":[item.clone()]}))
                .unwrap();
        assert_eq!(
            encode::encode_request(&request, "feature-model")["input"][0]["input"],
            input
        );
        let events = decode_stream(fixture_stream(&item, None)).await;
        for (wire, frames) in [
            encode_events(events).await,
            synthetic_events(&decoded).await,
        ] {
            let terminal = frames
                .iter()
                .find(|frame| frame["type"] == "response.completed")
                .unwrap();
            assert_eq!(terminal["response"]["output"][0]["input"], input);
            assert!(
                terminal_output(&decode_stream(wire).await)
                    .iter()
                    .any(|node| matches!(node,Node::ToolCall { arguments,.. } if arguments==input))
            );
        }
    }
}

#[tokio::test]
async fn provider_item_identity_and_config_mutation_override_native_shape() {
    let item = json!({"type":"web_search_call","id":"ws_old","status":"completed","action":{"type":"search","query":"q"}});
    let mut decoded = decode::decode_response(&response(vec![item.clone()])).unwrap();
    let Node::ProviderItem { id, item_type, .. } = &mut decoded.output[0] else {
        panic!("provider item")
    };
    *id = Some("ws_new".into());
    *item_type = "future_search_call".into();
    let encoded = encode::encode_response(&decoded, "feature-model");
    assert_eq!(encoded["output"][0]["id"], "ws_new");
    assert_eq!(encoded["output"][0]["type"], "future_search_call");
    let (wire, _) = synthetic_events(&decoded).await;
    assert!(terminal_output(&decode_stream(wire).await).iter().any(|node|matches!(node,Node::ProviderItem { id:Some(id),item_type,.. } if id=="ws_new" && item_type=="future_search_call")));
    let Node::ProviderItem { id, .. } = &mut decoded.output[0] else {
        unreachable!()
    };
    *id = None;
    assert!(
        encode::encode_response(&decoded, "feature-model")["output"][0]
            .get("id")
            .is_none()
    );
    let mut request=decode::decode_request(&json!({"model":"feature-model","input":[item],"tools":[{"type":"file_search","vector_store_ids":["vs_old"],"max_num_results":2}]})).unwrap();
    let tool = &mut request.tools.as_mut().unwrap()[0];
    assert_eq!(tool.origin_protocol, Some(ProviderProtocol::Responses));
    assert!(tool.extra_body.is_empty());
    tool.config.as_mut().unwrap()["vector_store_ids"] = json!(["vs_new"]);
    tool.config
        .as_mut()
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove("max_num_results");
    let encoded = encode::encode_request(&request, "feature-model");
    assert_eq!(encoded["tools"][0]["vector_store_ids"], json!(["vs_new"]));
    assert!(encoded["tools"][0].get("max_num_results").is_none());
}

#[tokio::test]
async fn failed_and_error_events_prevent_later_success() {
    for event_name in ["error", "response.failed"] {
        let failure = if event_name == "error" {
            json!({"code":"tool_error","message":"failed","param":"tools"})
        } else {
            let mut value = response(vec![]);
            value["status"] = json!("failed");
            value["error"] = json!({"code":"tool_error","message":"failed","type":"server_error","param":"tools"});
            json!({"response":value})
        };
        let mut wire = frame("response.created", json!({"response":response(vec![])}));
        wire.push_str(&frame(event_name, failure));
        wire.push_str(&frame(
            "response.completed",
            json!({"response":response(vec![])}),
        ));
        let events = decode_stream(wire).await;
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, UrpStreamEvent::Error { .. }))
                .count(),
            1
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, UrpStreamEvent::ResponseDone { .. }))
        );
        let (_, frames) = encode_events(events).await;
        assert!(frames.iter().any(|frame| frame["type"] == "response.failed"
            && frame["response"]["error"]["code"] == "tool_error"));
        assert!(
            !frames
                .iter()
                .any(|frame| frame["type"] == "response.completed")
        );
    }
}

#[tokio::test]
async fn bare_native_error_envelopes_fail_without_losing_failed_response_semantics() {
    let error =
        json!({"error":{"code":"invalid_request","message":"invalid input","param":"input"}});
    assert_eq!(
        decode::decode_response(&error).unwrap_err(),
        "invalid input"
    );
    let mut failed = response(vec![]);
    failed["status"] = json!("failed");
    failed["error"] = error["error"].clone();
    let decoded = decode::decode_response(&failed).unwrap();
    assert_eq!(decoded.finish_reason, Some(FinishReason::Other));
    assert_eq!(
        encode::encode_response(&decoded, "feature-model")["error"],
        error["error"]
    );

    for prefix in [
        String::new(),
        frame("response.created", json!({"response":response(vec![])})),
    ] {
        let wire = format!(
            "{prefix}data: {error}\n\n{}",
            frame("response.completed", json!({"response":response(vec![])}))
        );
        let events = decode_stream(wire).await;
        assert_eq!(events.iter().filter(|event| matches!(event, UrpStreamEvent::Error { code:Some(code),message,.. } if code=="invalid_request" && message=="invalid input")).count(), 1);
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, UrpStreamEvent::ResponseDone { .. }))
        );
        let (_, frames) = encode_events(events).await;
        assert!(frames.iter().any(|frame| frame["type"] == "response.failed"
            && frame["response"]["error"]["message"] == "invalid input"));
        assert!(
            !frames
                .iter()
                .any(|frame| frame["type"] == "response.completed")
        );
    }
}

#[tokio::test]
async fn signed_tool_calls_survive_responses_live_and_synthetic_client_loops() {
    let item = json!({"type":"function_call","id":"fc_signed","call_id":"call_signed","name":"lookup","arguments":"{}","status":"completed"});
    let mut canonical = decode::decode_response(&response(vec![item.clone()])).unwrap();
    let set_signature = |node: &mut Node| {
        if let Node::ToolCall { signature, .. } = node {
            *signature = Some(json!("gemini-signature"));
        }
    };
    canonical.output.iter_mut().for_each(set_signature);
    let mut live = decode_stream(fixture_stream(&item, None)).await;
    for event in &mut live {
        match event {
            UrpStreamEvent::NodeStart {
                header: super::NodeHeader::ToolCall { signature, .. },
                ..
            } => *signature = Some(json!("gemini-signature")),
            UrpStreamEvent::NodeDone { node, .. } => set_signature(node),
            UrpStreamEvent::ResponseDone { output, .. } => {
                output.iter_mut().for_each(set_signature)
            }
            _ => {}
        }
    }
    for (_, frames) in [
        encode_events(live).await,
        synthetic_events(&canonical).await,
    ] {
        let terminal = frames
            .iter()
            .find(|frame| frame["type"] == "response.completed")
            .unwrap();
        let output = terminal["response"]["output"].as_array().unwrap();
        assert_eq!(
            output.len(),
            2,
            "one adapter-local transport must precede the call"
        );
        assert_eq!(output[0]["type"], "reasoning");
        assert_eq!(output[0]["encrypted_content"], "gemini-signature");
        assert_eq!(output[1]["type"], "function_call");
        assert!(output[1].get("signature").is_none());
        let restored =
            decode::decode_request(&json!({"model":"feature-model","input":output})).unwrap();
        assert_eq!(restored.input.len(), 1);
        assert!(
            matches!(&restored.input[0], Node::ToolCall { call_id,signature:Some(signature),.. } if call_id=="call_signed" && signature=="gemini-signature")
        );
        let native = super::encode::gemini::encode_request(&restored, "feature-model");
        assert_eq!(
            native["contents"][0]["parts"][0]["thoughtSignature"],
            "gemini-signature"
        );
    }
    assert_eq!(canonical.output.len(), 1);
    assert!(
        matches!(&canonical.output[0], Node::ToolCall { signature:Some(signature),.. } if signature=="gemini-signature")
    );
}

#[tokio::test]
async fn response_start_usage_and_unknown_fields_have_one_owner() {
    let mut start = response(vec![]);
    start["future"] = json!("kept");
    let events = decode_stream(
        frame("response.created", json!({"response":start}))
            + &frame("response.completed", json!({"response":response(vec![])})),
    )
    .await;
    let UrpStreamEvent::ResponseStart {
        usage, extra_body, ..
    } = &events[0]
    else {
        panic!("start")
    };
    assert_eq!(usage.as_ref().unwrap().input_tokens, 4);
    assert!(!extra_body.contains_key("usage"));
    assert_eq!(
        extra_body[super::RESPONSES_STREAM_START_SOURCE_EXTRA_KEY],
        json!({})
    );
    let (_, frames) = encode_events(events).await;
    assert_eq!(frames[0]["response"]["future"], "kept");
    assert_eq!(frames[0]["response"]["usage"]["input_tokens"], 4);
}

#[tokio::test]
async fn streaming_namespace_deletion_cannot_be_replayed_from_extras() {
    let item = json!({"type":"function_call","id":"fc_ns","call_id":"call_ns","name":"run","namespace":"old_namespace","arguments":"{}","status":"completed"});
    let mut events = decode_stream(fixture_stream(&item, None)).await;
    for event in &mut events {
        match event {
            UrpStreamEvent::NodeStart {
                header: super::NodeHeader::ToolCall { namespace, .. },
                extra_body,
                ..
            } => {
                *namespace = None;
                assert!(!extra_body.contains_key("namespace"));
            }
            UrpStreamEvent::NodeDone {
                node: Node::ToolCall { namespace, .. },
                extra_body,
                ..
            } => {
                *namespace = None;
                assert!(!extra_body.contains_key("namespace"));
            }
            UrpStreamEvent::ResponseDone { output, .. } => {
                for node in output {
                    if let Node::ToolCall { namespace, .. } = node {
                        *namespace = None;
                    }
                }
            }
            _ => {}
        }
    }
    let (wire, frames) = encode_events(events).await;
    assert!(!wire.contains("old_namespace"));
    assert!(
        frames
            .iter()
            .filter_map(|frame| frame.get("item"))
            .all(|item| item.get("namespace").is_none())
    );
    assert!(
        terminal_output(&decode_stream(wire).await)
            .iter()
            .any(|node| matches!(
                node,
                Node::ToolCall {
                    namespace: None,
                    ..
                }
            ))
    );
}

#[tokio::test]
async fn partial_generated_image_uses_current_typed_bytes() {
    let item = json!({"type":"image_generation_call","id":"ig_partial","status":"completed","result":"Yg==","output_format":"png"});
    for event_name in [
        "response.image_generation_call.partial_image",
        "response.image_generation.partial_image",
        "image_generation.partial_image",
    ] {
        let insert = frame(
            event_name,
            json!({"output_index":0,"item_id":"ig_partial","partial_image_index":0,"partial_image_b64":"YQ=="}),
        );
        let wire = fixture_stream(&item, None).replacen(
            "event: response.output_item.done",
            &(insert + "event: response.output_item.done"),
            1,
        );
        let mut events = decode_stream(wire).await;
        let event = events
            .iter_mut()
            .find(|event| {
                matches!(
                    event,
                    UrpStreamEvent::NodeDelta {
                        delta: super::NodeDelta::Image { .. },
                        ..
                    }
                )
            })
            .unwrap();
        let UrpStreamEvent::NodeDelta {
            delta: super::NodeDelta::Image { source },
            extra_body,
            ..
        } = event
        else {
            unreachable!()
        };
        assert!(!extra_body.contains_key("partial_image_b64"));
        *source = ImageSource::Base64 {
            media_type: "image/png".into(),
            data: "Yw==".into(),
        };
        let (_, frames) = encode_events(events).await;
        let partial = frames
            .iter()
            .find(|frame| frame["type"] == "response.image_generation_call.partial_image")
            .unwrap();
        assert_eq!(partial["partial_image_b64"], "Yw==");
    }
}

#[test]
fn responses_assistant_easy_input_media_and_file_metadata_are_typed() {
    for stream in [false, true] {
        for file in [
            json!({"file_id":"file_1"}),
            json!({"file_url":"https://example.com/report.pdf"}),
            json!({"file_data":"data:application/pdf;base64,JVBERi0x"}),
        ] {
            let mut part = file;
            part["type"] = json!("input_file");
            part["filename"] = json!("old.pdf");
            part["detail"] = json!("high");
            let mut request = decode::decode_request(&json!({"model":"feature-model","stream":stream,"input":[{
                "role":"assistant","type":"message","content":[{"type":"input_text","text":"history"},
                    {"type":"input_image","image_url":"data:image/png;base64,YQ==","detail":"low"},part]
            }]})).unwrap();
            let file = request
                .input
                .iter_mut()
                .find(|node| matches!(node, Node::File { .. }))
                .unwrap();
            let Node::File {
                metadata,
                extra_body,
                ..
            } = file
            else {
                unreachable!()
            };
            assert_eq!(metadata.filename.as_deref(), Some("old.pdf"));
            assert_eq!(metadata.detail.as_deref(), Some("high"));
            assert!(!extra_body.contains_key("filename") && !extra_body.contains_key("detail"));
            metadata.filename = Some("changed.pdf".into());
            metadata.detail = Some("low".into());
            extra_body.insert("filename".into(), json!("stale.pdf"));
            extra_body.insert("detail".into(), json!("high"));
            let wire = encode::encode_request_checked(&request, "feature-model").unwrap();
            let content: Vec<&Value> = wire["input"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|item| item["content"].as_array().unwrap())
                .collect();
            assert_eq!(content[0]["type"], "input_text");
            assert_eq!(content[1]["type"], "input_image");
            assert_eq!(content[1]["detail"], "low");
            assert_eq!(content[2]["type"], "input_file");
            assert_eq!(content[2]["filename"], "changed.pdf");
            assert_eq!(content[2]["detail"], "low");
            assert!(
                wire["input"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|item| item.get("status").is_none() && item.get("id").is_none())
            );
            let back = decode::decode_request(&wire).unwrap();
            assert!(back.input.iter().any(|node| matches!(node, Node::File { metadata, .. } if metadata.filename.as_deref() == Some("changed.pdf") && metadata.detail.as_deref() == Some("low"))));
            if let Node::File { metadata, .. } = request
                .input
                .iter_mut()
                .find(|node| matches!(node, Node::File { .. }))
                .unwrap()
            {
                metadata.filename = None;
                metadata.detail = None;
            }
            let wire = encode::encode_request_checked(&request, "feature-model").unwrap();
            let file = wire["input"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|item| item["content"].as_array().unwrap())
                .find(|part| part["type"] == "input_file")
                .unwrap();
            assert!(file.get("filename").is_none() && file.get("detail").is_none());
        }
    }
}

#[tokio::test]
async fn responses_tool_result_metadata_mutation_wins_in_all_output_modes() {
    let item = json!({"type":"function_call_output","id":"fco_metadata","call_id":"call_1","output":[
        {"type":"input_file","file_id":"file_1","filename":"old.pdf","detail":"high"}]});
    let mut decoded = decode::decode_response(&response(vec![item])).unwrap();
    for delete in [false, true] {
        let Node::ToolResult {
            content,
            name,
            namespace,
            extra_body: result_extra,
            ..
        } = &mut decoded.output[0]
        else {
            panic!("result")
        };
        *name = (!delete).then(|| "changed_name".into());
        *namespace = (!delete).then(|| "changed_namespace".into());
        result_extra.insert("name".into(), json!("stale_name"));
        result_extra.insert("namespace".into(), json!("stale_namespace"));
        result_extra.insert("output".into(), json!("stale output"));
        let super::ToolResultContent::File {
            metadata,
            extra_body,
            ..
        } = &mut content[0]
        else {
            panic!("file")
        };
        metadata.filename = (!delete).then(|| "changed.pdf".into());
        metadata.detail = (!delete).then(|| "low".into());
        extra_body.insert("filename".into(), json!("stale.pdf"));
        extra_body.insert("detail".into(), json!("high"));
        let wire = encode::encode_response_checked(&decoded, "feature-model").unwrap();
        let expected_item = wire["output"][0].clone();
        let expected = expected_item["output"].clone();
        assert_eq!(
            expected_item.get("name"),
            (!delete).then(|| json!("changed_name")).as_ref()
        );
        assert_eq!(
            expected_item.get("namespace"),
            (!delete).then(|| json!("changed_namespace")).as_ref()
        );
        if delete {
            assert!(expected[0].get("filename").is_none() && expected[0].get("detail").is_none());
        } else {
            assert_eq!(expected[0]["filename"], "changed.pdf");
            assert_eq!(expected[0]["detail"], "low");
        }
        let node = decoded.output[0].clone();
        let Node::ToolResult {
            id,
            tool_type,
            call_id,
            namespace,
            name,
            signature,
            ..
        } = &node
        else {
            unreachable!()
        };
        let header = super::NodeHeader::ToolResult {
            id: id.clone(),
            tool_type: *tool_type,
            call_id: call_id.clone(),
            namespace: namespace.clone(),
            name: name.clone(),
            signature: signature.clone(),
        };
        let events = vec![
            UrpStreamEvent::NodeStart {
                node_index: 0,
                header,
                extra_body: Default::default(),
            },
            UrpStreamEvent::NodeDone {
                node_index: 0,
                node,
                usage: None,
                extra_body: Default::default(),
            },
            UrpStreamEvent::ResponseDone {
                output: decoded.output.clone(),
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                extra_body: Default::default(),
            },
        ];
        for (_, frames) in [
            encode_events(events).await,
            synthetic_events(&decoded).await,
        ] {
            let done = &frames
                .iter()
                .find(|frame| frame["type"] == "response.output_item.done")
                .unwrap()["item"];
            assert_eq!(done["output"], expected);
            assert_eq!(done.get("name"), expected_item.get("name"));
            assert_eq!(done.get("namespace"), expected_item.get("namespace"));
        }
    }
}

#[test]
fn responses_compatible_content_shapes_and_audio_are_typed() {
    for stream in [false, true] {
        for content in [
            json!({"type":"input_audio","input_audio":{"data":"YQ==","format":"wav"}}),
            json!(["before",{"type":"input_audio","input_audio":{"data":"YQ==","format":"wav"}},"after"]),
        ] {
            let canonical=decode::decode_request(&json!({"model":"feature-model","stream":stream,"input":[{"role":"user","content":content}]})).unwrap();
            assert!(canonical.input.iter().any(|node|matches!(node,Node::Audio {source:super::AudioSource::Base64 {media_type,data},..} if media_type=="audio/wav" && data=="YQ==")));
            assert_eq!(
                canonical.input.len(),
                if content.is_array() { 3 } else { 1 }
            );
            assert!(encode::encode_request_checked(&canonical, "feature-model").is_err());
            let chat =
                super::encode::openai_chat::encode_request_checked(&canonical, "feature-model")
                    .unwrap();
            assert!(
                chat["messages"][0]["content"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|part| part["type"] == "input_audio")
            );
        }
        for kind in ["function_call_output", "custom_tool_call_output"] {
            let canonical=decode::decode_request(&json!({"model":"feature-model","stream":stream,"input":[{"type":kind,"call_id":"call_audio","output":{"type":"output_audio","data":"YQ==","format":"wav"}}]})).unwrap();
            assert!(
                matches!(&canonical.input[0],Node::ToolResult {content,..} if matches!(&content[0],super::ToolResultContent::File {source:super::FileSource::Base64 {media_type,data},..} if media_type=="audio/wav" && data=="YQ=="))
            );
        }
    }
}

#[tokio::test]
async fn responses_compatible_audio_decodes_before_target_validation() {
    let item = json!({"type":"message","id":"msg_audio","role":"assistant","content":[{"type":"output_audio","data":"YQ==","format":"wav"}]});
    let canonical = decode::decode_response(&response(vec![item.clone()])).unwrap();
    assert!(
        matches!(&canonical.output[0],Node::Audio {source:super::AudioSource::Base64 {media_type,data},..} if media_type=="audio/wav" && data=="YQ==")
    );
    let events = decode_stream(fixture_stream(&item, None)).await;
    assert!(
        terminal_output(&events).iter().any(|node| matches!(node,Node::Audio {source:super::AudioSource::Base64 {media_type,data},..} if media_type=="audio/wav" && data=="YQ=="))
    );
    assert_responses_media_stream_errors(&canonical, events).await;
    assert!(super::encode::gemini::encode_response_checked(&canonical, "feature-model").is_ok());
}

#[tokio::test]
async fn responses_media_names_inside_opaque_data_do_not_trigger_validation() {
    let items = vec![
        json!({"type":"function_call","id":"fc_audio","call_id":"call_audio","name":"inspect","arguments":{"type":"input_audio","content":[{"type":"audio"}]}}),
        json!({"type":"custom_tool_call","id":"ctc_audio","call_id":"custom_audio","name":"inspect","input":"{\"type\":\"audio\"}"}),
        json!({"type":"vendor_item","id":"vendor_audio","part":{"type":"output_audio"},"content":[{"type":"audio"}]}),
    ];
    for stream in [false, true] {
        assert!(
            decode::decode_request(&json!({"model":"feature-model","stream":stream,"input":items}))
                .is_ok()
        );
    }
    for item in &items {
        assert!(decode::decode_response(&response(vec![item.clone()])).is_ok());
        let events = decode_stream(fixture_stream(item, None)).await;
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, UrpStreamEvent::Error { .. }))
        );
        assert!(!terminal_output(&events).is_empty());
    }
    let mut wire = frame("response.created", json!({"response":response(vec![])}));
    wire.push_str(&frame(
        "response.vendor_notice",
        json!({"part":{"type":"audio"}}),
    ));
    wire.push_str(&frame(
        "response.completed",
        json!({"response":response(vec![])}),
    ));
    wire.push_str("data: [DONE]\n\n");
    let events = decode_stream(wire).await;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, UrpStreamEvent::Error { .. }))
    );
    assert!(terminal_output(&events).is_empty());
}

#[tokio::test]
async fn responses_unknown_content_preserves_its_message_position() {
    let block = json!({"type":"vendor_content","payload":{"type":"audio","value":7}});
    let item = json!({"type":"message","id":"msg_vendor","role":"assistant","content":[{"type":"output_text","text":"before"},block,{"type":"output_text","text":"after"}]});
    let canonical = decode::decode_response(&response(vec![item.clone()])).unwrap();
    assert_eq!(canonical.output.len(), 3);
    assert!(
        matches!(&canonical.output[1],Node::ProviderItem {origin_protocol:ProviderProtocol::Responses,extra_body,..} if extra_body.len()==1)
    );
    let encoded = encode::encode_response_checked(&canonical, "feature-model").unwrap();
    assert_eq!(encoded["output"][0]["content"][1], block);
    let streamed = decode_stream(fixture_stream(&item, None)).await;
    assert!(
        !streamed
            .iter()
            .any(|event| matches!(event, UrpStreamEvent::Error { .. })),
        "{streamed:#?}"
    );
    let (_, frames) = encode_events(streamed).await;
    let terminal = frames
        .iter()
        .find(|frame| frame["type"] == "response.completed")
        .unwrap_or_else(|| panic!("{frames:#?}"));
    assert_eq!(
        terminal["response"]["output"][0]["content"][1], block,
        "{terminal:#?}"
    );
    let (tx, mut rx) = mpsc::channel(4096);
    super::stream_encode::openai_responses::emit_synthetic_responses_stream(
        "feature-model",
        &canonical,
        None,
        None,
        tx,
    )
    .await
    .unwrap();
    let mut frames = Vec::new();
    while let Some(event) = rx.recv().await {
        frames.push(Ok::<_, std::convert::Infallible>(event));
    }
    let body = axum::response::Sse::new(futures_util::stream::iter(frames))
        .into_response()
        .into_body();
    let wire = String::from_utf8(
        axum::body::to_bytes(body, usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    let decoded = decode_stream(wire).await;
    assert!(
        terminal_output(&decoded)
            .iter()
            .any(|node| matches!(node,Node::ProviderItem {body,..} if body==&block))
    );
    for stream in [false, true] {
        let canonical=decode::decode_request(&json!({"model":"feature-model","stream":stream,"input":[{"role":"user","content":block}]})).unwrap();
        assert_eq!(
            encode::encode_request_checked(&canonical, "feature-model").unwrap()["input"][0]["content"]
                [0],
            block
        );
    }
}

#[tokio::test]
async fn responses_streamed_media_extras_do_not_duplicate_typed_sources() {
    for block in [
        json!({"type":"output_image","image_url":"https://example.com/a.png","detail":"high"}),
        json!({"type":"output_file","file_data":"data:application/pdf;base64,JVBERi0xLjQK","filename":"a.pdf"}),
    ] {
        let item = json!({"type":"message","id":"msg_media","role":"assistant","content":[block]});
        let events = decode_stream(fixture_stream(&item, None)).await;
        for event in &events {
            let extras = match event {
                UrpStreamEvent::NodeStart { extra_body, .. }
                | UrpStreamEvent::NodeDone { extra_body, .. } => Some(extra_body),
                _ => None,
            };
            if let Some(extras) = extras {
                for key in ["image_url", "file_data", "filename", "detail"] {
                    assert!(!extras.contains_key(key), "{event:?}");
                }
            }
        }
    }
}

#[tokio::test]
async fn responses_missing_terminal_content_keeps_duplicate_media_empty_text_and_typed_mutations() {
    let file = json!({"type":"output_file","file_data":"data:application/pdf;base64,JVBERi0xLjQK","filename":"old.pdf"});
    let item = json!({"type":"message","id":"msg_multi","role":"assistant","phase":"final_answer","content":[
        {"type":"output_text","text":"before"},file,file,{"type":"output_text","text":""},{"type":"output_text","text":"after"}]});
    let wire = fixture_stream(&item, None).replace(
        &frame(
            "response.completed",
            json!({"response":response(vec![item.clone()])}),
        ),
        &frame("response.completed", json!({"response":response(vec![])})),
    );
    let events = decode_stream(wire).await;
    let nodes = terminal_output(&events)
        .iter()
        .filter(|node| !matches!(node, Node::NextDownstreamEnvelopeExtra { .. }))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(nodes.len(), 5, "{events:#?}");
    assert!(matches!(&nodes[1], Node::File { .. }) && matches!(&nodes[2], Node::File { .. }));
    assert!(matches!(&nodes[3],Node::Text {content,..} if content.is_empty()));
    let mut request = decode::decode_request(&json!({"model":"feature-model","input":[]})).unwrap();
    request.input = nodes;
    for node in &mut request.input {
        match node {
            Node::File { metadata, .. } => metadata.filename = None,
            Node::Text { phase, content, .. } => {
                *phase = None;
                if content == "before" {
                    *content = "changed".into();
                }
            }
            _ => {}
        }
    }
    let encoded = encode::encode_request_checked(&request, "feature-model").unwrap();
    let blocks = encoded["input"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|item| item["content"].as_array().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(blocks.len(), 5);
    assert_eq!(blocks[0]["text"], "changed");
    assert_eq!(blocks[3]["text"], "");
    assert_eq!(
        blocks
            .iter()
            .filter(|part| part["type"] == "input_file")
            .count(),
        2
    );
    assert!(blocks.iter().all(|part| part.get("filename").is_none()));
    assert!(
        encoded["input"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item.get("phase").is_none())
    );
}

#[tokio::test]
async fn responses_missing_terminal_output_merges_done_media_with_unfinished_text_by_content_index()
{
    for include_text_start in [false, true] {
        let mut wire = frame("response.created", json!({"response":response(vec![])}));
        wire.push_str(&frame("response.output_item.added", json!({"output_index":0,"item":{
            "type":"message","id":"msg_partial","role":"assistant","phase":"final_answer","content":[]
        }})));
        let file = json!({"type":"output_file","file_data":"data:application/pdf;base64,JVBERi0xLjQK","filename":"partial.pdf"});
        for event in ["response.content_part.added", "response.content_part.done"] {
            wire.push_str(&frame(
                event,
                json!({"output_index":0,"content_index":0,"item_id":"msg_partial","part":file}),
            ));
        }
        if include_text_start {
            wire.push_str(&frame("response.content_part.added",json!({"output_index":0,"content_index":1,"item_id":"msg_partial","part":{"type":"output_text","text":""}})));
        }
        for delta in ["partial ", "text"] {
            wire.push_str(&frame(
                "response.output_text.delta",
                json!({"output_index":0,"content_index":1,"item_id":"msg_partial","delta":delta}),
            ));
        }
        let annotation = json!({"type":"url_citation","url":"https://example.com/source","title":"Source","start_index":0,"end_index":7});
        wire.push_str(&frame("response.output_text.annotation.added",json!({"output_index":0,"content_index":1,"item_id":"msg_partial","annotation_index":0,"annotation":annotation})));
        wire.push_str(&frame(
            "response.completed",
            json!({"response":response(vec![])}),
        ));
        wire.push_str("data: [DONE]\n\n");
        let events = decode_stream(wire).await;
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, UrpStreamEvent::Error { .. })),
            "{events:#?}"
        );
        let nodes = terminal_output(&events)
            .iter()
            .filter(|node| !matches!(node, Node::NextDownstreamEnvelopeExtra { .. }))
            .collect::<Vec<_>>();
        assert_eq!(nodes.len(), 2, "{events:#?}");
        assert!(
            matches!(&nodes[0],Node::File {metadata,..} if metadata.filename.as_deref()==Some("partial.pdf"))
        );
        assert!(
            matches!(&nodes[1],Node::Text {content,citations,phase,..} if content=="partial text" && citations==&vec![annotation] && phase.as_deref()==Some("final_answer"))
        );
        let text_starts = events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    UrpStreamEvent::NodeStart {
                        header: super::NodeHeader::Text { .. },
                        ..
                    }
                )
            })
            .count();
        assert_eq!(text_starts, 1, "{events:#?}");
    }
}

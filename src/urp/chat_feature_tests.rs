use super::{Node, UrpResponse, UrpStreamEvent};
use super::{decode::openai_chat as decode, encode::openai_chat as encode};
use axum::response::{IntoResponse, Sse, sse::Event};
use serde_json::{Value, json};
use std::{collections::HashSet, convert::Infallible};
use tokio::sync::mpsc;

fn response(message: Value, finish: &str) -> Value {
    json!({"id":"chat_feature","object":"chat.completion","created":1,"model":"chat-test","choices":[{"index":0,"message":message,"finish_reason":finish}],"usage":{"prompt_tokens":11,"completion_tokens":7,"total_tokens":18}})
}
async fn decode_stream(frames: Vec<Value>) -> Vec<UrpStreamEvent> {
    decode_stream_with_audio_format(frames, None).await
}
async fn decode_stream_with_audio_format(
    frames: Vec<Value>,
    audio_format: Option<&str>,
) -> Vec<UrpStreamEvent> {
    let mut wire: String = frames
        .iter()
        .map(|frame| format!("data: {frame}\n\n"))
        .collect();
    wire.push_str("data: [DONE]\n\n");
    decode_chat_wire(wire, audio_format).await
}
async fn decode_chat_wire(wire: String, audio_format: Option<&str>) -> Vec<UrpStreamEvent> {
    let upstream = reqwest::Response::from(
        axum::http::Response::builder()
            .header("content-type", "text/event-stream")
            .body(wire)
            .unwrap(),
    );
    let request = crate::handlers::UrpRequest {
        model: "chat-test".into(),
        max_multiplier: None,
        audio_output_format: audio_format.map(str::to_string),
        server_tool_usage_classes: vec![],
        messages_custom_tool_names: HashSet::new(),
        affinity_explicit: None,
        affinity_prefix_hash: String::new(),
    };
    let (tx, mut rx) = mpsc::channel(1024);
    super::stream_decode::openai_chat::stream_chat_to_urp_events(
        &request, upstream, tx, None, None, 1000,
    )
    .await
    .unwrap();
    let mut events = vec![];
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    events
}
async fn wire_values(mut rx: mpsc::Receiver<Event>) -> Vec<Value> {
    let mut events = vec![];
    while let Some(event) = rx.recv().await {
        events.push(Ok::<_, Infallible>(event));
    }
    let response = Sse::new(futures_util::stream::iter(events)).into_response();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let wire = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(wire.lines().filter(|line| *line == "data: [DONE]").count(), 1);
    assert!(!wire.lines().any(|line| line.starts_with("event:")));
    wire.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter(|line| *line != "[DONE]")
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
async fn encode_stream(events: Vec<UrpStreamEvent>) -> Vec<Value> {
    encode_stream_with_limit(events, None).await
}
async fn encode_stream_with_limit(events: Vec<UrpStreamEvent>, limit: Option<usize>) -> Vec<Value> {
    let (tx, rx) = mpsc::channel(1024);
    for event in events {
        tx.send(event).await.unwrap();
    }
    drop(tx);
    let (out_tx, out_rx) = mpsc::channel(1024);
    super::stream_encode::openai_chat::encode_urp_stream_as_chat(
        rx,
        out_tx,
        "chat-test",
        limit,
        false,
    )
    .await
    .unwrap();
    wire_values(out_rx).await
}
async fn synthetic_stream(response: &UrpResponse) -> Vec<Value> {
    synthetic_stream_with_limit(response, None).await
}
async fn synthetic_stream_with_limit(response: &UrpResponse, limit: Option<usize>) -> Vec<Value> {
    let (tx, rx) = mpsc::channel(1024);
    super::stream_encode::openai_chat::emit_synthetic_chat_stream("chat-test", response, limit, tx)
        .await
        .unwrap();
    wire_values(rx).await
}
fn terminal(events: &[UrpStreamEvent]) -> UrpResponse {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            UrpStreamEvent::ResponseDone {
                outcome: _,
                finish_reason,
                usage,
                output,
                extra_body,
            } => Some(UrpResponse {
                outcome: None,
                id: "chat_feature".into(),
                model: "chat-test".into(),
                created_at: Some(1),
                finish_reason: *finish_reason,
                usage: usage.clone(),
                output: output.clone(),
                extra_body: extra_body.clone(),
            }),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing terminal: {events:?}"))
}
fn assert_message(response: &UrpResponse, expected: &Value) {
    let output = encode::encode_response(response, "chat-test");
    let message = &output["choices"][0]["message"];
    for (key, value) in expected.as_object().unwrap() {
        assert_eq!(&message[key], value, "message field {key}");
    }
}
async fn feature(message: Value, finish: &str) {
    let original = response(message.clone(), finish);
    let urp = decode::decode_response(&original).unwrap();
    assert_message(&urp, &message);
    let events = decode_stream(vec![
        json!({"id":"chat_feature","choices":[{"index":0,"delta":message,"finish_reason":null}]}),
        json!({"choices":[{"index":0,"delta":{},"finish_reason":finish}]}),
        json!({"choices":[],"usage":{"prompt_tokens":11,"completion_tokens":7,"total_tokens":18}}),
    ])
    .await;
    let streamed = terminal(&events);
    assert_message(&streamed, &message);
    assert_eq!(streamed.usage.as_ref().unwrap().input_tokens, 11);
    assert_message(
        &terminal(&decode_stream(encode_stream(events).await).await),
        &message,
    );
    assert_message(
        &terminal(&decode_stream(synthetic_stream(&urp).await).await),
        &message,
    );
}

#[tokio::test]
async fn chat_function_calls_bidirectional() {
    feature(json!({"role":"assistant","content":null,"tool_calls":[{"id":"call_weather","type":"function","function":{"name":"weather","arguments":"{\"city\":\"深圳\"}"}}]}),"tool_calls").await;
}
#[tokio::test]
async fn chat_custom_tool_calls_bidirectional() {
    feature(json!({"role":"assistant","content":null,"tool_calls":[{"id":"call_patch","type":"custom","custom":{"name":"apply_patch","input":"*** Begin Patch\n*** End Patch"}}]}),"tool_calls").await;
}
#[tokio::test]
async fn chat_legacy_functions_bidirectional() {
    feature(json!({"role":"assistant","content":null,"function_call":{"name":"weather","arguments":"{\"city\":\"Paris\"}"}}),"function_call").await;
}
#[tokio::test]
async fn chat_refusal_bidirectional() {
    feature(
        json!({"role":"assistant","content":null,"refusal":"I cannot help with that."}),
        "stop",
    )
    .await;
}
#[tokio::test]
async fn chat_reasoning_details_bidirectional() {
    feature(json!({"role":"assistant","content":"Answer","reasoning_details":[{"type":"reasoning.text","text":"Consider","index":0,"format":"native"},{"type":"reasoning.summary","summary":"Brief","index":1},{"type":"reasoning.encrypted","data":"cipher","index":2}]}),"stop").await;
}
#[tokio::test]
async fn chat_scalar_reasoning_bidirectional() {
    let message = json!({"role":"assistant","content":"Answer","reasoning_content":"Consider","reasoning_opaque":"cipher"});
    let urp = decode::decode_response(&response(message.clone(), "stop")).unwrap();
    let source = vec![json!({"choices":[{"delta":message,"finish_reason":"stop"}]})];
    let events = decode_stream(source).await;
    let cases = vec![
        decode::decode_response(&encode::encode_response(&urp, "chat-test")).unwrap(),
        terminal(&events),
        terminal(&decode_stream(encode_stream(events).await).await),
        terminal(&decode_stream(synthetic_stream(&urp).await).await),
    ];
    for response in cases {
        assert!(response.output.iter().any(
            |node| matches!(node,Node::Reasoning {content:Some(content),..} if content=="Consider")
        ));
        assert!(response.output.iter().any(|node|matches!(node,Node::Reasoning {encrypted:Some(encrypted),..} if encrypted=="cipher")));
    }
}
#[tokio::test]
async fn chat_citations_bidirectional() {
    feature(json!({"role":"assistant","content":"A cited answer.","annotations":[{"type":"url_citation","url_citation":{"url":"https://example.com","title":"Example","start_index":0,"end_index":1}}]}),"stop").await;
}
#[tokio::test]
async fn chat_generated_audio_bidirectional() {
    feature(json!({"role":"assistant","content":null,"audio":{"id":"audio_1","data":"aGVsbG8=","transcript":"Hello","expires_at":1234}}),"stop").await;
}
#[tokio::test]
async fn chat_text_phase_bidirectional() {
    feature(
        json!({"role":"assistant","content":"Answer","phase":"final_answer"}),
        "stop",
    )
    .await;
}

#[test]
fn chat_request_features_both_stream_modes() {
    for stream in [false, true] {
        let source = json!({"model":"chat-test","stream":stream,"messages":[{"role":"developer","content":"Rules"},{"role":"user","content":[{"type":"text","text":"Look"},{"type":"image_url","image_url":{"url":"https://example.com/i.png","detail":"high"}},{"type":"input_audio","input_audio":{"data":"YQ==","format":"wav"}},{"type":"file","file":{"file_id":"file_1"}}]}],"tools":[{"type":"function","function":{"name":"weather","description":"Forecast","parameters":{"type":"object"},"strict":true}},{"type":"custom","custom":{"name":"patch","format":{"type":"grammar","syntax":"lark","definition":"start: WORD"}}}],"tool_choice":{"type":"allowed_tools","allowed_tools":{"mode":"required","tools":[{"type":"function","function":{"name":"weather"}}]}},"stop":["END"],"verbosity":"low","user":"caller","max_completion_tokens":123,"parallel_tool_calls":false,"response_format":{"type":"json_schema","json_schema":{"name":"answer","schema":{"type":"object"},"strict":true}},"web_search_options":{"search_context_size":"low"},"audio":{"voice":"alloy","format":"wav"},"modalities":["text","audio"]});
        let urp = decode::decode_request(&source).unwrap();
        let encoded = encode::encode_request(&urp, "chat-test");
        for key in [
            "tools",
            "tool_choice",
            "stop",
            "verbosity",
            "user",
            "parallel_tool_calls",
            "response_format",
            "web_search_options",
            "audio",
            "modalities",
        ] {
            assert_eq!(encoded[key], source[key], "{key}");
        }
        let again = decode::decode_request(&encoded).unwrap();
        assert_eq!(
            serde_json::to_value(&again.input).unwrap(),
            serde_json::to_value(&urp.input).unwrap()
        );
        assert_eq!(again.max_output_tokens, Some(123));
    }
}
#[test]
fn chat_tool_results_and_legacy_history_both_stream_modes() {
    for stream in [false, true] {
        for (messages, kind) in [
            (
                json!([{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"custom","custom":{"name":"patch","input":"patch"}}]},{"role":"tool","tool_call_id":"call_1","content":"ok"}]),
                super::ToolCallType::Custom,
            ),
            (
                json!([{"role":"assistant","content":null,"function_call":{"name":"old","arguments":"{}"}},{"role":"function","name":"old","content":"ok"}]),
                super::ToolCallType::Function,
            ),
        ] {
            let urp = decode::decode_request(
                &json!({"model":"chat-test","stream":stream,"messages":messages}),
            )
            .unwrap();
            assert!(urp.input.iter().any(|node|matches!(node,Node::ToolResult {tool_type,content,..} if *tool_type==kind && !content.is_empty())));
            let again = decode::decode_request(&encode::encode_request(&urp, "chat-test")).unwrap();
            assert_eq!(
                serde_json::to_value(again.input).unwrap(),
                serde_json::to_value(urp.input).unwrap()
            );
        }
    }
}
#[test]
fn chat_legacy_result_name_uses_typed_state_both_stream_modes() {
    for stream in [false, true] {
        let source = json!({
            "model":"chat-test", "stream":stream,
            "messages":[{"role":"function","name":"old","content":"ok","vendor":"kept"}]
        });
        let mut request = decode::decode_request(&source).unwrap();
        let Node::ToolResult {
            name, extra_body, ..
        } = &request.input[0]
        else {
            panic!("expected typed tool result");
        };
        assert_eq!(name.as_deref(), Some("old"));
        assert_eq!(
            extra_body[super::CHAT_LEGACY_FUNCTION_RESULT_EXTRA_KEY],
            json!(true)
        );
        let encoded = encode::encode_request(&request, "chat-test");
        assert_eq!(encoded["messages"], source["messages"]);
        let decoded = decode::decode_request(&encoded).unwrap();
        assert_eq!(
            serde_json::to_value(&decoded.input).unwrap(),
            serde_json::to_value(&request.input).unwrap()
        );

        let Node::ToolResult {
            name, extra_body, ..
        } = &mut request.input[0]
        else {
            unreachable!();
        };
        *name = Some("updated".into());
        extra_body.insert(
            super::CHAT_LEGACY_FUNCTION_RESULT_EXTRA_KEY.into(),
            json!("stale"),
        );
        let encoded = encode::encode_request(&request, "chat-test");
        assert_eq!(encoded["messages"][0]["role"], "function");
        assert_eq!(encoded["messages"][0]["name"], "updated");
        let decoded = decode::decode_request(&encoded).unwrap();
        assert!(
            matches!(&decoded.input[0], Node::ToolResult { name: Some(name), .. } if name == "updated")
        );

        let Node::ToolResult { name, .. } = &mut request.input[0] else {
            unreachable!();
        };
        *name = None;
        let encoded = encode::encode_request(&request, "chat-test");
        assert_eq!(encoded["messages"][0]["role"], "function");
        assert!(encoded["messages"][0].get("name").is_none());
        assert_eq!(encoded["messages"][0]["vendor"], "kept");
        let decoded = decode::decode_request(&encoded).unwrap();
        assert!(matches!(
            &decoded.input[0],
            Node::ToolResult { name: None, .. }
        ));
    }
}
#[test]
fn chat_tool_result_identity_uses_typed_state_both_stream_modes() {
    for stream in [false, true] {
        for role in ["tool", "function"] {
            let mut result =
                json!({"role":role,"id":"old-id","name":"old-name","content":"ok","vendor":"kept"});
            if role == "tool" {
                result["tool_call_id"] = json!("call_1");
            }
            let mut request = decode::decode_request(
                &json!({"model":"chat-test","stream":stream,"messages":[result.clone()]}),
            )
            .unwrap();
            let Node::ToolResult {
                id,
                name,
                extra_body,
                ..
            } = &request.input[0]
            else {
                panic!("expected tool result");
            };
            assert_eq!(id.as_deref(), Some("old-id"));
            assert_eq!(name.as_deref(), Some("old-name"));
            assert!(!extra_body.contains_key("id"));
            assert!(!extra_body.contains_key("name"));
            let encoded = encode::encode_request(&request, "chat-test");
            assert_eq!(encoded["messages"][0], result);
            let decoded = decode::decode_request(&encoded).unwrap();
            assert_eq!(
                serde_json::to_value(&decoded.input).unwrap(),
                serde_json::to_value(&request.input).unwrap()
            );

            let Node::ToolResult {
                id,
                name,
                extra_body,
                ..
            } = &mut request.input[0]
            else {
                unreachable!();
            };
            *id = Some("new-id".into());
            *name = Some("new-name".into());
            extra_body.insert("id".into(), json!("stale-id"));
            extra_body.insert("name".into(), json!("stale-name"));
            let encoded = encode::encode_request(&request, "chat-test");
            assert_eq!(encoded["messages"][0]["id"], "new-id");
            assert_eq!(encoded["messages"][0]["name"], "new-name");

            let Node::ToolResult { id, name, .. } = &mut request.input[0] else {
                unreachable!();
            };
            *id = None;
            *name = None;
            let encoded = encode::encode_request(&request, "chat-test");
            assert!(encoded["messages"][0].get("id").is_none());
            assert!(encoded["messages"][0].get("name").is_none());
            assert_eq!(encoded["messages"][0]["vendor"], "kept");
            let decoded = decode::decode_request(&encoded).unwrap();
            assert!(matches!(
                &decoded.input[0],
                Node::ToolResult {
                    id: None,
                    name: None,
                    ..
                }
            ));
        }
    }
}
#[test]
fn chat_typed_audio_and_citation_deletion_wins() {
    let mut urp=decode::decode_response(&response(json!({"role":"assistant","content":"Answer","annotations":[{"type":"url_citation","url_citation":{"url":"https://example.com","start_index":0,"end_index":1}}],"audio":{"id":"old","data":"old","transcript":"old","expires_at":1}}),"stop")).unwrap();
    for node in &mut urp.output {
        match node {
            Node::Audio {
                metadata,
                source: super::AudioSource::Base64 { data, .. },
                extra_body,
                ..
            } => {
                metadata.reference_id = None;
                metadata.transcript = None;
                metadata.expires_at = None;
                *data = "new".into();
                assert!(!extra_body.contains_key("data"));
            }
            Node::Text {
                logprobs: _,
                content,
                citations,
                ..
            } => {
                *content = "new".into();
                citations.clear();
            }
            _ => {}
        }
    }
    let wire = encode::encode_response(&urp, "chat-test");
    let message = &wire["choices"][0]["message"];
    assert_eq!(message["audio"], json!({"data":"new"}));
    assert!(message.get("annotations").is_none());
    assert_eq!(message["content"], "new");
}
#[tokio::test]
async fn chat_errors_and_missing_terminal_are_failures() {
    for frames in [
        vec![json!({"error":{"code":429,"message":"rate limit"}})],
        vec![json!({"choices":[{"delta":{"content":"partial"}}]})],
    ] {
        let events = decode_stream(frames).await;
        assert!(
            events
                .iter()
                .any(|event| matches!(event, UrpStreamEvent::Error { .. }))
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, UrpStreamEvent::ResponseDone { .. }))
        );
    }
}
#[tokio::test]
async fn chat_typed_terminal_finish_reason_wins_over_tools() {
    let mut urp=decode::decode_response(&response(json!({"role":"assistant","content":null,"tool_calls":[{"id":"c","type":"function","function":{"name":"f","arguments":"{}"}}]}),"tool_calls")).unwrap();
    urp.finish_reason = Some(super::FinishReason::Length);
    let terminal = terminal(&decode_stream(synthetic_stream(&urp).await).await);
    assert_eq!(terminal.finish_reason, Some(super::FinishReason::Length));
}

#[tokio::test]
async fn chat_custom_json_looking_input_remains_verbatim() {
    for input in ["1.0", " { \"n\": 1.0 } "] {
        feature(json!({"role":"assistant","content":null,"tool_calls":[{"id":"custom_json","type":"custom","custom":{"name":"freeform","input":input}}]}),"tool_calls").await;
        let mut response=decode::decode_response(&response(json!({"role":"assistant","tool_calls":[{"id":"custom_json","type":"custom","custom":{"name":"freeform","input":input}}]}),"tool_calls")).unwrap();
        super::integerize_tool_call_nodes(&mut response.output);
        assert!(matches!(&response.output[0],Node::ToolCall {arguments,..} if arguments==input));
    }
}
#[tokio::test]
async fn chat_fragmented_audio_and_refusal_preserve_ordered_payloads() {
    let events=decode_stream(vec![json!({"choices":[{"delta":{"audio":{"id":"audio_1","data":"aGVs","transcript":"Hel"}}}]}),json!({"choices":[{"delta":{"audio":{"data":"bG8=","transcript":"lo","expires_at":10},"refusal":"Cannot "}}]}),json!({"choices":[{"delta":{"refusal":"comply"},"finish_reason":"stop"}]})]).await;
    let response = terminal(&events);
    assert_message(
        &response,
        &json!({"audio":{"id":"audio_1","data":"aGVsbG8=","transcript":"Hello","expires_at":10},"refusal":"Cannot comply"}),
    );
    assert_message(
        &terminal(&decode_stream(encode_stream(events).await).await),
        &json!({"audio":{"id":"audio_1","data":"aGVsbG8=","transcript":"Hello","expires_at":10},"refusal":"Cannot comply"}),
    );
}

#[tokio::test]
async fn chat_error_typed_code_and_message_override_shape() {
    let mut events=decode_stream(vec![json!({"error":{"code":429,"message":"old error","type":"rate_limit","metadata":{"provider_code":429,"trace":"keep"}}})]).await;
    let error = events
        .iter_mut()
        .find(|event| matches!(event, UrpStreamEvent::Error { .. }))
        .unwrap();
    if let UrpStreamEvent::Error {
        code,
        message,
        extra_body,
    } = error
    {
        assert!(
            !serde_json::to_string(extra_body)
                .unwrap()
                .contains("old error")
        );
        *code = None;
        *message = "changed error".into();
    }
    let wire = encode_stream(events).await;
    let error = wire.iter().find_map(|frame| frame.get("error")).unwrap();
    assert_eq!(error["message"], "changed error");
    assert!(error.get("code").is_none());
    assert_eq!(error["metadata"]["trace"], "keep");
}

async fn assert_typed_media_feature(block: Value, kind: &str) {
    let message = json!({"role":"assistant","content":[block]});
    let native = decode::decode_response(&response(message.clone(), "stop")).unwrap();
    let events = decode_stream(vec![
        json!({"choices":[{"delta":message,"finish_reason":"stop"}]}),
    ])
    .await;
    let streamed = terminal(&events);
    for response in [&native, &streamed] {
        assert_eq!(response.output.len(), 1);
        let serialized = serde_json::to_value(&response.output[0]).unwrap();
        assert_eq!(
            serialized["type"], kind,
            "media must use its canonical node kind"
        );
        let mut cross_protocol = response.output.clone();
        super::retain_provider_items_for_protocol(
            &mut cross_protocol,
            super::ProviderProtocol::Messages,
        );
        assert_eq!(
            cross_protocol.len(),
            1,
            "typed media survives protocol filtering"
        );
    }
    assert!(encode::encode_response_checked(&native, "chat-test").is_err());
    assert_eq!(
        encode::encode_response(&native, "chat-test")["error"]["code"],
        "unsupported_media"
    );
    assert_chat_media_stream_errors(&native, events).await;
}

async fn assert_chat_media_stream_errors(response: &UrpResponse, events: Vec<UrpStreamEvent>) {
    for synthetic in [false, true] {
        let (out_tx, out_rx) = mpsc::channel(1024);
        let result = if synthetic {
            super::stream_encode::openai_chat::emit_synthetic_chat_stream(
                "chat-test",
                response,
                None,
                out_tx,
            )
            .await
        } else {
            let (tx, rx) = mpsc::channel(1024);
            for event in events.clone() {
                tx.send(event).await.unwrap();
            }
            drop(tx);
            super::stream_encode::openai_chat::encode_urp_stream_as_chat(
                rx,
                out_tx,
                "chat-test",
                None,
                false,
            )
            .await
        };
        assert_eq!(result.unwrap_err().code, "unsupported_media");
        let frames = wire_values(out_rx).await;
        assert!(
            frames
                .iter()
                .any(|frame| frame["error"]["code"] == "unsupported_media")
        );
        assert!(
            frames
                .iter()
                .all(|frame| frame["choices"][0]["finish_reason"].is_null())
        );
        assert!(
            frames
                .iter()
                .all(|frame| !frame["choices"][0]["delta"]["content"].is_array())
        );
    }
}

#[tokio::test]
async fn chat_assistant_image_response_returns_explicit_error() {
    assert_typed_media_feature(json!({"type":"image_url","image_url":{"url":"https://example.com/image.png","detail":"high"}}), "image").await;
    assert_typed_media_feature(json!({"type":"image_url","image_url":{"url":"data:image/png;base64,aGVsbG8=","detail":null}}), "image").await;
}

#[tokio::test]
async fn chat_provider_identity_is_authoritative_in_all_response_modes() {
    fn edit_identity(node: &mut Node, deleted: bool) {
        if let Node::ProviderItem {
            id,
            item_type,
            extra_body,
            ..
        } = node
        {
            *id = (!deleted).then(|| "new-id".to_string());
            *item_type = if deleted { "" } else { "new-kind" }.to_string();
            extra_body.insert("id".into(), json!("stale-id"));
            extra_body.insert("type".into(), json!("stale-kind"));
        }
    }

    for audio_reference in [false, true] {
        let message = if audio_reference {
            json!({"role":"assistant","content":null,"audio":{"id":"old-id","vendor":"kept"}})
        } else {
            json!({"role":"assistant","content":[{"type":"old-kind","id":"old-id","payload":{"kept":true}}]})
        };
        feature(message.clone(), "stop").await;
        let native = decode::decode_response(&response(message.clone(), "stop")).unwrap();
        let original_events = decode_stream(vec![
            json!({"choices":[{"delta":message,"finish_reason":"stop"}]}),
        ])
        .await;
        for deleted in [false, true] {
            let mut expected = message.clone();
            let target = if audio_reference {
                &mut expected["audio"]
            } else {
                &mut expected["content"][0]
            };
            let target = target.as_object_mut().unwrap();
            target.remove("id");
            if !deleted {
                target.insert("id".into(), json!("new-id"));
            }
            if !audio_reference {
                target.remove("type");
                if !deleted {
                    target.insert("type".into(), json!("new-kind"));
                }
            }

            let mut changed = native.clone();
            for node in &mut changed.output {
                edit_identity(node, deleted);
            }
            let before = serde_json::to_value(&changed).unwrap();
            assert_message(&changed, &expected);
            assert_message(
                &terminal(&decode_stream(synthetic_stream(&changed).await).await),
                &expected,
            );
            assert_eq!(
                serde_json::to_value(&changed).unwrap(),
                before,
                "encoding must not rewrite canonical body"
            );
            let mut events = original_events.clone();
            for event in &mut events {
                match event {
                    UrpStreamEvent::NodeDone { node, .. } => edit_identity(node, deleted),
                    UrpStreamEvent::ResponseDone {
                        outcome: _, output, ..
                    } => {
                        for node in output {
                            edit_identity(node, deleted);
                        }
                    }
                    _ => {}
                }
            }
            assert_message(
                &terminal(&decode_stream(encode_stream(events).await).await),
                &expected,
            );
            let fallback = vec![UrpStreamEvent::ResponseDone {
                outcome: None,
                finish_reason: changed.finish_reason,
                usage: changed.usage.clone(),
                output: changed.output.clone(),
                extra_body: changed.extra_body.clone(),
            }];
            assert_message(
                &terminal(&decode_stream(encode_stream(fallback).await).await),
                &expected,
            );

            for stream in [false, true] {
                let mut request = decode::decode_request(
                    &json!({"model":"chat-test","stream":stream,"messages":[message.clone()]}),
                )
                .unwrap();
                for node in &mut request.input {
                    edit_identity(node, deleted);
                }
                let encoded = encode::encode_request(&request, "chat-test");
                let field = if audio_reference { "audio" } else { "content" };
                assert_eq!(encoded["messages"][0][field], expected[field]);
            }
            for (original, changed) in native.output.iter().zip(&changed.output) {
                if let (
                    Node::ProviderItem { body: original, .. },
                    Node::ProviderItem { body: changed, .. },
                ) = (original, changed)
                {
                    assert_eq!(original, changed);
                }
            }
        }
    }
}

#[test]
fn chat_configuration_update_identity_preserves_native_shape_both_stream_modes() {
    for stream in [false, true] {
        for native_type in [false, true] {
            let mut message =
                json!({"role":"system","id":"old-id","configuration_update":{"mode":"kept"}});
            if native_type {
                message["type"] = json!("native-config");
            }
            let mut request = decode::decode_request(
                &json!({"model":"chat-test","stream":stream,"messages":[message.clone()]}),
            )
            .unwrap();
            assert_eq!(
                encode::encode_request(&request, "chat-test")["messages"][0],
                message
            );
            for deleted in [false, true] {
                let Node::ProviderItem {
                    id,
                    item_type,
                    body,
                    ..
                } = &mut request.input[0]
                else {
                    panic!("expected configuration provider item");
                };
                assert_eq!(*body, message);
                *id = (!deleted).then(|| "new-id".to_string());
                *item_type = if deleted { "" } else { "new-config" }.to_string();
                let encoded = encode::encode_request(&request, "chat-test");
                let result = &encoded["messages"][0];
                assert_eq!(
                    result["configuration_update"],
                    message["configuration_update"]
                );
                if deleted {
                    assert!(result.get("id").is_none());
                } else {
                    assert_eq!(result["id"], "new-id");
                }
                if native_type && !deleted {
                    assert_eq!(result["type"], "new-config");
                } else {
                    assert!(result.get("type").is_none());
                }
            }
        }
    }
}

#[tokio::test]
async fn chat_assistant_file_response_returns_explicit_error() {
    assert_typed_media_feature(json!({"type":"file","file":{"file_id":"file_1"}}), "file").await;
    assert_typed_media_feature(json!({"type":"file","file":{"file_data":"data:application/pdf;base64,cGRm","filename":"document.pdf"}}), "file").await;
}

#[tokio::test]
async fn chat_assistant_audio_content_returns_explicit_error() {
    for format in ["wav", "mp3"] {
        assert_typed_media_feature(
            json!({"type":"input_audio","input_audio":{"data":"YQ==","format":format}}),
            "audio",
        )
        .await;
    }
}

#[test]
fn chat_assistant_history_annotations_bidirectional_both_stream_modes() {
    let annotations = json!([{"type":"url_citation","url_citation":{"url":"https://example.com","title":"Example","start_index":0,"end_index":1}}]);
    for stream in [false, true] {
        let source = json!({"model":"chat-test","stream":stream,"messages":[{"role":"assistant","content":"A","annotations":annotations},{"role":"user","content":"Explain"}]});
        let mut request = decode::decode_request(&source).unwrap();
        assert!(
            matches!(&request.input[0], Node::Text {citations,extra_body,..} if citations.len()==1 && !extra_body.contains_key("annotations"))
        );
        let encoded = encode::encode_request(&request, "chat-test");
        assert_eq!(encoded["messages"], source["messages"]);
        let decoded = decode::decode_request(&encoded).unwrap();
        assert_eq!(
            serde_json::to_value(&decoded.input).unwrap(),
            serde_json::to_value(&request.input).unwrap()
        );
        let response = decode::decode_response(&response(
            json!({"role":"assistant","content":"A","annotations":annotations}),
            "stop",
        ))
        .unwrap();
        request.input = response.output;
        assert_eq!(
            encode::encode_request(&request, "chat-test")["messages"][0]["annotations"],
            annotations
        );
        if let Node::Text {
            logprobs: _,
            citations,
            ..
        } = &mut request.input[0]
        {
            citations.clear();
        }
        assert!(
            encode::encode_request(&request, "chat-test")["messages"][0]
                .get("annotations")
                .is_none()
        );
    }
}

#[tokio::test]
async fn chat_bounded_text_frames_emit_citations_once() {
    let content = "x".repeat(4000);
    let citation = json!({"type":"url_citation","url_citation":{"url":"https://example.com","title":"Example","start_index":0,"end_index":1}});
    let message = json!({"role":"assistant","content":content,"annotations":[citation]});
    let canonical = decode::decode_response(&response(message.clone(), "stop")).unwrap();
    let mut events = decode_stream(vec![
        json!({"choices":[{"delta":message,"finish_reason":"stop"}]}),
    ])
    .await;
    events.retain(|event| !matches!(event, UrpStreamEvent::NodeDelta {delta:super::NodeDelta::Text {content,citations,..},..} if content.is_empty() && !citations.is_empty()));
    for event in &mut events {
        if let UrpStreamEvent::NodeDelta {
            delta:
                super::NodeDelta::Text {
                    logprobs: _,
                    content,
                    citations,
                    ..
                },
            ..
        } = event
            && !content.is_empty()
        {
            *citations = vec![crate::urp::Citation::decode(
                citation.clone(),
                crate::urp::ProviderProtocol::ChatCompletion,
            )];
        }
    }
    for wire in [
        synthetic_stream_with_limit(&canonical, Some(600)).await,
        encode_stream_with_limit(events, Some(600)).await,
    ] {
        let text_chunks = wire
            .iter()
            .filter(|frame| {
                frame["choices"][0]["delta"]["content"]
                    .as_str()
                    .is_some_and(|value| !value.is_empty())
            })
            .count();
        assert!(text_chunks > 1);
        let annotation_count: usize = wire
            .iter()
            .filter_map(|frame| frame["choices"][0]["delta"]["annotations"].as_array())
            .map(Vec::len)
            .sum();
        assert_eq!(annotation_count, 1);
        for frame in &wire {
            if frame["choices"][0]["delta"].get("annotations").is_some() {
                assert!(frame["choices"][0]["delta"].get("content").is_none());
            }
        }
        let decoded = terminal(&decode_stream(wire).await);
        assert_message(&decoded, &message);
    }
}

#[test]
fn chat_request_media_metadata_and_role_constraints() {
    for stream in [false, true] {
        let mut request = decode::decode_request(&json!({"model":"chat-test","stream":stream,"messages":[{"role":"user","content":[
            {"type":"image_url","image_url":{"url":"data:image/png;base64,YQ==","detail":"high"}},
            {"type":"file","file":{"file_data":"data:application/pdf;base64,JVBERi0x","filename":"original.pdf"}}
        ]}]})).unwrap();
        assert!(
            matches!(&request.input[0], Node::Image { source: super::ImageSource::Base64 { media_type, data }, metadata, .. }
            if media_type == "image/png" && data == "YQ==" && metadata.detail.as_deref() == Some("high"))
        );
        let Node::File {
            metadata,
            extra_body,
            ..
        } = &mut request.input[1]
        else {
            panic!("file");
        };
        assert_eq!(metadata.filename.as_deref(), Some("original.pdf"));
        metadata.filename = Some("changed.pdf".into());
        metadata.detail = Some("low".into());
        extra_body.insert("filename".into(), json!("stale.pdf"));
        extra_body.insert("detail".into(), json!("high"));
        extra_body.insert(
            "file".into(),
            json!({"filename":"stale.pdf","detail":"high","file_id":"stale"}),
        );
        let wire = encode::encode_request_checked(&request, "chat-test").unwrap();
        let parts = wire["messages"][0]["content"].as_array().unwrap();
        assert_eq!(parts[0]["image_url"]["detail"], "high");
        assert_eq!(parts[1]["file"]["filename"], "changed.pdf");
        assert!(parts[1].get("detail").is_none());
        assert!(parts[1]["file"].get("detail").is_none());
        assert!(parts[1]["file"].get("file_id").is_none());
        let Node::File { metadata, .. } = &mut request.input[1] else {
            unreachable!()
        };
        metadata.filename = None;
        let wire = encode::encode_request_checked(&request, "chat-test").unwrap();
        assert!(
            wire["messages"][0]["content"][1]["file"]
                .get("filename")
                .is_none()
        );
        for role in [
            super::OrdinaryRole::Assistant,
            super::OrdinaryRole::System,
            super::OrdinaryRole::Developer,
        ] {
            let mut invalid = request.clone();
            for node in &mut invalid.input {
                match node {
                    Node::Image { role: target, .. } | Node::File { role: target, .. } => {
                        *target = role
                    }
                    _ => {}
                }
            }
            assert!(encode::encode_request_checked(&invalid, "chat-test").is_err());
        }
        let native = super::decode::openai_responses::decode_request(&json!({"model":"test","stream":stream,"input":[{
            "type":"function_call_output","call_id":"call_1","output":[{"type":"input_text","text":"keep"},{"type":"input_image","image_url":"https://example.com/image.png"}]
        }]})).unwrap();
        assert!(encode::encode_request_checked(&native, "chat-test").is_err());
    }
}

#[tokio::test]
async fn chat_audio_context_and_history_reference_are_legal() {
    let native = json!({"id":"audio_1","data":"YQ==","transcript":"spoken","expires_at":100});
    let events = decode_stream_with_audio_format(
        vec![json!({"choices":[{"delta":{"audio":native},"finish_reason":"stop"}]})],
        Some("wav"),
    )
    .await;
    let decoded = terminal(&events);
    assert!(
        matches!(&decoded.output[0], Node::Audio { source: super::AudioSource::Base64 { media_type, .. }, .. } if media_type == "audio/wav")
    );
    let reencoded = encode_stream(events).await;
    assert!(
        reencoded
            .iter()
            .any(|frame| frame["choices"][0]["delta"]["audio"]["data"] == "YQ==")
    );
    for stream in [false, true] {
        let mut request =
            decode::decode_request(&json!({"model":"chat-test","stream":stream,"messages":[]}))
                .unwrap();
        request.input = decoded.output.clone();
        let wire = encode::encode_request_checked(&request, "chat-test").unwrap();
        assert_eq!(wire["messages"][0]["audio"], json!({"id":"audio_1"}));
        if let Node::Audio { metadata, .. } = &mut request.input[0] {
            metadata.reference_id = None;
        }
        assert!(encode::encode_request_checked(&request, "chat-test").is_err());
        for role in ["assistant", "system", "developer"] {
            let invalid = decode::decode_request(&json!({"model":"chat-test","stream":stream,"messages":[{"role":role,"content":[{"type":"input_audio","input_audio":{"data":"YQ==","format":"wav"}}]}]})).unwrap();
            assert!(encode::encode_request_checked(&invalid, "chat-test").is_err());
        }
    }
}

#[test]
fn chat_compatible_tool_media_preserves_blocks_and_following_user() {
    use super::{ToolCallType, ToolResultContent};
    let image = json!({"type":"image_url","image_url":{"url":"https://example.com/tool.png","detail":"high"}});
    let file = json!({"type":"file","file":{"file_data":"data:application/pdf;base64,JVBERi0xLjQK","filename":"result.pdf"}});
    for stream in [false, true] {
        for mode in ["function", "tool", "custom"] {
            for content in [
                image.clone(),
                file.clone(),
                json!([{"type":"input_text","text":"before"},image,file,"after"]),
            ] {
                let assistant = if mode == "function" {
                    json!({"role":"assistant","function_call":{"name":"fetch","arguments":"{}"}})
                } else if mode == "custom" {
                    json!({"role":"assistant","tool_calls":[{"id":"call_1","type":"custom","custom":{"name":"fetch","input":"raw"}}]})
                } else {
                    json!({"role":"assistant","tool_calls":[{"id":"call_1","type":"function","function":{"name":"fetch","arguments":"{}"}}]})
                };
                let request = json!({"model":"chat-test","stream":stream,"messages":[assistant,
                    {"role":if mode == "function" {"function"} else {"tool"},"name":"fetch","tool_call_id":"call_1","content":content},
                    {"role":"user","content":[{"type":"image_url","image_url":{"url":"https://example.com/user.png"}}]}]});
                let mut canonical = decode::decode_request(&request).unwrap();
                let (tool_type, call_id, parts) = canonical
                    .input
                    .iter()
                    .find_map(|node| match node {
                        Node::ToolResult {
                            tool_type,
                            call_id,
                            content,
                            ..
                        } => Some((*tool_type, call_id.clone(), content)),
                        _ => None,
                    })
                    .unwrap();
                assert_eq!(
                    tool_type,
                    if mode == "custom" {
                        ToolCallType::Custom
                    } else {
                        ToolCallType::Function
                    }
                );
                assert_eq!(parts.len(), if content.is_array() { 4 } else { 1 });
                assert!(parts.iter().any(|part| matches!(
                    part,
                    ToolResultContent::Image { .. } | ToolResultContent::File { .. }
                )));
                for part in parts {
                    if let ToolResultContent::File {
                        metadata,
                        source,
                        extra_body,
                    } = part
                    {
                        assert_eq!(metadata.filename.as_deref(), Some("result.pdf"));
                        assert!(
                            matches!(source, super::FileSource::Base64 {media_type,data} if media_type=="application/pdf" && data=="JVBERi0xLjQK")
                        );
                        assert!(
                            !extra_body.contains_key("filename")
                                && !extra_body.contains_key("file")
                        );
                    }
                }
                let encoded = super::encode::openai_responses::encode_request_checked(
                    &canonical,
                    "chat-test",
                )
                .unwrap();
                let items = encoded["input"].as_array().unwrap();
                let result = items
                    .iter()
                    .find(|item| item["call_id"] == call_id && item.get("output").is_some())
                    .unwrap();
                assert_eq!(
                    result["type"],
                    if mode == "custom" {
                        "custom_tool_call_output"
                    } else {
                        "function_call_output"
                    }
                );
                assert_eq!(result["output"].as_array().unwrap().len(), parts.len());
                if content.is_array() {
                    assert_eq!(
                        result["output"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|p| p["type"].as_str().unwrap())
                            .collect::<Vec<_>>(),
                        vec!["input_text", "input_image", "input_file", "input_text"]
                    );
                }
                assert_eq!(items.last().unwrap()["role"], "user");
                assert_eq!(
                    items.last().unwrap()["content"][0]["image_url"],
                    "https://example.com/user.png"
                );
                let roundtrip = super::decode::openai_responses::decode_request(&encoded).unwrap();
                assert!(roundtrip.input.iter().any(|node| matches!(node,Node::ToolResult {content,..} if content.iter().any(|p|matches!(p,ToolResultContent::Image {..}|ToolResultContent::File {..})))));
                assert!(encode::encode_request_checked(&canonical, "chat-test").is_err());
                for node in &mut canonical.input {
                    if let Node::ToolResult { content, .. } = node {
                        for part in content {
                            if let ToolResultContent::File { metadata, .. } = part {
                                metadata.filename = None;
                            }
                        }
                    }
                }
                let changed = super::encode::openai_responses::encode_request_checked(
                    &canonical,
                    "chat-test",
                )
                .unwrap();
                assert!(
                    changed["input"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter_map(|item| item["output"].as_array())
                        .flatten()
                        .all(|part| part.get("filename").is_none())
                );
            }
        }
    }
}

#[test]
fn chat_tool_result_arbitrary_json_and_malformed_media_are_distinct() {
    for stream in [false, true] {
        for value in [
            json!({"answer":42,"nested":{"type":"input_audio"}}),
            json!(["",{"type":"vendor","payload":{"type":"audio"}}]),
            Value::Null,
        ] {
            let canonical=decode::decode_request(&json!({"model":"chat-test","stream":stream,"messages":[{"role":"tool","tool_call_id":"call_1","content":value}]})).unwrap();
            let Node::ToolResult { content, .. } = &canonical.input[0] else {
                panic!()
            };
            if value.is_null() {
                assert!(content.is_empty());
            } else {
                assert!(
                    content
                        .iter()
                        .all(|part| matches!(part, super::ToolResultContent::Text { .. }))
                );
            }
        }
        for value in [
            json!({"type":"image_url","image_url":{}}),
            json!({"type":"file","file":{}}),
        ] {
            assert!(decode::decode_request(&json!({"model":"chat-test","stream":stream,"messages":[{"role":"tool","tool_call_id":"call_1","content":value}]})).is_err());
        }
    }
}

#[tokio::test]
async fn chat_compatible_single_content_object_and_input_text_are_typed() {
    for block in [
        json!({"type":"input_text","text":"known"}),
        json!({"type":"image_url","image_url":{"url":"https://example.com/a.png"}}),
    ] {
        for stream in [false, true] {
            let request=decode::decode_request(&json!({"model":"chat-test","stream":stream,"messages":[{"role":"user","content":block}]})).unwrap();
            assert_eq!(request.input.len(), 1);
            assert!(matches!(
                request.input[0],
                Node::Text { .. } | Node::Image { .. }
            ));
        }
        let native = decode::decode_response(&response(
            json!({"role":"assistant","content":block}),
            "stop",
        ))
        .unwrap();
        let streamed = terminal(
            &decode_stream(vec![
                json!({"choices":[{"delta":{"content":block},"finish_reason":"stop"}]}),
            ])
            .await,
        );
        assert_eq!(native.output.len(), 1);
        assert_eq!(streamed.output.len(), 1);
        assert_eq!(
            std::mem::discriminant(&native.output[0]),
            std::mem::discriminant(&streamed.output[0])
        );
    }
}

#[test]
fn chat_compatible_tool_audio_preserves_raw_source_as_typed_file() {
    for stream in [false, true] {
        for role in ["tool", "function"] {
            let request=decode::decode_request(&json!({"model":"chat-test","stream":stream,"messages":[{"role":role,"name":"audio","tool_call_id":"call_audio","content":{"type":"output_audio","data":"data:audio/wav;base64,YQ=="}}]})).unwrap();
            assert!(
                matches!(&request.input[0],Node::ToolResult {content,..} if matches!(&content[0],super::ToolResultContent::File {source:super::FileSource::Base64 {media_type,data},..} if media_type=="audio/wav" && data=="YQ=="))
            );
        }
    }
}

fn reasoning_fields(
    response: &UrpResponse,
) -> Vec<(Option<String>, Option<String>, Option<Value>)> {
    response
        .output
        .iter()
        .filter_map(|node| match node {
            Node::Reasoning {
                content,
                summary,
                encrypted,
                ..
            } => Some((content.clone(), summary.clone(), encrypted.clone())),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn chat_raw_cot_summary_and_cipher_remain_independent_all_response_modes() {
    let cipher = "eyJzZWNyZXQiOiJtdXN0LW5vdC1iZWNvbWUtdGV4dCJ9";
    for (message, expected) in [
        (
            json!({"role":"assistant","content":"answer","reasoning_content":"raw steps"}),
            vec![(Some("raw steps".into()), None, None)],
        ),
        (
            json!({"role":"assistant","content":"answer","reasoning_details":[{"type":"reasoning.summary","summary":"brief","index":0},{"type":"reasoning.encrypted","data":cipher,"index":1}]}),
            vec![
                (None, Some("brief".into()), None),
                (None, None, Some(json!(cipher))),
            ],
        ),
        (
            json!({"role":"assistant","content":"answer","reasoning_details":[{"type":"reasoning.text","text":"raw","index":0},{"type":"reasoning.summary","summary":"brief","index":1},{"type":"reasoning.encrypted","data":cipher,"index":2}]}),
            vec![
                (Some("raw".into()), None, None),
                (None, Some("brief".into()), None),
                (None, None, Some(json!(cipher))),
            ],
        ),
    ] {
        let native = decode::decode_response(&response(message.clone(), "stop")).unwrap();
        let streamed = decode_stream(vec![
            json!({"choices":[{"delta":message,"finish_reason":"stop"}]}),
        ])
        .await;
        for actual in [
            native.clone(),
            decode::decode_response(&encode::encode_response(&native, "chat-test")).unwrap(),
            terminal(&streamed),
            terminal(&decode_stream(encode_stream(streamed).await).await),
            terminal(&decode_stream(synthetic_stream(&native).await).await),
        ] {
            assert_eq!(reasoning_fields(&actual), expected);
            assert!(
                !serde_json::to_string(&actual.output)
                    .unwrap()
                    .contains("must-not-become-text")
            );
        }
    }
}

#[test]
fn chat_reasoning_history_replay_mutation_and_deletion_both_stream_modes() {
    for stream in [false, true] {
        for message in [
            json!({"role":"assistant","content":"answer","reasoning_content":"raw"}),
            json!({"role":"assistant","content":"answer","reasoning_details":[{"type":"reasoning.text","text":"raw","index":0,"id":"r0"},{"type":"reasoning.summary","summary":"brief","index":1},{"type":"reasoning.encrypted","data":"cipher","index":2}]}),
        ] {
            let source = json!({"model":"chat-test","stream":stream,"messages":[message]});
            let mut request = decode::decode_request(&source).unwrap();
            assert_eq!(
                encode::encode_request(&request, "chat-test")["messages"],
                source["messages"]
            );
            for node in &mut request.input {
                if let Node::Reasoning {
                    content,
                    summary,
                    encrypted,
                    ..
                } = node
                {
                    if content.is_some() {
                        *content = Some("new raw".into());
                    }
                    if summary.is_some() {
                        *summary = Some("new brief".into());
                    }
                    if encrypted.is_some() {
                        *encrypted = Some(json!("new cipher"));
                    }
                }
            }
            let encoded = encode::encode_request(&request, "chat-test");
            let again = decode::decode_request(&encoded).unwrap();
            assert_eq!(
                serde_json::to_value(&again.input).unwrap(),
                serde_json::to_value(&request.input).unwrap()
            );
            for node in &mut request.input {
                if let Node::Reasoning {
                    content,
                    summary,
                    encrypted,
                    ..
                } = node
                {
                    *content = None;
                    *summary = None;
                    *encrypted = None;
                }
            }
            let encoded =
                serde_json::to_string(&encode::encode_request(&request, "chat-test")).unwrap();
            for deleted in ["new raw", "new brief", "new cipher"] {
                assert!(!encoded.contains(deleted));
            }
        }
    }
}

#[tokio::test]
async fn chat_fragmented_raw_and_opaque_do_not_merge_with_summary() {
    let events = decode_stream(vec![
        json!({"choices":[{"delta":{"reasoning_content":"raw ","reasoning_opaque":"cipher-"}}]}),
        json!({"choices":[{"delta":{"reasoning_content":"steps","reasoning_opaque":"tail"}}]}),
        json!({"choices":[{"delta":{"content":"answer"},"finish_reason":"stop"}]}),
    ])
    .await;
    for actual in [
        terminal(&events),
        terminal(&decode_stream(encode_stream(events).await).await),
    ] {
        let fields = reasoning_fields(&actual);
        assert_eq!(
            fields
                .iter()
                .filter_map(|f| f.0.as_deref())
                .collect::<String>(),
            "raw steps"
        );
        assert!(fields.iter().all(|f| f.1.is_none()));
        assert_eq!(
            fields
                .iter()
                .filter_map(|f| f.2.as_ref().and_then(Value::as_str))
                .collect::<String>(),
            "cipher-tail"
        );
    }
}

fn chat_scores(text: &str) -> Value {
    json!([{"token":text,"bytes":text.as_bytes(),"logprob":-0.25,"top_logprobs":[{"token":"alternative","bytes":null,"logprob":-1.5}]}])
}

#[test]
fn chat_request_logprobs_and_formats_have_typed_ownership_both_stream_modes() {
    for stream in [false, true] {
        for format in [
            json!({"type":"text"}),
            json!({"type":"json_object"}),
            json!({"type":"json_schema","json_schema":{"name":"answer","schema":{"type":"object","additionalProperties":false},"strict":true}}),
        ] {
            let mut request=decode::decode_request(&json!({"model":"chat-test","stream":stream,"messages":[{"role":"user","content":"JSON"}],"logprobs":true,"top_logprobs":2,"response_format":format,"stream_options":{"include_usage":true,"include_obfuscation":false}})).unwrap();
            assert_eq!(
                request.logprobs,
                Some(super::LogprobConfig {
                    enabled: true,
                    top_k: Some(2)
                })
            );
            let encoded = encode::encode_request(&request, "chat-test");
            assert_eq!(encoded["response_format"], format);
            assert_eq!(encoded["logprobs"], true);
            assert_eq!(encoded["top_logprobs"], 2);
            request.logprobs = None;
            let encoded = encode::encode_request(&request, "chat-test");
            assert!(encoded.get("logprobs").is_none());
            assert!(encoded.get("top_logprobs").is_none());
        }
    }
}

#[tokio::test]
async fn chat_logprobs_text_and_refusal_roundtrip_and_mutation_all_modes() {
    for (surface, text) in [("content", "hello"), ("refusal", "cannot")] {
        let mut message = json!({"role":"assistant","content":null});
        message[surface] = json!(text);
        let mut native = response(message.clone(), "stop");
        native["choices"][0]["logprobs"] = json!({surface:chat_scores(text)});
        let decoded = decode::decode_response(&native).unwrap();
        let events=decode_stream(vec![json!({"choices":[{"delta":message,"logprobs":{surface:chat_scores(text)},"finish_reason":"stop"}]})]).await;
        for mut actual in [
            decoded.clone(),
            terminal(&events),
            terminal(&decode_stream(encode_stream(events).await).await),
            terminal(&decode_stream(synthetic_stream(&decoded).await).await),
        ] {
            let encoded = encode::encode_response(&actual, "chat-test");
            assert_eq!(
                encoded["choices"][0]["logprobs"][surface],
                chat_scores(text)
            );
            for node in &mut actual.output {
                match node {
                    Node::Text { content, .. } | Node::Refusal { content, .. } => {
                        *content = "changed".into()
                    }
                    _ => {}
                }
            }
            let encoded = encode::encode_response(&actual, "chat-test");
            assert!(
                encoded["choices"][0].get("logprobs").is_none(),
                "stale scores {encoded}"
            );
        }
    }
}

#[tokio::test]
async fn chat_finish_reason_matrix_preserves_partial_output_and_usage_all_modes() {
    for (reason, expected) in [
        ("stop", super::FinishReason::Stop),
        ("length", super::FinishReason::Length),
        ("content_filter", super::FinishReason::ContentFilter),
        ("tool_calls", super::FinishReason::ToolCalls),
        ("future_stop", super::FinishReason::Other),
    ] {
        let mut native = response(json!({"role":"assistant","content":"partial"}), reason);
        native["usage"] = json!({"prompt_tokens":20,"completion_tokens":10,"total_tokens":30,"prompt_tokens_details":{"cached_tokens":5,"audio_tokens":2,"cache_creation_tokens":0,"cache_write_tokens":0,"tool_prompt_tokens":0},"completion_tokens_details":{"reasoning_tokens":6,"audio_tokens":1,"accepted_prediction_tokens":2,"rejected_prediction_tokens":1}});
        let decoded = decode::decode_response(&native).unwrap();
        let events = decode_stream(vec![
            json!({"choices":[{"delta":{"content":"partial"},"finish_reason":reason}]}),
            json!({"choices":[],"usage":native["usage"]}),
        ])
        .await;
        for actual in [
            decoded.clone(),
            terminal(&events),
            terminal(&decode_stream(encode_stream(events).await).await),
            terminal(&decode_stream(synthetic_stream(&decoded).await).await),
        ] {
            assert_eq!(actual.finish_reason, Some(expected));
            let encoded = encode::encode_response(&actual, "chat-test");
            assert_eq!(encoded["choices"][0]["message"]["content"], "partial");
            assert_eq!(encoded["choices"][0]["finish_reason"], reason);
            assert_eq!(encoded["usage"], native["usage"]);
        }
    }
}

#[test]
fn chat_malformed_requests_and_nonstream_errors_are_rejected() {
    for stream in [false, true] {
        for n in [
            json!(0),
            json!(2),
            json!(-1),
            json!(1.5),
            json!("1"),
            json!(null),
        ] {
            assert!(
                decode::decode_request(
                    &json!({"model":"chat-test","stream":stream,"messages":[],"n":n})
                )
                .is_err()
            );
        }
    }
    for invalid in [
        json!(null),
        json!([]),
        json!({}),
        json!({"model":"m","messages":"wrong"}),
    ] {
        assert!(decode::decode_request(&invalid).is_err());
    }
    for invalid in [
        json!(null),
        json!({}),
        json!({"choices":[]}),
        json!({"choices":[{"message":null}]}),
        json!({"error":{"message":"quota","code":"insufficient_quota"}}),
        json!({"choices":[{"error":{"message":"failed"}}]}),
        response(json!({"content":"partial"}), "error"),
    ] {
        assert!(decode::decode_response(&invalid).is_err(), "{invalid}");
    }
}

#[tokio::test]
async fn chat_interleaved_function_fragments_keep_call_identity_and_arguments() {
    let events=decode_stream(vec![
        json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c0","type":"function","function":{"name":"weather","arguments":"{\"city\":\""}},{"index":1,"id":"c1","type":"function","function":{"name":"time","arguments":"{"}}]}}]}),
        json!({"choices":[{"delta":{"tool_calls":[{"index":1,"function":{"arguments":"}"}},{"index":0,"function":{"arguments":"北京\"}"}}]},"finish_reason":"tool_calls"}]}),
    ]).await;
    for actual in [
        terminal(&events),
        terminal(&decode_stream(encode_stream(events).await).await),
    ] {
        let wire = encode::encode_response(&actual, "chat-test");
        assert_eq!(
            wire["choices"][0]["message"]["tool_calls"],
            json!([{"id":"c0","type":"function","function":{"name":"weather","arguments":"{\"city\":\"北京\"}"}},{"id":"c1","type":"function","function":{"name":"time","arguments":"{}"}}])
        );
    }
}

#[tokio::test]
async fn chat_sse_malformed_json_eof_and_post_start_errors_never_succeed() {
    for wire in [
        "data: {broken\n\ndata: [DONE]\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
        "data: [DONE]\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\ndata: {\"error\":{\"code\":\"overloaded\",\"message\":\"retry\"}}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"error\"}]}\n\n",
    ] {
        let events = decode_chat_wire(wire.into(), None).await;
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, UrpStreamEvent::Error { .. }))
                .count(),
            1,
            "{events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, UrpStreamEvent::ResponseDone { .. })),
            "{events:?}"
        );
        let downstream = encode_stream(events).await;
        assert!(
            downstream.iter().any(|v| v.get("error").is_some()),
            "{downstream:?}"
        );
        assert!(
            !downstream
                .iter()
                .any(|v| v["choices"][0]["finish_reason"] == "stop")
        );
    }
}

#[tokio::test]
async fn chat_sse_comments_crlf_and_usage_after_finish_roundtrip() {
    let wire = concat!(
        ": keepalive\r\n\r\n",
        "data: {\"id\":\"c\",\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"hi\"},\"finish_reason\":null}]}\r\n\r\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\r\n\r\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":2,\"total_tokens\":11}}\r\n\r\n",
        "data: [DONE]\r\n\r\n"
    );
    let events = decode_chat_wire(wire.into(), None).await;
    assert_eq!(terminal(&events).usage.unwrap().total_tokens(), 11);
    let output = encode_stream(events).await;
    let usage = output
        .iter()
        .position(|v| v.get("usage").is_some_and(|u| !u.is_null()))
        .unwrap();
    assert_eq!(output[usage]["choices"], json!([]));
    assert_eq!(output[usage - 1]["choices"][0]["finish_reason"], "stop");
    assert!(output[usage - 1].get("usage").is_none_or(Value::is_null));
    assert_eq!(
        output
            .iter()
            .filter(|v| v.get("usage").is_some_and(|u| !u.is_null()))
            .count(),
        1
    );
}

#[tokio::test]
async fn chat_reasoning_typed_deletion_does_not_replay_native_payload_all_modes() {
    let native = response(
        json!({"role":"assistant","content":"answer","reasoning_details":[{"type":"reasoning.text","text":"raw-delete","signature":"cipher-delete","id":"r0","format":"native","index":0,"vendor":"kept"},{"type":"reasoning.summary","summary":"summary-delete","index":1}]}),
        "stop",
    );
    let mut decoded = decode::decode_response(&native).unwrap();
    for node in &mut decoded.output {
        if let Node::Reasoning {
            content,
            summary,
            encrypted,
            ..
        } = node
        {
            *content = None;
            *summary = None;
            *encrypted = None;
        }
    }
    for output in [
        encode::encode_response(&decoded, "chat-test"),
        json!(synthetic_stream(&decoded).await),
    ] {
        let wire = serde_json::to_string(&output).unwrap();
        for deleted in ["raw-delete", "cipher-delete", "summary-delete"] {
            assert!(!wire.contains(deleted), "{wire}");
        }
    }
    let mut events = decode_stream(vec![
        json!({"choices":[{"delta":native["choices"][0]["message"],"finish_reason":"stop"}]}),
    ])
    .await;
    for event in &mut events {
        match event {
            UrpStreamEvent::NodeDelta {
                delta:
                    super::NodeDelta::Reasoning {
                        content,
                        summary,
                        encrypted,
                        ..
                    },
                ..
            } => {
                *content = None;
                *summary = None;
                *encrypted = None;
            }
            UrpStreamEvent::NodeDone {
                node:
                    Node::Reasoning {
                        content,
                        summary,
                        encrypted,
                        ..
                    },
                ..
            } => {
                *content = None;
                *summary = None;
                *encrypted = None;
            }
            UrpStreamEvent::ResponseDone { output, .. } => {
                for node in output {
                    if let Node::Reasoning {
                        content,
                        summary,
                        encrypted,
                        ..
                    } = node
                    {
                        *content = None;
                        *summary = None;
                        *encrypted = None;
                    }
                }
            }
            _ => {}
        }
    }
    let wire = serde_json::to_string(&encode_stream(events).await).unwrap();
    for deleted in ["raw-delete", "cipher-delete", "summary-delete"] {
        assert!(!wire.contains(deleted), "{wire}");
    }
}

#[tokio::test]
async fn chat_malformed_logprobs_are_omitted_without_losing_text() {
    for scores in [
        json!(null),
        json!("invalid"),
        json!([{"token":"hi","logprob":"invalid"}]),
        chat_scores("stale"),
    ] {
        let mut native = response(json!({"role":"assistant","content":"hi"}), "stop");
        native["choices"][0]["logprobs"] = json!({"content":scores});
        let decoded = decode::decode_response(&native).unwrap();
        let events=decode_stream(vec![json!({"choices":[{"delta":{"content":"hi"},"logprobs":{"content":scores},"finish_reason":"stop"}]})]).await;
        for actual in [
            decoded.clone(),
            terminal(&events),
            terminal(&decode_stream(encode_stream(events).await).await),
            terminal(&decode_stream(synthetic_stream(&decoded).await).await),
        ] {
            let wire = encode::encode_response(&actual, "chat-test");
            assert_eq!(wire["choices"][0]["message"]["content"], "hi");
            assert!(wire["choices"][0].get("logprobs").is_none(), "{wire}");
        }
    }
}

#[test]
fn chat_origin_metadata_and_reserved_keys_follow_protocol_boundary() {
    let source = json!({"model":"chat-test","messages":[{"role":"assistant","content":"answer","annotations":[{"type":"url_citation","url_citation":{"url":"https://example.com","title":"source","start_index":0,"end_index":3},"chat_only":"kept"}],"vendor":"kept","_monoize_injected":"drop"}],"_monoize_injected":"drop"});
    for stream in [false, true] {
        let mut source = source.clone();
        source["stream"] = json!(stream);
        let mut decoded = decode::decode_request(&source).unwrap();
        let same = encode::encode_request(&decoded, "chat-test");
        assert_eq!(same["messages"][0]["annotations"][0]["chat_only"], "kept");
        assert_eq!(same["messages"][0]["vendor"], "kept");
        assert!(
            !serde_json::to_string(&same)
                .unwrap()
                .contains("_monoize_injected")
        );
        super::strip_nested_extra_body(&mut decoded.input);
        let other = super::encode::openai_responses::encode_request(&decoded, "chat-test");
        let wire = serde_json::to_string(&other).unwrap();
        assert!(!wire.contains("chat_only"));
        assert!(!wire.contains("vendor"));
        assert!(wire.contains("https://example.com"));
    }
}

#[tokio::test]
async fn chat_reasoning_only_length_finish_is_valid_and_billed() {
    let native = response(
        json!({"role":"assistant","content":"","reasoning_content":"unfinished analysis"}),
        "length",
    );
    let decoded = decode::decode_response(&native).unwrap();
    let events=decode_stream(vec![json!({"choices":[{"delta":{"reasoning_content":"unfinished analysis"},"finish_reason":"length"}]}),json!({"choices":[],"usage":native["usage"]})]).await;
    for actual in [
        decoded.clone(),
        terminal(&events),
        terminal(&decode_stream(encode_stream(events).await).await),
        terminal(&decode_stream(synthetic_stream(&decoded).await).await),
    ] {
        assert_eq!(actual.finish_reason, Some(super::FinishReason::Length));
        assert_eq!(actual.usage.as_ref().unwrap().output_tokens, 7);
        assert_eq!(
            reasoning_fields(&actual),
            vec![(Some("unfinished analysis".into()), None, None)]
        );
        assert_eq!(
            encode::encode_response(&actual, "chat-test")["choices"][0]["message"]["content"],
            ""
        );
    }
}

#[test]
fn chat_documented_controls_and_tool_choices_both_stream_modes() {
    for stream in [false, true] {
        for choice in [
            json!("none"),
            json!("auto"),
            json!("required"),
            json!({"type":"function","function":{"name":"f"}}),
            json!({"type":"custom","custom":{"name":"patch"}}),
        ] {
            let source = json!({"model":"chat-test","stream":stream,"messages":[{"role":"system","content":"rules"},{"role":"developer","content":"developer"},{"role":"user","content":"question"}],"tools":[{"type":"function","function":{"name":"f","description":"function","parameters":{"type":"object"},"strict":false}},{"type":"custom","custom":{"name":"patch","format":{"type":"text"}}}],"tool_choice":choice,"temperature":0.4,"top_p":0.9,"max_completion_tokens":512,"stop":"END","parallel_tool_calls":true,"reasoning_effort":"high","presence_penalty":0.2,"frequency_penalty":0.1,"logit_bias":{"12":-1},"seed":42,"store":false,"metadata":{"trace":"test"},"service_tier":"default","safety_identifier":"safe","prompt_cache_key":"cache","prompt_cache_retention":"24h","prediction":{"type":"content","content":"predicted"}});
            let mut request = decode::decode_request(&source).unwrap();
            let output = encode::encode_request(&request, "chat-test");
            for key in [
                "messages",
                "tools",
                "tool_choice",
                "temperature",
                "top_p",
                "stop",
                "parallel_tool_calls",
                "reasoning_effort",
                "presence_penalty",
                "frequency_penalty",
                "logit_bias",
                "seed",
                "store",
                "metadata",
                "service_tier",
                "safety_identifier",
                "prompt_cache_key",
                "prompt_cache_retention",
                "prediction",
            ] {
                assert_eq!(output[key], source[key], "{key}");
            }
            assert_eq!(
                decode::decode_request(&output).unwrap().max_output_tokens,
                Some(512)
            );
            request.temperature = None;
            request.top_p = None;
            request.max_output_tokens = None;
            request.stop = None;
            request.reasoning = None;
            request.tool_choice = None;
            let output = encode::encode_request(&request, "chat-test");
            for key in [
                "temperature",
                "top_p",
                "max_tokens",
                "max_completion_tokens",
                "stop",
                "reasoning_effort",
                "reasoning",
                "thinking",
                "tool_choice",
            ] {
                assert!(output.get(key).is_none(), "{key}: {output}");
            }
        }
    }
}

#[tokio::test]
async fn chat_fragmented_logprobs_preserve_unicode_token_bytes_and_order() {
    let native = response(json!({"role":"assistant","content":"你a"}), "stop");
    let first = chat_scores("你");
    let second = chat_scores("a");
    let events=decode_stream(vec![json!({"choices":[{"delta":{"content":"你"},"logprobs":{"content":first}}]}),json!({"choices":[{"delta":{"content":"a"},"logprobs":{"content":second},"finish_reason":"stop"}]})]).await;
    let expected = json!([first[0], second[0]]);
    for actual in [
        terminal(&events),
        terminal(&decode_stream(encode_stream_with_limit(events, Some(320)).await).await),
    ] {
        let wire = encode::encode_response(&actual, "chat-test");
        assert_eq!(
            wire["choices"][0]["message"],
            native["choices"][0]["message"]
        );
        assert_eq!(wire["choices"][0]["logprobs"]["content"], expected);
    }
}

#[tokio::test]
async fn chat_native_raw_provenance_cannot_restore_raw_from_summary() {
    for stream in [false, true] {
        let mut request=decode::decode_request(&json!({"model":"chat-test","stream":stream,"messages":[{"role":"assistant","content":"answer","reasoning_content":"deleted raw"}]})).unwrap();
        for node in &mut request.input {
            if let Node::Reasoning {
                content,
                summary,
                encrypted,
                ..
            } = node
            {
                *content = None;
                *summary = Some("retained summary".into());
                *encrypted = Some(json!("retained cipher"));
            }
        }
        let encoded = encode::encode_request(&request, "chat-test");
        let message = &encoded["messages"][0];
        assert!(message.get("reasoning_content").is_none(), "{message}");
        assert_eq!(
            message["reasoning_details"],
            json!([{"type":"reasoning.summary","summary":"retained summary"},{"type":"reasoning.encrypted","data":"retained cipher"}])
        );
    }
    let mut response = decode::decode_response(&response(
        json!({"role":"assistant","content":"answer","reasoning_content":"deleted raw"}),
        "stop",
    ))
    .unwrap();
    for node in &mut response.output {
        if let Node::Reasoning {
            content,
            summary,
            encrypted,
            ..
        } = node
        {
            *content = None;
            *summary = Some("retained summary".into());
            *encrypted = Some(json!("retained cipher"));
        }
    }
    let encoded = encode::encode_response(&response, "chat-test");
    assert!(
        encoded["choices"][0]["message"]
            .get("reasoning_content")
            .is_none()
    );
    for actual in [
        decode::decode_response(&encoded).unwrap(),
        terminal(&decode_stream(synthetic_stream(&response).await).await),
    ] {
        let fields = reasoning_fields(&actual);
        assert!(fields.iter().all(|f| f.0.is_none()), "{fields:?}");
        assert_eq!(
            fields
                .iter()
                .filter_map(|f| f.1.as_deref())
                .collect::<String>(),
            "retained summary"
        );
        assert_eq!(
            fields
                .iter()
                .filter_map(|f| f.2.as_ref().and_then(Value::as_str))
                .collect::<String>(),
            "retained cipher"
        );
    }
}

#[tokio::test]
async fn chat_equal_raw_and_summary_bytes_are_not_semantic_duplicates() {
    for message in [
        json!({"role":"assistant","content":"answer","reasoning_content":"same","reasoning_details":[{"type":"reasoning.summary","summary":"same","index":0}]}),
        json!({"role":"assistant","content":"answer","reasoning":"same","reasoning_content":"same","reasoning_details":[{"type":"reasoning.summary","summary":"same","index":0}]}),
        json!({"role":"assistant","content":"answer","reasoning":"generic","reasoning_content":"raw"}),
        json!({"role":"assistant","content":"answer","reasoning":"same","reasoning_content":"same","reasoning_details":[{"type":"reasoning.text","text":"same","index":0}]}),
    ] {
        let expected_raw = if message["reasoning"] == "generic" {
            "genericraw"
        } else {
            "same"
        };
        let expected_summary = if message["reasoning_details"][0]["type"] == "reasoning.summary" {
            "same"
        } else {
            ""
        };
        let check = |nodes: &[Node]| {
            let raw = nodes
                .iter()
                .filter_map(|node| match node {
                    Node::Reasoning { content, .. } => content.as_deref(),
                    _ => None,
                })
                .collect::<String>();
            let summary = nodes
                .iter()
                .filter_map(|node| match node {
                    Node::Reasoning { summary, .. } => summary.as_deref(),
                    _ => None,
                })
                .collect::<String>();
            assert_eq!(raw, expected_raw, "{nodes:?}");
            assert_eq!(summary, expected_summary, "{nodes:?}");
        };
        for stream in [false, true] {
            let request = decode::decode_request(
                &json!({"model":"chat-test","stream":stream,"messages":[message.clone()]}),
            )
            .unwrap();
            check(&request.input);
            check(
                &decode::decode_request(&encode::encode_request(&request, "chat-test"))
                    .unwrap()
                    .input,
            );
        }
        let decoded = decode::decode_response(&response(message.clone(), "stop")).unwrap();
        let events = decode_stream(vec![
            json!({"choices":[{"delta":message,"finish_reason":"stop"}]}),
        ])
        .await;
        for actual in [
            decoded.clone(),
            decode::decode_response(&encode::encode_response(&decoded, "chat-test")).unwrap(),
            terminal(&events),
            terminal(&decode_stream(encode_stream(events).await).await),
            terminal(&decode_stream(synthetic_stream(&decoded).await).await),
        ] {
            check(&actual.output);
        }
    }
}

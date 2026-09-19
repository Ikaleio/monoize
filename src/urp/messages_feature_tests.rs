use super::{Node, ProviderProtocol, UrpResponse, UrpStreamEvent};
use crate::urp::{decode::anthropic as decode, encode::anthropic as encode};
use axum::response::{IntoResponse, Sse, sse::Event};
use serde_json::{Value, json};
use std::{collections::HashMap, convert::Infallible};
use tokio::sync::mpsc;

fn response(blocks: Vec<Value>) -> Value {
    json!({"id":"msg_feature", "type":"message", "role":"assistant", "model":"claude-test", "content":blocks,
        "stop_reason":"end_turn", "stop_sequence":null, "usage":{"input_tokens":11,"output_tokens":7}})
}

async fn decode_stream(frames: Vec<Value>) -> Vec<UrpStreamEvent> {
    decode_stream_with_custom_names(frames, &[]).await
}

async fn decode_stream_with_custom_names(
    frames: Vec<Value>,
    custom_names: &[&str],
) -> Vec<UrpStreamEvent> {
    let wire: String = frames
        .iter()
        .map(|frame| {
            format!(
                "event: {}\ndata: {}\n\n",
                frame["type"].as_str().unwrap(),
                frame
            )
        })
        .collect();
    decode_raw_stream(wire, custom_names).await
}

async fn decode_raw_stream(wire: String, custom_names: &[&str]) -> Vec<UrpStreamEvent> {
    let raw = axum::http::Response::builder()
        .header("content-type", "text/event-stream")
        .body(wire)
        .unwrap();
    let upstream = reqwest::Response::from(raw);
    let request = crate::handlers::UrpRequest {
        audio_output_format: None,
        model: "claude-test".into(),
        max_multiplier: None,
        server_tool_usage_classes: vec![],
        messages_custom_tool_names: custom_names.iter().map(|name| name.to_string()).collect(),
        affinity_explicit: None,
        affinity_prefix_hash: String::new(),
    };
    let (tx, mut rx) = mpsc::channel(1024);
    crate::urp::stream_decode::anthropic::stream_messages_to_urp_events(
        &request, upstream, tx, None, None, 1000,
    )
    .await
    .unwrap();
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    events
}

async fn sse_json(mut rx: mpsc::Receiver<Event>) -> Vec<Value> {
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(Ok::<Event, Infallible>(event));
    }
    let response = Sse::new(futures_util::stream::iter(events)).into_response();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec())
        .unwrap()
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

async fn encode_stream(events: Vec<UrpStreamEvent>) -> Vec<Value> {
    let (source_tx, source_rx) = mpsc::channel(1024);
    for event in events {
        source_tx.send(event).await.unwrap();
    }
    drop(source_tx);
    let (tx, rx) = mpsc::channel(1024);
    crate::urp::stream_encode::anthropic::encode_urp_stream_as_messages(
        source_rx,
        tx,
        "claude-test",
        None,
        false,
    )
    .await
    .unwrap();
    sse_json(rx).await
}

async fn synthetic_stream(response: &UrpResponse) -> Vec<Value> {
    let (tx, rx) = mpsc::channel(1024);
    crate::urp::stream_encode::anthropic::emit_synthetic_messages_stream(
        "claude-test",
        response,
        None,
        tx,
    )
    .await
    .unwrap();
    sse_json(rx).await
}

fn terminal(events: &[UrpStreamEvent]) -> UrpResponse {
    events
        .iter()
        .rev()
        .find_map(|event| {
            if let UrpStreamEvent::ResponseDone {
                outcome: _,
                finish_reason,
                usage,
                output,
                extra_body,
            } = event
            {
                Some(UrpResponse {
                    outcome: None,
                    id: "msg_feature".into(),
                    model: "claude-test".into(),
                    created_at: None,
                    output: output.clone(),
                    finish_reason: *finish_reason,
                    usage: usage.clone(),
                    extra_body: extra_body.clone(),
                })
            } else {
                None
            }
        })
        .expect("terminal canonical response")
}

fn frames(blocks: &[Value], stop: &str) -> Vec<Value> {
    let mut result = vec![
        json!({"type":"message_start","message":{"id":"msg_feature","type":"message","role":"assistant","model":"claude-test","content":[],"usage":{"input_tokens":11,"output_tokens":0}}}),
    ];
    for (index, block) in blocks.iter().enumerate() {
        let mut start = block.clone();
        let mut deltas = vec![];
        match block["type"].as_str().unwrap() {
            "text" => {
                start["text"] = json!("");
                start.as_object_mut().unwrap().remove("citations");
                deltas.push(json!({"type":"text_delta","text":block["text"]}));
                for citation in block
                    .get("citations")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    deltas.push(json!({"type":"citations_delta","citation":citation}));
                }
            }
            "thinking" => {
                start["thinking"] = json!("");
                start["signature"] = json!("");
                deltas.push(json!({"type":"thinking_delta","thinking":block["thinking"]}));
                if let Some(signature) = block.get("signature") {
                    deltas.push(json!({"type":"signature_delta","signature":signature}));
                }
            }
            "tool_use" | "server_tool_use" | "mcp_tool_use" => {
                start["input"] = json!({});
                let input = block["input"].to_string();
                let boundary = input
                    .char_indices()
                    .nth(input.chars().count() / 2)
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                for partial in [&input[..boundary], &input[boundary..]] {
                    deltas.push(json!({"type":"input_json_delta","partial_json":partial}));
                }
            }
            _ => {}
        }
        result.push(json!({"type":"content_block_start","index":index + 4,"content_block":start}));
        for delta in deltas {
            result.push(json!({"type":"content_block_delta","index":index + 4,"delta":delta}));
        }
        result.push(json!({"type":"content_block_stop","index":index + 4}));
    }
    result.push(json!({"type":"message_delta","delta":{"stop_reason":stop,"stop_sequence":null},"usage":{"output_tokens":7}}));
    result.push(json!({"type":"message_stop"}));
    result
}

async fn assert_block_roundtrip(blocks: Vec<Value>) {
    let original = response(blocks.clone());
    let urp = decode::decode_response(&original).unwrap();
    assert_eq!(
        encode::encode_response(&urp, "claude-test")["content"],
        json!(blocks),
        "nonstream protocol to URP to protocol"
    );
    let events = decode_stream(frames(&blocks, "end_turn")).await;
    assert_eq!(
        encode::encode_response(&terminal(&events), "claude-test")["content"],
        json!(blocks),
        "stream protocol to URP"
    );
    let roundtrip = decode_stream(encode_stream(events).await).await;
    assert_eq!(
        encode::encode_response(&terminal(&roundtrip), "claude-test")["content"],
        json!(blocks),
        "live URP to protocol to URP"
    );
    let synthetic = decode_stream(synthetic_stream(&urp).await).await;
    assert_eq!(
        encode::encode_response(&terminal(&synthetic), "claude-test")["content"],
        json!(blocks),
        "synthetic URP to protocol to URP"
    );
}

#[tokio::test]
async fn messages_client_function_and_computer_use_bidirectional() {
    assert_block_roundtrip(vec![
        json!({"type":"tool_use","id":"toolu_function","name":"weather","input":{"city":"深圳","nested":{"n":2}}}),
        json!({"type":"tool_use","id":"toolu_computer","name":"computer","input":{"action":"left_click","coordinate":[10,20]}}),
        json!({"type":"tool_use","id":"toolu_member","name":"screenshot","toolset_name":"computer","input":{}}),
        json!({"type":"tool_use","id":"toolu_browser","name":"navigate","toolset_name":"browser","input":{"url":"https://example.com"}}),
    ]).await;
}

#[tokio::test]
async fn messages_thinking_redaction_signature_bidirectional() {
    assert_block_roundtrip(vec![
        json!({"type":"thinking","thinking":"Consider both cases.","signature":"opaque-signature"}),
        json!({"type":"redacted_thinking","data":"opaque-redaction"}),
    ])
    .await;
}

#[tokio::test]
async fn messages_citations_cache_and_phase_bidirectional() {
    assert_block_roundtrip(vec![json!({"type":"text","text":"A cited answer.","phase":"final_answer","cache_control":{"type":"ephemeral","ttl":"1h"},"citations":[
        {"type":"char_location","cited_text":"A","document_index":0,"document_title":"Doc","start_char_index":0,"end_char_index":1},
        {"type":"web_search_result_location","url":"https://example.com","title":"Example","encrypted_index":"cipher","cited_text":"answer"}
    ]})]).await;
}

#[test]
fn messages_documents_images_files_request_bidirectional() {
    let blocks = vec![
        json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"aGVsbG8="}}),
        json!({"type":"image","source":{"type":"url","url":"https://example.com/image.png"}}),
        json!({"type":"image","source":{"type":"file","file_id":"file_image"}}),
        json!({"type":"document","title":"Text","context":"Context","citations":{"enabled":true},"source":{"type":"text","media_type":"text/plain","data":"Document text"}}),
        json!({"type":"document","source":{"type":"base64","media_type":"application/pdf","data":"JVBERi0xLjcK"}}),
        json!({"type":"document","source":{"type":"file","file_id":"file_doc"}}),
        json!({"type":"document","source":{"type":"content","content":[{"type":"text","text":"Nested text"}]}}),
    ];
    for stream in [false, true] {
        let native = json!({"model":"claude-test","max_tokens":128,"stream":stream,
            "messages":[{"role":"user","content":blocks}]});
        let canonical = decode::decode_request(&native).unwrap();
        let encoded = encode::encode_request_checked(&canonical, "claude-test").unwrap();
        assert_eq!(encoded["messages"], native["messages"]);
        assert_eq!(encoded["stream"], stream);
        let restored = decode::decode_request(&encoded).unwrap();
        assert_eq!(
            serde_json::to_value(&restored.input).unwrap(),
            serde_json::to_value(&canonical.input).unwrap()
        );
    }
}

#[test]
fn messages_document_string_and_typed_metadata_request_bidirectional() {
    for stream in [false, true] {
        for nested in [false, true] {
            let document = json!({"type":"document","title":"Old title","context":"Old context",
                "citations":{"enabled":true},"cache_control":{"type":"ephemeral"},"future_document":7,
                "source":{"type":"content","content":"Custom text"}});
            let block = if nested {
                json!({"type":"tool_result","tool_use_id":"toolu_document","content":[document]})
            } else {
                document
            };
            let native = json!({"model":"claude-test","max_tokens":128,"stream":stream,
                "messages":[{"role":"user","content":[block]}]});
            let mut canonical = decode::decode_request(&native).unwrap();
            let (source, metadata, extra) = match &mut canonical.input[0] {
                Node::File {
                    source,
                    metadata,
                    extra_body,
                    ..
                } => (source, metadata, extra_body),
                Node::ToolResult { content, .. } => match &mut content[0] {
                    super::ToolResultContent::File {
                        source,
                        metadata,
                        extra_body,
                    } => (source, metadata, extra_body),
                    other => panic!("expected typed tool-result document: {other:?}"),
                },
                other => panic!("expected document: {other:?}"),
            };
            assert!(
                matches!(source, super::FileSource::Content { content } if content == &vec![json!({"type":"text","text":"Custom text"})])
            );
            assert_eq!(metadata.document_title.as_deref(), Some("Old title"));
            assert_eq!(metadata.document_context.as_deref(), Some("Old context"));
            assert_eq!(metadata.document_citations, Some(json!({"enabled":true})));
            for key in ["title", "context", "citations"] {
                assert!(!extra.contains_key(key));
            }
            metadata.document_title = Some("New title".into());
            metadata.document_context = None;
            metadata.document_citations = None;
            for (key, value) in [
                ("title", json!("stale")),
                ("context", json!("stale")),
                ("citations", json!({"enabled":true})),
            ] {
                extra.insert(key.into(), value);
            }
            let encoded = encode::encode_request_checked(&canonical, "claude-test").unwrap();
            let block = if nested {
                &encoded["messages"][0]["content"][0]["content"][0]
            } else {
                &encoded["messages"][0]["content"][0]
            };
            assert_eq!(block["title"], "New title");
            assert!(block.get("context").is_none());
            assert!(block.get("citations").is_none());
            assert_eq!(block["cache_control"], json!({"type":"ephemeral"}));
            assert_eq!(block["future_document"], 7);
            assert_eq!(
                block["source"]["content"],
                json!([{"type":"text","text":"Custom text"}])
            );
            let mut restored = decode::decode_request(&encoded).unwrap();
            let metadata = match &mut restored.input[0] {
                Node::File { metadata, .. } => metadata,
                Node::ToolResult { content, .. } => match &mut content[0] {
                    super::ToolResultContent::File { metadata, .. } => metadata,
                    _ => unreachable!(),
                },
                _ => unreachable!(),
            };
            metadata.document_title = None;
            let deleted = encode::encode_request_checked(&restored, "claude-test").unwrap();
            assert!(!deleted.to_string().contains("New title"));
        }
    }
}

#[test]
fn messages_cross_protocol_media_preparation_and_errors() {
    for stream in [false, true] {
        let native = json!({"model":"model","stream":stream,"messages":[{"role":"user","content":[
            {"type":"file","file":{"file_data":"JVBERi0xLjcK","filename":"note.pdf"}},
            {"type":"file","file":{"file_data":"data:application/pdf;base64,JVBERi0xLjcK","filename":"other.pdf"}},
            {"type":"image_url","image_url":{"url":"data:image/png;base64,aGVsbG8="}}
        ]}]});
        let canonical = super::decode::openai_chat::decode_request(&native).unwrap();
        let before = serde_json::to_value(&canonical).unwrap();
        let encoded = encode::encode_request_checked(&canonical, "claude-test").unwrap();
        assert_eq!(serde_json::to_value(&canonical).unwrap(), before);
        for index in 0..2 {
            let document = &encoded["messages"][0]["content"][index];
            assert_eq!(
                document["source"],
                json!({"type":"base64","media_type":"application/pdf","data":"JVBERi0xLjcK"})
            );
            assert!(document["source"].get("filename").is_none());
        }
        assert_eq!(
            encoded["messages"][0]["content"][2]["source"],
            json!({"type":"base64","media_type":"image/png","data":"aGVsbG8="})
        );

        for (mime, data, text) in [
            ("application/json", "eyJ4IjoxfQ==", "{\"x\":1}"),
            ("text/plain", "aGVsbG8=", "hello"),
        ] {
            let mut canonical =
                super::decode::gemini::decode_request(&json!({"contents":[{"role":"user","parts":[
                    {"inlineData":{"mimeType":mime,"data":data}}
                ]}]}))
                .unwrap();
            canonical.stream = Some(stream);
            let encoded = encode::encode_request_checked(&canonical, "claude-test").unwrap();
            assert_eq!(
                encoded["messages"][0]["content"][0]["source"],
                json!({"type":"text","media_type":"text/plain","data":text})
            );
        }
        for (mime, data) in [
            ("image/heic", "aGVsbG8="),
            ("application/zip", "UEsDBAo="),
            ("audio/wav", "UklGRg=="),
        ] {
            let mut canonical =
                super::decode::gemini::decode_request(&json!({"contents":[{"role":"user","parts":[
                    {"text":"Keep this text"},{"inlineData":{"mimeType":mime,"data":data}}
                ]}]}))
                .unwrap();
            canonical.stream = Some(stream);
            assert!(
                encode::encode_request_checked(&canonical, "claude-test").is_err(),
                "{mime}"
            );
            let error = encode::encode_request(&canonical, "claude-test");
            assert_eq!(error["type"], "error");
            assert!(error.get("messages").is_none());
        }
    }
}

#[test]
fn messages_file_reference_metadata_is_typed_and_required() {
    for stream in [false, true] {
        for kind in ["image", "document"] {
            for nested in [false, true] {
                let media = json!({"type":kind,"source":{"type":"file","file_id":"file_owned"}});
                let block = if nested {
                    json!({"type":"tool_result","tool_use_id":"toolu_file","content":[media]})
                } else {
                    media
                };
                let mut canonical =
                    decode::decode_request(&json!({"model":"claude-test","stream":stream,
                    "messages":[{"role":"user","content":[block]}]}))
                    .unwrap();
                let encoded = encode::encode_request_checked(&canonical, "claude-test").unwrap();
                assert!(encoded.to_string().contains("file_owned"));
                assert!(!encoded.to_string().contains("resource"));
                let (metadata, extra) = match &mut canonical.input[0] {
                    Node::Image {
                        metadata,
                        extra_body,
                        ..
                    }
                    | Node::File {
                        metadata,
                        extra_body,
                        ..
                    } => (metadata, extra_body),
                    Node::ToolResult { content, .. } => match &mut content[0] {
                        super::ToolResultContent::Image {
                            metadata,
                            extra_body,
                            ..
                        }
                        | super::ToolResultContent::File {
                            metadata,
                            extra_body,
                            ..
                        } => (metadata, extra_body),
                        _ => unreachable!(),
                    },
                    _ => unreachable!(),
                };
                assert_eq!(
                    metadata.resource.as_ref().unwrap().protocol,
                    ProviderProtocol::Messages
                );
                assert!(!extra.keys().any(|key| key.contains("file_id_origin")));
                metadata.resource = None;
                assert!(encode::encode_request_checked(&canonical, "claude-test").is_err());
            }
        }
    }
}

#[test]
fn messages_compound_document_rejects_unsupported_image_mime() {
    for stream in [false, true] {
        let canonical = decode::decode_request(&json!({"model":"claude-test","stream":stream,"messages":[{"role":"user","content":[
            {"type":"document","source":{"type":"content","content":[
                {"type":"text","text":"Before unsupported image"},
                {"type":"image","source":{"type":"base64","media_type":"image/heic","data":"aGVsbG8="}}
            ]}}
        ]}]})).unwrap();
        assert!(encode::encode_request_checked(&canonical, "claude-test").is_err());
    }
}

#[test]
fn messages_malformed_media_sources_are_request_errors() {
    for stream in [false, true] {
        for block in [
            json!({"type":"image","source":{"type":"unknown"}}),
            json!({"type":"image","source":{"type":"unknown","data":"aGVsbG8="}}),
            json!({"type":"document","source":{"type":"text","media_type":"text/plain"}}),
        ] {
            for nested in [false, true] {
                let content = if nested {
                    json!({"type":"tool_result","tool_use_id":"toolu_1","content":[block]})
                } else {
                    block.clone()
                };
                assert!(decode::decode_request(&json!({"model":"claude-test","stream":stream,"messages":[{"role":"user","content":[content]}]})).is_err());
            }
        }
    }
}

#[test]
fn messages_missing_image_mime_uses_bytes_or_rejects() {
    for stream in [false, true] {
        for (data, expected) in [
            ("aGVsbG8=", None),
            (
                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aT54AAAAASUVORK5CYII=",
                Some("image/png"),
            ),
        ] {
            let canonical = decode::decode_request(
                &json!({"model":"claude-test","stream":stream,"messages":[{"role":"user","content":[
                    {"type":"image","source":{"type":"base64","data":data}}
                ]}]}),
            )
            .unwrap();
            match expected {
                Some(mime) => {
                    let encoded =
                        encode::encode_request_checked(&canonical, "claude-test").unwrap();
                    assert_eq!(
                        encoded["messages"][0]["content"][0]["source"]["media_type"],
                        mime
                    );
                    assert_eq!(encoded["messages"][0]["content"][0]["source"]["data"], data);
                }
                None => assert!(encode::encode_request_checked(&canonical, "claude-test").is_err()),
            }
        }
    }
}

#[tokio::test]
async fn messages_compatible_media_responses_preserve_typed_nodes_in_actual_sse() {
    for block in [
        json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"aGVsbG8="}}),
        json!({"type":"image_url","image_url":{"url":"https://example.com/image.png","detail":"high"}}),
        json!({"type":"document","title":"Custom document","source":{"type":"content","content":"Document text"}}),
        json!({"type":"input_file","filename":"notes.json","file_data":"data:application/json;base64,eyJ4IjoxfQ=="}),
        json!({"type":"file","source":{"type":"file","file_id":"file_doc"}}),
        json!({"type":"audio","source":{"type":"base64","media_type":"audio/wav","data":"UklGRg=="}}),
        json!({"type":"input_audio","input_audio":{"format":"mp3","data":"SUQz"}}),
        json!({"type":"output_audio","source":{"type":"url","url":"https://example.com/audio.wav","media_type":"audio/wav"}}),
    ] {
        let canonical = decode::decode_response(&response(vec![block.clone()])).unwrap();
        assert_eq!(canonical.output.len(), 1);
        assert!(matches!(
            &canonical.output[0],
            Node::Image { .. } | Node::File { .. } | Node::Audio { .. }
        ));
        let mut singleton = response(vec![]);
        singleton["content"] = block.clone();
        assert_eq!(
            serde_json::to_value(decode::decode_response(&singleton).unwrap().output).unwrap(),
            serde_json::to_value(&canonical.output).unwrap()
        );
        let events = decode_stream(frames(&[block], "end_turn")).await;
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, UrpStreamEvent::Error { .. }))
        );
        assert_eq!(
            serde_json::to_value(terminal(&events).output).unwrap(),
            serde_json::to_value(&canonical.output).unwrap()
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, UrpStreamEvent::NodeStart { .. }))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event,
                    UrpStreamEvent::NodeDelta {
                        delta: super::NodeDelta::Image { .. }
                            | super::NodeDelta::File { .. }
                            | super::NodeDelta::Audio { .. },
                        ..
                    }
                ))
                .count(),
            1
        );
        assert!(encode::encode_response_checked(&canonical, "claude-test").is_err());
    }
}

#[tokio::test]
async fn messages_malformed_compatible_media_remains_explicit_decode_error() {
    for block in [
        json!({"type":"image_url","image_url":{}}),
        json!({"type":"input_file","file_data":42}),
        json!({"type":"document","source":{"type":"unknown","data":"hello"}}),
        json!({"type":"output_audio","source":{"type":"unknown","data":"UklGRg=="}}),
    ] {
        assert!(decode::decode_response(&response(vec![block.clone()])).is_err());
        for stream in [false, true] {
            for nested in [false, true] {
                let content = if nested {
                    json!({"type":"tool_result","tool_use_id":"toolu_bad","content":block})
                } else {
                    block.clone()
                };
                assert!(decode::decode_request(&json!({"model":"claude-test","stream":stream,"messages":[{"role":"user","content":content}]})).is_err());
            }
        }
        let events = decode_stream(frames(&[block], "end_turn")).await;
        assert!(events.iter().any(|event| matches!(event, UrpStreamEvent::Error { code:Some(code), .. } if code == "messages_media_content_invalid")));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, UrpStreamEvent::ResponseDone { .. }))
        );
    }
}

#[tokio::test]
async fn messages_unsupported_output_media_errors_in_all_response_modes() {
    let nodes = vec![
        Node::Image {
            id: None,
            role: super::OrdinaryRole::Assistant,
            source: super::ImageSource::Base64 {
                media_type: "image/png".into(),
                data: "aGVsbG8=".into(),
            },
            metadata: Default::default(),
            extra_body: HashMap::new(),
        },
        Node::File {
            id: None,
            role: super::OrdinaryRole::Assistant,
            source: super::FileSource::Text {
                text: "Document".into(),
            },
            metadata: Default::default(),
            extra_body: HashMap::new(),
        },
        Node::Audio {
            id: None,
            role: super::OrdinaryRole::Assistant,
            source: super::AudioSource::Base64 {
                media_type: "audio/wav".into(),
                data: "UklGRg==".into(),
            },
            metadata: Default::default(),
            extra_body: HashMap::new(),
        },
        Node::ProviderItem {
            id: None,
            role: super::OrdinaryRole::Assistant,
            origin_protocol: ProviderProtocol::Messages,
            item_type: "document".into(),
            body: json!({"type":"text","text":"stale type"}),
            extra_body: HashMap::new(),
        },
        decode::decode_response(&response(vec![
            json!({"type":"tool_result","tool_use_id":"toolu_result","content":"Result text"}),
        ]))
        .unwrap()
        .output
        .into_iter()
        .next()
        .unwrap(),
    ];
    for node in nodes {
        let mut canonical = decode::decode_response(&response(vec![
            json!({"type":"text","text":"Before media"}),
        ]))
        .unwrap();
        canonical.output.push(node.clone());
        let before = serde_json::to_value(&canonical).unwrap();
        assert!(
            encode::encode_response_checked(&canonical, "claude-test")
                .unwrap_err()
                .contains("top-level")
        );
        assert_eq!(
            encode::encode_response(&canonical, "claude-test")["type"],
            "error"
        );
        let (tx, rx) = mpsc::channel(1024);
        let result = crate::urp::stream_encode::anthropic::emit_synthetic_messages_stream(
            "claude-test",
            &canonical,
            None,
            tx,
        )
        .await;
        assert!(result.is_err());
        let wire = sse_json(rx).await;
        assert_eq!(wire.len(), 1);
        assert_eq!(wire[0]["type"], "error");
        assert_eq!(wire[0]["error"]["type"], "api_error");
        assert_eq!(serde_json::to_value(&canonical).unwrap(), before);
        let header = match &node {
            Node::Image { metadata, role, .. } => super::NodeHeader::Image {
                id: None,
                role: *role,
                metadata: metadata.clone(),
            },
            Node::File { metadata, role, .. } => super::NodeHeader::File {
                id: None,
                role: *role,
                metadata: metadata.clone(),
            },
            Node::Audio { metadata, role, .. } => super::NodeHeader::Audio {
                id: None,
                role: *role,
                metadata: metadata.clone(),
            },
            Node::ProviderItem {
                origin_protocol,
                role,
                item_type,
                body,
                ..
            } => super::NodeHeader::ProviderItem {
                id: None,
                origin_protocol: *origin_protocol,
                role: *role,
                item_type: item_type.clone(),
                body: Some(body.clone()),
            },
            Node::ToolResult {
                namespace,
                signature,
                name,
                id,
                tool_type,
                call_id,
                ..
            } => super::NodeHeader::ToolResult {
                namespace: namespace.clone(),
                signature: signature.clone(),
                name: name.clone(),
                id: id.clone(),
                tool_type: *tool_type,
                call_id: call_id.clone(),
            },
            _ => unreachable!(),
        };
        let mut endings = vec![
            UrpStreamEvent::NodeStart {
                node_index: 1,
                header,
                extra_body: HashMap::new(),
            },
            UrpStreamEvent::NodeDone {
                node_index: 1,
                node: node.clone(),
                usage: None,
                extra_body: HashMap::new(),
            },
            UrpStreamEvent::ResponseDone {
                outcome: None,
                finish_reason: Some(super::FinishReason::Stop),
                usage: None,
                output: canonical.output.clone(),
                extra_body: HashMap::new(),
            },
        ];
        let delta = match &node {
            Node::Image { source, .. } => Some(super::NodeDelta::Image {
                source: source.clone(),
            }),
            Node::File { source, .. } => Some(super::NodeDelta::File {
                source: source.clone(),
            }),
            Node::Audio { source, .. } => Some(super::NodeDelta::Audio {
                source: source.clone(),
            }),
            _ => None,
        };
        if let Some(delta) = delta {
            endings.push(UrpStreamEvent::NodeDelta {
                node_index: 1,
                delta,
                usage: None,
                extra_body: HashMap::new(),
            });
        }
        for ending in endings {
            let (source_tx, source_rx) = mpsc::channel(1024);
            source_tx
                .send(UrpStreamEvent::ResponseStart {
                    id: canonical.id.clone(),
                    model: canonical.model.clone(),
                    usage: None,
                    extra_body: HashMap::new(),
                })
                .await
                .unwrap();
            source_tx
                .send(UrpStreamEvent::NodeDone {
                    node_index: 0,
                    node: canonical.output[0].clone(),
                    usage: None,
                    extra_body: HashMap::new(),
                })
                .await
                .unwrap();
            source_tx.send(ending).await.unwrap();
            drop(source_tx);
            let (tx, rx) = mpsc::channel(1024);
            let result = crate::urp::stream_encode::anthropic::encode_urp_stream_as_messages(
                source_rx,
                tx,
                "claude-test",
                None,
                false,
            )
            .await;
            assert!(result.is_err());
            let wire = sse_json(rx).await;
            assert_eq!(
                wire.iter().filter(|event| event["type"] == "error").count(),
                1
            );
            assert!(
                !wire.iter().any(
                    |event| event["type"] == "message_stop" || event["type"] == "message_delta"
                )
            );
            assert!(!wire.iter().any(|event| matches!(
                event["content_block"]["type"].as_str(),
                Some("image" | "document" | "file" | "audio")
            )));
        }
    }
}

macro_rules! provider_feature {
    ($name:ident, $call:expr, $result:expr) => {
        #[tokio::test]
        async fn $name() {
            let blocks = vec![$call, $result];
            let urp = decode::decode_response(&response(blocks.clone())).unwrap();
            assert!(urp.output.iter().all(|node| matches!(
                node,
                Node::ProviderItem {
                    origin_protocol: ProviderProtocol::Messages,
                    ..
                }
            )));
            let mut cross = urp.output.clone();
            super::retain_provider_items_for_protocol(&mut cross, ProviderProtocol::Responses);
            assert!(cross.is_empty());
            assert_block_roundtrip(blocks).await;
        }
    };
}

provider_feature!(
    messages_web_search_bidirectional,
    json!({"type":"server_tool_use","id":"srvtoolu_search","name":"web_search","input":{"query":"Rust"}}),
    json!({"type":"web_search_tool_result","tool_use_id":"srvtoolu_search","content":[{"type":"web_search_result","url":"https://www.rust-lang.org","title":"Rust","encrypted_content":"cipher","page_age":"1 day"}]})
);
provider_feature!(
    messages_web_fetch_bidirectional,
    json!({"type":"server_tool_use","id":"srvtoolu_fetch","name":"web_fetch","input":{"url":"https://example.com"}}),
    json!({"type":"web_fetch_tool_result","tool_use_id":"srvtoolu_fetch","content":{"type":"web_fetch_result","url":"https://example.com","retrieved_at":"2026-09-12T00:00:00Z","content":{"type":"document","title":"Example","source":{"type":"text","media_type":"text/plain","data":"Fetched text"},"citations":{"enabled":true}}}})
);
provider_feature!(
    messages_code_execution_bidirectional,
    json!({"type":"server_tool_use","id":"srvtoolu_code","name":"code_execution","input":{"code":"print(1)"}}),
    json!({"type":"code_execution_tool_result","tool_use_id":"srvtoolu_code","content":{"type":"code_execution_result","stdout":"1\n","stderr":"","return_code":0,"content":[]}})
);
provider_feature!(
    messages_bash_execution_bidirectional,
    json!({"type":"server_tool_use","id":"srvtoolu_bash","name":"bash_code_execution","input":{"command":"pwd"}}),
    json!({"type":"bash_code_execution_tool_result","tool_use_id":"srvtoolu_bash","content":{"type":"bash_code_execution_result","stdout":"/workspace\n","stderr":"","return_code":0,"content":[]}})
);
provider_feature!(
    messages_text_editor_execution_bidirectional,
    json!({"type":"server_tool_use","id":"srvtoolu_editor","name":"text_editor_code_execution","input":{"command":"view","path":"/workspace/file.txt"}}),
    json!({"type":"text_editor_code_execution_tool_result","tool_use_id":"srvtoolu_editor","content":{"type":"text_editor_code_execution_view_result","content":"Text","file_type":"text","num_lines":1,"start_line":1,"total_lines":1}})
);
provider_feature!(
    messages_tool_search_bidirectional,
    json!({"type":"server_tool_use","id":"srvtoolu_tools","name":"tool_search_tool_regex","input":{"pattern":"weather","limit":10}}),
    json!({"type":"tool_search_tool_result","tool_use_id":"srvtoolu_tools","content":{"type":"tool_search_tool_search_result","tool_references":[{"type":"tool_reference","tool_name":"get_weather"}]}})
);
provider_feature!(
    messages_mcp_bidirectional,
    json!({"type":"mcp_tool_use","id":"mcptoolu_1","name":"get_weather","server_name":"weather","input":{"city":"Paris"}}),
    json!({"type":"mcp_tool_result","tool_use_id":"mcptoolu_1","is_error":false,"content":[{"type":"text","text":"Sunny"}]})
);

#[test]
fn messages_request_tools_configuration_and_results_bidirectional() {
    for stream in [false, true] {
        let native = json!({
            "model":"claude-test","max_tokens":4096,"stream":stream,"temperature":0.8,"top_p":0.9,"top_k":5,
            "system":[{"type":"text","text":"Be concise.","cache_control":{"type":"ephemeral"}}],
            "thinking":{"type":"enabled","budget_tokens":1024,"display":"summarized","future_option":1},
            "output_config":{"effort":"high","format":{"type":"json_schema","schema":{"type":"object","properties":{"answer":{"type":"string"}}},"future_format":true}},
            "metadata":{"user_id":"caller","session_tag":"s"},"stop_sequences":["DONE"],
            "tool_choice":{"type":"auto","disable_parallel_tool_use":true},
            "tools":[
                {"name":"lookup","description":"Look up a value.","input_schema":{"type":"object","properties":{"key":{"type":"string"}}},"strict":true,"cache_control":{"type":"ephemeral"},"defer_loading":true},
                {"type":"computer_20251124","name":"computer","display_width_px":1024,"display_height_px":768,"enable_zoom":true},
                {"type":"computer_toolset_20260801","configs":{"zoom":{"enabled":false}},"cache_control":{"type":"ephemeral"}},
                {"type":"browser_toolset_20260801","configs":{"screenshot":{"enabled":false}}},
                {"type":"web_search_20250305","name":"web_search","max_uses":2,"allowed_domains":["example.com"]},
                {"type":"web_fetch_20250910","name":"web_fetch","max_content_tokens":1000},
                {"type":"code_execution_20250825","name":"code_execution"},
                {"type":"tool_search_tool_regex_20251119","name":"tool_search_tool_regex"},
                {"type":"mcp_toolset","mcp_server_name":"server","default_config":{"enabled":true}}
            ],
            "mcp_servers":[{"type":"url","name":"server","url":"https://example.com/mcp"}],
            "container":{"id":"container_1"},"context_management":{"edits":[]},
            "messages":[{"role":"assistant","content":[{"type":"tool_use","id":"toolu_member","name":"screenshot","toolset_name":"computer","input":{}}]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_member","toolset_name":"computer","is_error":false,"content":[
                    {"type":"text","text":"Screenshot"},
                    {"type":"image","source":{"type":"base64","media_type":"image/png","data":"aGVsbG8="}},
                    {"type":"document","source":{"type":"text","media_type":"text/plain","data":"Log"}},
                    {"type":"search_result","source":"https://example.com","title":"Search","content":[{"type":"text","text":"Result"}]},
                    {"type":"tool_reference","tool_name":"lookup"}
                ]}]}]
        });
        let urp = decode::decode_request(&native).unwrap();
        assert_eq!(urp.parallel_tool_calls, Some(false));
        assert_eq!(urp.user.as_deref(), Some("caller"));
        assert!(
            matches!(&urp.input[1],Node::ToolCall {namespace:Some(namespace),..} if namespace=="computer")
        );
        assert!(
            matches!(&urp.input[2],Node::ToolResult {namespace:Some(namespace),..} if namespace=="computer")
        );
        for tool in urp.tools.as_ref().unwrap().iter().skip(1) {
            assert_eq!(tool.origin_protocol, Some(ProviderProtocol::Messages));
            assert!(tool.extra_body.is_empty());
            assert!(tool.config.is_some());
        }
        let encoded = encode::encode_request(&urp, "claude-test");
        for key in [
            "model",
            "stream",
            "max_tokens",
            "temperature",
            "top_p",
            "top_k",
            "system",
            "thinking",
            "output_config",
            "metadata",
            "stop_sequences",
            "tool_choice",
            "tools",
            "mcp_servers",
            "container",
            "context_management",
            "messages",
        ] {
            assert_eq!(
                encoded[key], native[key],
                "request field {key}, stream={stream}"
            );
        }
        let decoded = decode::decode_request(&encoded).unwrap();
        assert_eq!(
            serde_json::to_value(&decoded).unwrap(),
            serde_json::to_value(&urp).unwrap()
        );
    }
}

#[test]
fn messages_typed_deletions_and_mutations_own_nonstream_values() {
    let mut urp = decode::decode_request(&json!({"model":"claude-test","max_tokens":2048,"thinking":{"type":"enabled","budget_tokens":1024,"display":"summarized"},"output_config":{"effort":"high","format":{"type":"json_schema","schema":{"type":"object"}}},"metadata":{"user_id":"old","tag":1},"stop_sequences":["stop"],"tools":[{"type":"computer_20251124","name":"computer","display_width_px":1024}],"messages":[{"role":"assistant","content":[{"type":"tool_use","id":"toolu","name":"screenshot","toolset_name":"computer","input":{"old":true}}]},{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu","toolset_name":"computer","content":"old"}]}]})).unwrap();
    urp.user = None;
    urp.stop = None;
    urp.reasoning = None;
    urp.response_format = None;
    let tool = &mut urp.tools.as_mut().unwrap()[0];
    tool.name = None;
    tool.config = None;
    if let Node::ToolCall {
        namespace,
        name,
        arguments,
        ..
    } = &mut urp.input[0]
    {
        *namespace = None;
        *name = "new".into();
        *arguments = "{}".into();
    } else {
        panic!("tool call")
    }
    if let Node::ToolResult {
        namespace, content, ..
    } = &mut urp.input[1]
    {
        *namespace = None;
        content.clear();
    } else {
        panic!("tool result")
    }
    let encoded = encode::encode_request(&urp, "claude-test");
    assert!(encoded.get("thinking").is_none());
    assert!(encoded.get("output_config").is_none());
    assert!(encoded.get("stop_sequences").is_none());
    assert_eq!(encoded["metadata"], json!({"tag":1}));
    assert_eq!(encoded["tools"], json!([{"type":"computer_20251124"}]));
    assert!(
        encoded["messages"][0]["content"][0]
            .get("toolset_name")
            .is_none()
    );
    assert_eq!(encoded["messages"][0]["content"][0]["input"], json!({}));
    assert!(
        encoded["messages"][1]["content"][0]
            .get("toolset_name")
            .is_none()
    );
    assert!(!encoded.to_string().contains("old"));
    let mut response = decode::decode_response(&response(vec![json!({"type":"text","text":"answer","phase":"analysis","citations":[{"type":"char_location","cited_text":"x"}]}),json!({"type":"thinking","thinking":"secret","signature":"signature"})])).unwrap();
    if let Node::Text {
        logprobs: _,
        phase,
        citations,
        ..
    } = &mut response.output[0]
    {
        *phase = None;
        citations.clear();
    }
    if let Node::Reasoning {
        summary, encrypted, ..
    } = &mut response.output[1]
    {
        *summary = None;
        *encrypted = None;
    }
    let encoded = encode::encode_response(&response, "claude-test");
    assert_eq!(encoded["content"], json!([{"type":"text","text":"answer"}]));
}

#[tokio::test]
async fn messages_stop_reasons_and_usage_bidirectional_and_typed_override() {
    for reason in [
        "end_turn",
        "stop_sequence",
        "max_tokens",
        "tool_use",
        "refusal",
        "pause_turn",
        "model_context_window_exceeded",
    ] {
        let mut wire = response(vec![json!({"type":"text","text":"Answer"})]);
        wire["stop_reason"] = json!(reason);
        wire["stop_sequence"] = if reason == "stop_sequence" {
            json!("DONE")
        } else {
            Value::Null
        };
        wire["usage"] = json!({"input_tokens":11,"output_tokens":7,"cache_read_input_tokens":5,"cache_creation_input_tokens":4,"cache_creation":{"ephemeral_5m_input_tokens":3,"ephemeral_1h_input_tokens":1},"server_tool_use":{"web_search_requests":1}});
        let mut urp = decode::decode_response(&wire).unwrap();
        assert_eq!(urp.usage.as_ref().unwrap().input_tokens, 20);
        assert_eq!(
            encode::encode_response(&urp, "claude-test")["stop_reason"],
            json!(reason)
        );
        let streamed = terminal(&decode_stream(synthetic_stream(&urp).await).await);
        assert_eq!(streamed.finish_reason, urp.finish_reason);
        assert_eq!(streamed.extra_body["stop_reason"], json!(reason));
        assert_eq!(streamed.usage.as_ref().unwrap().input_tokens, 20);
        urp.finish_reason = Some(super::FinishReason::Length);
        let encoded = encode::encode_response(&urp, "claude-test");
        assert_eq!(encoded["stop_reason"], json!("max_tokens"));
        assert_eq!(encoded["stop_sequence"], Value::Null);
        let encoded_stream = synthetic_stream(&urp).await;
        assert_eq!(
            encoded_stream
                .iter()
                .find(|f| f["type"] == "message_delta")
                .unwrap()["delta"]["stop_reason"],
            json!("max_tokens")
        );
    }
}

#[tokio::test]
async fn messages_stream_errors_are_terminal_and_preserve_details() {
    let decoded = decode_stream(vec![json!({"type":"error","error":{"type":"overloaded_error","message":"Try later","param":"messages","details":{"retry":true}},"request_id":"req_1"}),json!({"type":"message_stop"})]).await;
    assert_eq!(decoded.len(), 1);
    assert!(
        matches!(&decoded[0],UrpStreamEvent::Error {code:Some(code),message,..} if code=="overloaded_error" && message=="Try later")
    );
    let encoded = encode_stream(decoded).await;
    assert_eq!(encoded.len(), 1);
    let error = encoded.iter().find(|f| f["type"] == "error").unwrap();
    assert_eq!(error["request_id"], "req_1");
    assert_eq!(error["error"]["param"], "messages");
    assert_eq!(error["error"]["type"], "overloaded_error");
    assert_eq!(error["error"]["details"], json!({"retry":true}));
    assert!(!encoded.iter().any(|f| f["type"] == "message_stop"));
}

#[tokio::test]
async fn messages_provider_start_uses_typed_body_without_internal_snapshot() {
    let block = json!({"type":"server_tool_use","id":"srvtoolu","name":"web_search","input":{"query":"old"}});
    let events = decode_stream(frames(&[block], "end_turn")).await;
    for event in &events {
        if let UrpStreamEvent::NodeStart {
            header: super::NodeHeader::ProviderItem { body, .. },
            extra_body,
            ..
        } = event
        {
            assert!(body.is_some());
            assert!(!extra_body.contains_key("_monoize_messages_provider_item_start_body"));
        }
    }
    let mut urp = terminal(&events);
    if let Node::ProviderItem { body, .. } = &mut urp.output[0] {
        body["input"] = json!({"query":"new"});
    }
    let roundtrip = terminal(&decode_stream(synthetic_stream(&urp).await).await);
    let wire = encode::encode_response(&roundtrip, "claude-test");
    assert_eq!(wire["content"][0]["input"], json!({"query":"new"}));
}

#[tokio::test]
async fn messages_late_citations_after_closed_text_block_are_explicit_stream_errors() {
    let citation = json!({"uri":"https://example.com/source", "startIndex":0, "endIndex":6});
    let text: Node = serde_json::from_value(json!({
        "type":"text", "role":"assistant", "content":"answer"
    }))
    .unwrap();
    let mut cited_text = text.clone();
    if let Node::Text {
        logprobs: _,
        citations,
        ..
    } = &mut cited_text
    {
        citations.push(crate::urp::Citation::decode(
            citation.clone(),
            crate::urp::ProviderProtocol::Messages,
        ));
    }
    let events = vec![
        UrpStreamEvent::ResponseStart {
            id: "msg_feature".into(),
            model: "claude-test".into(),
            usage: None,
            extra_body: HashMap::new(),
        },
        UrpStreamEvent::NodeStart {
            node_index: 0,
            header: super::NodeHeader::Text {
                id: None,
                role: super::OrdinaryRole::Assistant,
                phase: None,
                signature: None,
                citations: vec![],
            },
            extra_body: HashMap::new(),
        },
        UrpStreamEvent::NodeDelta {
            node_index: 0,
            delta: super::NodeDelta::Text {
                logprobs: None,
                content: "answer".into(),
                citations: vec![],
                signature: None,
            },
            usage: None,
            extra_body: HashMap::new(),
        },
        UrpStreamEvent::NodeDone {
            node_index: 0,
            node: text,
            usage: None,
            extra_body: HashMap::new(),
        },
        UrpStreamEvent::NodeDelta {
            node_index: 0,
            delta: super::NodeDelta::Text {
                logprobs: None,
                content: String::new(),
                citations: vec![crate::urp::Citation::decode(
                    citation,
                    crate::urp::ProviderProtocol::Messages,
                )],
                signature: None,
            },
            usage: None,
            extra_body: HashMap::new(),
        },
        UrpStreamEvent::ResponseDone {
            outcome: None,
            finish_reason: Some(super::FinishReason::Stop),
            usage: None,
            output: vec![cited_text],
            extra_body: HashMap::new(),
        },
    ];
    let (source_tx, source_rx) = mpsc::channel(1024);
    for event in events {
        source_tx.send(event).await.unwrap();
    }
    drop(source_tx);
    let (tx, rx) = mpsc::channel(1024);
    let error = crate::urp::stream_encode::anthropic::encode_urp_stream_as_messages(
        source_rx,
        tx,
        "claude-test",
        None,
        false,
    )
    .await
    .unwrap_err();
    assert!(error.message.contains("closed text block"));
    assert!(error.downstream_stream_terminal_sent);
    let wire = sse_json(rx).await;
    assert_eq!(
        wire.iter().filter(|frame| frame["type"] == "error").count(),
        1
    );
    assert_eq!(
        wire.iter()
            .filter(|frame| frame["type"] == "content_block_start")
            .count(),
        1
    );
    assert!(
        wire.iter()
            .any(|frame| frame["type"] == "content_block_stop")
    );
    assert!(wire.iter().any(|frame| frame["delta"]["text"] == "answer"));
    assert!(!wire.iter().any(|frame| matches!(
        frame["type"].as_str(),
        Some("message_stop" | "message_delta")
    )));
}

#[tokio::test]
async fn messages_buffered_terminal_deletions_override_accumulated_values() {
    let text_header = super::NodeHeader::Text {
        id: None,
        role: super::OrdinaryRole::Assistant,
        phase: None,
        signature: None,
        citations: vec![],
    };
    let blocker: Node =
        serde_json::from_value(json!({"type":"text","role":"assistant","content":"first"}))
            .unwrap();
    let cleared: Node =
        serde_json::from_value(json!({"type":"text","role":"assistant","content":""})).unwrap();
    let events = vec![
        UrpStreamEvent::ResponseStart {
            id: "msg_feature".into(),
            model: "claude-test".into(),
            usage: None,
            extra_body: HashMap::new(),
        },
        UrpStreamEvent::NodeStart {
            node_index: 0,
            header: text_header.clone(),
            extra_body: HashMap::new(),
        },
        UrpStreamEvent::NodeStart {
            node_index: 1,
            header: text_header,
            extra_body: HashMap::new(),
        },
        UrpStreamEvent::NodeDelta {
            node_index: 1,
            delta: super::NodeDelta::Text {
                logprobs: None,
                content: "deleted".into(),
                citations: vec![],
                signature: None,
            },
            usage: None,
            extra_body: HashMap::new(),
        },
        UrpStreamEvent::NodeDone {
            node_index: 1,
            node: cleared.clone(),
            usage: None,
            extra_body: HashMap::new(),
        },
        UrpStreamEvent::NodeDone {
            node_index: 0,
            node: blocker.clone(),
            usage: None,
            extra_body: HashMap::new(),
        },
        UrpStreamEvent::ResponseDone {
            outcome: None,
            finish_reason: Some(super::FinishReason::Stop),
            usage: None,
            output: vec![blocker, cleared],
            extra_body: HashMap::new(),
        },
    ];
    let encoded = encode_stream(events).await;
    assert!(!serde_json::to_string(&encoded).unwrap().contains("deleted"));
    let roundtrip = terminal(&decode_stream(encoded).await);
    assert_eq!(
        encode::encode_response(&roundtrip, "claude-test")["content"],
        json!([{"type":"text","text":"first"}])
    );
}

#[test]
fn messages_parallel_control_mutations_and_unknown_choice_fields() {
    for kind in ["auto", "any", "tool"] {
        let mut choice = json!({"type":kind,"disable_parallel_tool_use":true,"future_choice":7});
        if kind == "tool" {
            choice["name"] = json!("lookup");
        }
        let mut urp = decode::decode_request(
            &json!({"model":"claude-test","messages":[],"tool_choice":choice}),
        )
        .unwrap();
        assert_eq!(urp.parallel_tool_calls, Some(false));
        assert!(
            !serde_json::to_value(&urp.tool_choice)
                .unwrap()
                .to_string()
                .contains("disable_parallel")
        );
        let encoded = encode::encode_request(&urp, "claude-test");
        assert_eq!(encoded["tool_choice"], choice);
        urp.parallel_tool_calls = Some(true);
        assert_eq!(
            encode::encode_request(&urp, "claude-test")["tool_choice"]["disable_parallel_tool_use"],
            false
        );
        urp.parallel_tool_calls = None;
        let encoded = encode::encode_request(&urp, "claude-test");
        assert!(
            encoded["tool_choice"]
                .get("disable_parallel_tool_use")
                .is_none()
        );
        assert_eq!(encoded["tool_choice"]["future_choice"], 7);
    }
}

#[tokio::test]
async fn messages_provider_typed_identity_and_kind_override_native_body() {
    let mut urp=decode::decode_response(&response(vec![json!({"type":"server_tool_use","id":"srvtoolu_old","name":"web_search","input":{"query":"Rust"}})])).unwrap();
    if let Node::ProviderItem { id, item_type, .. } = &mut urp.output[0] {
        *id = Some("srvtoolu_new".into());
        *item_type = "future_server_tool_use".into();
    }
    let nonstream = encode::encode_response(&urp, "claude-test");
    assert_eq!(nonstream["content"][0]["id"], "srvtoolu_new");
    assert_eq!(nonstream["content"][0]["type"], "future_server_tool_use");
    let streamed = terminal(&decode_stream(synthetic_stream(&urp).await).await);
    assert_eq!(
        encode::encode_response(&streamed, "claude-test")["content"],
        nonstream["content"]
    );
    if let Node::ProviderItem { id, .. } = &mut urp.output[0] {
        *id = None;
    }
    assert!(
        encode::encode_response(&urp, "claude-test")["content"][0]
            .get("id")
            .is_none()
    );
    let frames = synthetic_stream(&urp).await;
    let block = &frames
        .iter()
        .find(|f| f["type"] == "content_block_start")
        .unwrap()["content_block"];
    assert!(block.get("id").is_none());
}

#[tokio::test]
async fn messages_terminal_snapshot_emits_unstreamed_nodes_of_same_kind() {
    let blocks = vec![
        json!({"type":"text","text":"First"}),
        json!({"type":"text","text":"Second"}),
        json!({"type":"tool_use","id":"toolu_1","name":"first","input":{}}),
        json!({"type":"tool_use","id":"toolu_2","name":"second","input":{}}),
    ];
    let response = decode::decode_response(&response(blocks.clone())).unwrap();
    let mut events = decode_stream(frames(&blocks[..1], "end_turn")).await;
    let last = events
        .iter_mut()
        .find(|event| matches!(event, UrpStreamEvent::ResponseDone { .. }))
        .unwrap();
    if let UrpStreamEvent::ResponseDone {
        outcome: _, output, ..
    } = last
    {
        *output = response.output;
    }
    let roundtrip = terminal(&decode_stream(encode_stream(events).await).await);
    assert_eq!(
        encode::encode_response(&roundtrip, "claude-test")["content"],
        json!(blocks)
    );
}

#[tokio::test]
async fn messages_buffered_reasoning_deletion_does_not_restore_signature() {
    let blocker: Node =
        serde_json::from_value(json!({"type":"text","role":"assistant","content":"first"}))
            .unwrap();
    let cleared: Node =
        serde_json::from_value(json!({"type":"reasoning","metadata":{"summary_as_thinking":true}}))
            .unwrap();
    let events = vec![
        UrpStreamEvent::ResponseStart {
            id: "msg_feature".into(),
            model: "claude-test".into(),
            usage: None,
            extra_body: HashMap::new(),
        },
        UrpStreamEvent::NodeStart {
            node_index: 0,
            header: super::NodeHeader::Text {
                id: None,
                role: super::OrdinaryRole::Assistant,
                phase: None,
                signature: None,
                citations: vec![],
            },
            extra_body: HashMap::new(),
        },
        UrpStreamEvent::NodeStart {
            node_index: 1,
            header: super::NodeHeader::Reasoning {
                id: None,
                metadata: Default::default(),
            },
            extra_body: HashMap::new(),
        },
        UrpStreamEvent::NodeDelta {
            node_index: 1,
            delta: super::NodeDelta::Reasoning {
                content: None,
                summary: Some("deleted thought".into()),
                encrypted: Some(json!("deleted signature")),
                source: None,
                metadata: Default::default(),
            },
            usage: None,
            extra_body: HashMap::new(),
        },
        UrpStreamEvent::NodeDone {
            node_index: 1,
            node: cleared.clone(),
            usage: None,
            extra_body: HashMap::new(),
        },
        UrpStreamEvent::NodeDone {
            node_index: 0,
            node: blocker.clone(),
            usage: None,
            extra_body: HashMap::new(),
        },
        UrpStreamEvent::ResponseDone {
            outcome: None,
            finish_reason: Some(super::FinishReason::Stop),
            usage: None,
            output: vec![blocker, cleared],
            extra_body: HashMap::new(),
        },
    ];
    let encoded = encode_stream(events).await;
    assert!(!serde_json::to_string(&encoded).unwrap().contains("deleted"));
    assert_eq!(
        encoded
            .iter()
            .filter(|frame| frame["type"] == "content_block_start")
            .count(),
        1
    );
}

#[tokio::test]
async fn messages_computer_member_name_does_not_collide_with_custom_tool() {
    let block = json!({"type":"tool_use","id":"toolu_member","name":"screenshot","toolset_name":"computer","input":{}});
    let events =
        decode_stream_with_custom_names(frames(&[block], "tool_use"), &["screenshot"]).await;
    let result = terminal(&events);
    assert!(
        matches!(&result.output[0], Node::ToolCall { namespace:Some(namespace),tool_type:super::ToolCallType::Function,..} if namespace=="computer")
    );
}

#[test]
fn messages_nonstream_error_is_not_a_successful_empty_response() {
    let error = json!({"type":"error","error":{"type":"overloaded_error","message":"Try later"},"request_id":"req_1"});
    assert_eq!(decode::decode_response(&error).unwrap_err(), "Try later");
}

#[test]
fn messages_native_thinking_field_deletions_do_not_trigger_defaults() {
    let source = json!({"model":"claude-test","max_tokens":4096,"thinking":{"type":"enabled","budget_tokens":1024,"display":"summarized"},"output_config":{"effort":"high"},"messages":[]});
    let mut urp = decode::decode_request(&source).unwrap();
    urp.reasoning.as_mut().unwrap().budget_tokens = None;
    let encoded = encode::encode_request(&urp, "claude-test");
    assert!(encoded["thinking"].get("budget_tokens").is_none());
    assert!(encode::encode_request_checked(&urp, "claude-test").is_err());
    let mut urp = decode::decode_request(&source).unwrap();
    let reasoning = urp.reasoning.as_mut().unwrap();
    reasoning.mode = None;
    reasoning.display = None;
    reasoning.effort = None;
    let encoded = encode::encode_request(&urp, "claude-test");
    assert!(encoded["thinking"].get("type").is_none());
    assert!(encoded["thinking"].get("display").is_none());
    assert!(encoded["output_config"].get("effort").is_none());
}

#[test]
fn messages_compatible_single_objects_and_ordered_arrays_reach_typed_urp() {
    for stream in [false, true] {
        for (block, mime, data) in [
            (
                json!({"type":"input_image","image_url":"data:image/png;base64,aGVsbG8=","detail":"high"}),
                "image/png",
                "aGVsbG8=",
            ),
            (
                json!({"type":"output_file","filename":"notes.json","file_data":"data:application/json;base64,eyJ4IjoxfQ=="}),
                "application/json",
                "eyJ4IjoxfQ==",
            ),
            (
                json!({"type":"input_audio","input_audio":{"format":"wav","data":"UklGRg=="}}),
                "audio/wav",
                "UklGRg==",
            ),
        ] {
            for singleton in [false, true] {
                let content = if singleton {
                    block.clone()
                } else {
                    json!(["Before", block, "After"])
                };
                let canonical =
                    decode::decode_request(&json!({"model":"claude-test","stream":stream,
                    "messages":[{"role":"user","content":content}]}))
                    .unwrap();
                let index = if singleton { 0 } else { 1 };
                assert_eq!(canonical.input.len(), if singleton { 1 } else { 3 });
                match &canonical.input[index] {
                    Node::Image {
                        source:
                            super::ImageSource::Base64 {
                                media_type,
                                data: bytes,
                            },
                        ..
                    }
                    | Node::File {
                        source:
                            super::FileSource::Base64 {
                                media_type,
                                data: bytes,
                            },
                        ..
                    }
                    | Node::Audio {
                        source:
                            super::AudioSource::Base64 {
                                media_type,
                                data: bytes,
                            },
                        ..
                    } => {
                        assert_eq!(media_type, mime);
                        assert_eq!(bytes, data);
                    }
                    node => panic!("compatible content was not typed media: {node:?}"),
                }
                if !singleton {
                    assert!(
                        matches!(&canonical.input[0],Node::Text { content, .. } if content == "Before")
                    );
                    assert!(
                        matches!(&canonical.input[2],Node::Text { content, .. } if content == "After")
                    );
                }
                let target =
                    super::encode::gemini::encode_request_checked(&canonical, "gemini-test")
                        .unwrap();
                let parts = target["contents"][0]["parts"].as_array().unwrap();
                assert!(
                    parts
                        .iter()
                        .any(|part| part["inlineData"]["mimeType"] == mime
                            && part["inlineData"]["data"] == data)
                );
                let restored = super::decode::gemini::decode_request(&target).unwrap();
                assert_eq!(restored.input.len(), canonical.input.len());
            }
        }
        let canonical = decode::decode_request(&json!({"model":"claude-test","stream":stream,
            "system":{"type":"input_image","image_url":"https://example.com/system.png"},
            "messages":[{"role":"user","content":"hello"}]}))
        .unwrap();
        assert!(matches!(
            &canonical.input[0],
            Node::Image {
                role: super::OrdinaryRole::System,
                ..
            }
        ));
        assert!(encode::encode_request_checked(&canonical, "claude-test").is_err());
    }
}

#[test]
fn messages_compatible_tool_result_media_aliases_preserve_bytes_and_audio_uris() {
    for stream in [false, true] {
        for (block, mime, data) in [
            (
                json!({"type":"image_url","image_url":{"url":"data:image/png;base64,aGVsbG8="}}),
                "image/png",
                "aGVsbG8=",
            ),
            (
                json!({"type":"input_file","file_data":"data:application/pdf;base64,JVBERi0xLjcK","filename":"result.pdf"}),
                "application/pdf",
                "JVBERi0xLjcK",
            ),
            (
                json!({"type":"output_audio","source":{"type":"base64","media_type":"audio/wav","data":"UklGRg=="}}),
                "audio/wav",
                "UklGRg==",
            ),
        ] {
            for singleton in [false, true] {
                let content = if singleton {
                    block.clone()
                } else {
                    json!(["Before", [block], "After"])
                };
                let canonical = decode::decode_request(&json!({"model":"claude-test","stream":stream,"messages":[
                    {"role":"assistant","content":[{"type":"tool_use","id":"toolu_read","name":"read","input":{}}]},
                    {"role":"user","content":{"type":"tool_result","tool_use_id":"toolu_read","content":content}}
                ]})).unwrap();
                let Node::ToolResult { content, .. } = &canonical.input[1] else {
                    panic!("typed result required")
                };
                let index = if singleton { 0 } else { 1 };
                assert_eq!(content.len(), if singleton { 1 } else { 3 });
                match &content[index] {
                    super::ToolResultContent::Image {
                        source:
                            super::ImageSource::Base64 {
                                media_type,
                                data: bytes,
                            },
                        ..
                    }
                    | super::ToolResultContent::File {
                        source:
                            super::FileSource::Base64 {
                                media_type,
                                data: bytes,
                            },
                        ..
                    } => {
                        assert_eq!(media_type, mime);
                        assert_eq!(bytes, data);
                    }
                    part => panic!("result media was not preserved: {part:?}"),
                }
                let target =
                    super::encode::gemini::encode_request_checked(&canonical, "gemini-test")
                        .unwrap();
                let function_response = &target["contents"][1]["parts"][0]["functionResponse"];
                assert_eq!(
                    function_response["parts"][0]["inlineData"]["mimeType"],
                    mime
                );
                assert_eq!(function_response["parts"][0]["inlineData"]["data"], data);
            }
        }
        let canonical = decode::decode_request(&json!({"model":"claude-test","stream":stream,"messages":[{"role":"user","content":{
            "type":"tool_result","tool_use_id":"toolu_uri","content":{"type":"audio","source":{"type":"url","url":"https://example.com/sound.wav","media_type":"audio/wav"}}
        }}]})).unwrap();
        let Node::ToolResult { content, .. } = &canonical.input[0] else {
            unreachable!()
        };
        assert!(
            matches!(&content[0],super::ToolResultContent::File { source:super::FileSource::Url { url }, metadata, .. } if url == "https://example.com/sound.wav" && metadata.media_type.as_deref() == Some("audio/wav"))
        );
        assert!(super::encode::gemini::encode_request_checked(&canonical, "gemini-test").is_err());
    }
}

#[tokio::test]
async fn messages_compatible_tool_result_response_preserves_native_and_typed_contents() {
    let block = json!({"type":"tool_result","tool_use_id":"toolu_read","content":[
        {"type":"input_image","image_url":"data:image/png;base64,aGVsbG8="},
        {"type":"output_audio","source":{"type":"base64","media_type":"audio/wav","data":"UklGRg=="}},
        {"type":"browser_state","tabs":[{"id":"tab_1","active":true,"url":"https://example.com"}],"future_payload":{"type":"image","source":{"type":"unknown"}}}
    ]});
    let canonical = decode::decode_response(&response(vec![block.clone()])).unwrap();
    let Node::ToolResult { content, .. } = &canonical.output[0] else {
        panic!("tool_result must be typed")
    };
    assert!(matches!(
        &content[0],
        super::ToolResultContent::Image { .. }
    ));
    assert!(
        matches!(&content[1],super::ToolResultContent::File { source:super::FileSource::Base64 { media_type, .. }, .. } if media_type == "audio/wav")
    );
    assert!(
        matches!(&content[2],super::ToolResultContent::ProviderItem { item_type, body, .. } if item_type == "browser_state" && body["future_payload"]["source"]["type"] == "unknown")
    );
    let events = decode_stream(frames(&[block], "end_turn")).await;
    assert_eq!(
        serde_json::to_value(terminal(&events).output).unwrap(),
        serde_json::to_value(&canonical.output).unwrap()
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, UrpStreamEvent::Error { .. }))
    );
}

#[tokio::test]
async fn messages_media_like_tool_arguments_and_native_tool_payloads_remain_opaque() {
    let blocks = vec![
        json!({"type":"tool_use","id":"toolu_json","name":"inspect","input":{"type":"image","source":{"type":"unknown"},"nested":{"type":"input_audio"}}}),
        json!({"type":"server_tool_use","id":"srvtoolu_json","name":"web_fetch","input":{"type":"document","source":{"type":"unknown"}}}),
        json!({"type":"web_fetch_tool_result","tool_use_id":"srvtoolu_json","content":{"type":"web_fetch_result","content":{"type":"document","source":{"type":"text","data":"Native body"}}}}),
    ];
    let canonical = decode::decode_response(&response(blocks.clone())).unwrap();
    assert!(
        matches!(&canonical.output[0], Node::ToolCall { arguments, .. } if serde_json::from_str::<Value>(arguments).unwrap() == blocks[0]["input"])
    );
    assert!(matches!(&canonical.output[1], Node::ProviderItem { body, .. } if body == &blocks[1]));
    assert!(matches!(&canonical.output[2], Node::ProviderItem { body, .. } if body == &blocks[2]));
    assert_block_roundtrip(blocks).await;
}

#[tokio::test]
async fn messages_compatible_media_actual_sse_reaches_gemini_wire_once() {
    let blocks = vec![
        json!({"type":"input_image","image_url":"data:image/png;base64,aGVsbG8="}),
        json!({"type":"input_file","file_data":"data:application/pdf;base64,JVBERi0xLjcK"}),
        json!({"type":"input_audio","input_audio":{"format":"wav","data":"UklGRg=="}}),
    ];
    let canonical = decode::decode_response(&response(blocks.clone())).unwrap();
    let nonstream =
        super::encode::gemini::encode_response_checked(&canonical, "gemini-test").unwrap();
    let expected = nonstream["candidates"][0]["content"]["parts"]
        .as_array()
        .unwrap();
    assert_eq!(expected.len(), 3);
    for (part, mime, data) in [
        (&expected[0], "image/png", "aGVsbG8="),
        (&expected[1], "application/pdf", "JVBERi0xLjcK"),
        (&expected[2], "audio/wav", "UklGRg=="),
    ] {
        assert_eq!(part["inlineData"]["mimeType"], mime);
        assert_eq!(part["inlineData"]["data"], data);
    }
    let events = decode_stream(frames(&blocks, "end_turn")).await;
    let (source_tx, source_rx) = mpsc::channel(1024);
    for event in events {
        source_tx.send(event).await.unwrap();
    }
    drop(source_tx);
    let (tx, rx) = mpsc::channel(1024);
    crate::urp::stream_encode::gemini::encode_urp_stream_as_gemini(source_rx, tx, "gemini-test")
        .await
        .unwrap();
    let wire = sse_json(rx).await;
    let parts: Vec<Value> = wire
        .iter()
        .flat_map(|frame| {
            frame["candidates"][0]["content"]["parts"]
                .as_array()
                .into_iter()
                .flatten()
                .cloned()
        })
        .collect();
    assert_eq!(&parts, expected);
    assert_eq!(
        wire.last().unwrap()["candidates"][0]["finishReason"],
        "STOP"
    );
    assert!(!wire.iter().any(|frame| frame.get("error").is_some()));
}

#[test]
fn messages_compatible_binary_files_are_retained_until_target_validation() {
    for stream in [false, true] {
        for nested in [false, true] {
            let file = json!({"type":"document","title":"Archive","source":{
                "type":"base64","media_type":"application/zip","data":"UEsDBAo="
            }});
            let content = if nested {
                json!({"type":"tool_result","tool_use_id":"toolu_archive","content":file})
            } else {
                file
            };
            let canonical = decode::decode_request(&json!({"model":"claude-test","stream":stream,
                "messages":[{"role":"user","content":content}]}))
            .unwrap();
            let source = match &canonical.input[0] {
                Node::File { source, .. } => source,
                Node::ToolResult { content, .. } => match &content[0] {
                    super::ToolResultContent::File { source, .. } => source,
                    _ => panic!("archive must remain typed file content"),
                },
                _ => panic!("archive must remain a typed file"),
            };
            assert_eq!(
                source,
                &super::FileSource::Base64 {
                    media_type: "application/zip".into(),
                    data: "UEsDBAo=".into(),
                }
            );
            assert!(encode::encode_request_checked(&canonical, "claude-test").is_err());
            assert!(
                super::encode::gemini::encode_request_checked(&canonical, "gemini-test").is_err()
            );
        }
    }
}

fn assert_messages_reasoning_surfaces(nodes: &[Node]) {
    let Node::Reasoning {
        content,
        summary,
        encrypted,
        metadata,
        extra_body,
        ..
    } = &nodes[0]
    else {
        panic!("expected thinking node");
    };
    assert_eq!(
        content, &None,
        "Messages thinking is a public summary, never raw CoT"
    );
    assert_eq!(summary.as_deref(), Some("Public summary. 第二段。"));
    assert_eq!(encrypted, &Some(json!("encrypted-full-thinking")));
    assert!(metadata.summary_as_thinking);
    assert!(!metadata.redacted);
    assert!(!extra_body.contains_key("thinking"));
    assert!(!extra_body.contains_key("signature"));
    let Node::Reasoning {
        content,
        summary,
        encrypted,
        metadata,
        ..
    } = &nodes[1]
    else {
        panic!("expected redacted thinking node");
    };
    assert_eq!(content, &None);
    assert_eq!(summary, &None);
    assert_eq!(encrypted, &Some(json!("opaque-redacted-data")));
    assert!(metadata.redacted);
}

#[tokio::test]
async fn messages_documented_thinking_summary_and_encrypted_data_never_become_raw_cot() {
    let blocks = vec![
        json!({"type":"thinking","thinking":"Public summary. 第二段。","signature":"encrypted-full-thinking"}),
        json!({"type":"redacted_thinking","data":"opaque-redacted-data"}),
    ];
    let canonical = decode::decode_response(&response(blocks.clone())).unwrap();
    assert_messages_reasoning_surfaces(&canonical.output);
    for stream in [false, true] {
        let native = json!({"model":"claude-test","max_tokens":4096,"stream":stream,
            "messages":[{"role":"assistant","content":blocks}]});
        let request = decode::decode_request(&native).unwrap();
        assert_messages_reasoning_surfaces(&request.input);
        let replay = encode::encode_request_checked(&request, "claude-test").unwrap();
        assert_eq!(replay["messages"], native["messages"]);
        assert_messages_reasoning_surfaces(&decode::decode_request(&replay).unwrap().input);
    }
    let mut actual = frames(&blocks, "end_turn");
    let signature = actual
        .iter()
        .position(|frame| frame["delta"]["type"] == "signature_delta")
        .unwrap();
    actual[signature]["delta"]["signature"] = json!("encrypted-");
    actual.insert(signature + 1, json!({"type":"content_block_delta","index":4,"delta":{"type":"signature_delta","signature":"full-thinking"}}));
    let events = decode_stream(actual).await;
    assert_messages_reasoning_surfaces(&terminal(&events).output);
    assert!(events.iter().all(|event| !matches!(
        event,
        UrpStreamEvent::NodeDelta {
            delta: super::NodeDelta::Reasoning {
                content: Some(_),
                ..
            },
            ..
        }
    )));
    for wire in [
        encode_stream(events).await,
        synthetic_stream(&canonical).await,
    ] {
        assert_messages_reasoning_surfaces(&terminal(&decode_stream(wire).await).output);
    }
}

#[test]
fn messages_thinking_controls_roundtrip_enabled_adaptive_disabled_and_omitted() {
    for stream in [false, true] {
        for thinking in [
            None,
            Some(json!({"type":"enabled","budget_tokens":1024,"display":"summarized"})),
            Some(json!({"type":"adaptive","display":"omitted"})),
            Some(json!({"type":"disabled"})),
        ] {
            let mut native = json!({"model":"claude-sonnet-4-6","stream":stream,"max_tokens":4096,
                "messages":[{"role":"user","content":"Hello"}]});
            if let Some(thinking) = &thinking {
                native["thinking"] = thinking.clone();
            }
            let canonical = decode::decode_request(&native).unwrap();
            let replay = encode::encode_request_checked(&canonical, "claude-sonnet-4-6").unwrap();
            assert_eq!(replay.get("thinking"), thinking.as_ref());
            let restored = decode::decode_request(&replay).unwrap();
            assert_eq!(
                serde_json::to_value(canonical.reasoning).unwrap(),
                serde_json::to_value(restored.reasoning).unwrap()
            );
        }
    }
}

#[tokio::test]
async fn messages_all_document_citation_ranges_are_source_ranges_and_roundtrip() {
    let citations = json!([
        {"type":"char_location","document_index":2,"document_title":"中文","start_char_index":3,"end_char_index":7,"cited_text":"引用"},
        {"type":"page_location","document_index":1,"document_title":"PDF","start_page_number":1,"end_page_number":3,"cited_text":"pages"},
        {"type":"content_block_location","document_index":0,"document_title":"Blocks","start_block_index":0,"end_block_index":2,"cited_text":"blocks"},
        {"type":"future_citation","opaque":{"nested":[1,2]}}
    ]);
    let blocks = vec![json!({"type":"text","text":"answer","citations":citations})];
    let canonical = decode::decode_response(&response(blocks.clone())).unwrap();
    let Node::Text { citations, .. } = &canonical.output[0] else {
        panic!()
    };
    assert!(
        citations
            .iter()
            .all(|citation| citation.answer_range.is_none())
    );
    assert_block_roundtrip(blocks).await;
}

#[tokio::test]
async fn messages_compaction_delta_and_iterations_roundtrip_without_double_billing() {
    let native_usage = json!({"input_tokens":10,"output_tokens":4,"cache_read_input_tokens":2,
        "iterations":[{"type":"compaction","input_tokens":100,"output_tokens":20,"cache_creation_input_tokens":5},
        {"type":"message","input_tokens":10,"output_tokens":4,"cache_read_input_tokens":2}]});
    let mut native = response(vec![
        json!({"type":"compaction","content":"Complete context summary."}),
    ]);
    native["stop_reason"] = json!("compaction");
    native["usage"] = native_usage.clone();
    let canonical = decode::decode_response(&native).unwrap();
    let mut actual = frames(&[], "compaction");
    actual.splice(1..1, [
        json!({"type":"content_block_start","index":0,"content_block":{"type":"compaction","content":null}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"compaction_delta","content":"Complete context summary."}}),
        json!({"type":"content_block_stop","index":0}),
    ]);
    actual
        .iter_mut()
        .find(|frame| frame["type"] == "message_delta")
        .unwrap()["usage"] = native_usage;
    let events = decode_stream(actual).await;
    let streamed = terminal(&events);
    let live = terminal(&decode_stream(encode_stream(events).await).await);
    let synthetic = terminal(&decode_stream(synthetic_stream(&canonical).await).await);
    for response in [&canonical, &streamed, &live, &synthetic] {
        assert_eq!(
            response.finish_reason,
            Some(super::FinishReason::Compaction)
        );
        let usage = response.usage.as_ref().unwrap();
        assert_eq!((usage.input_tokens, usage.output_tokens), (12, 4));
        assert_eq!(
            (
                usage.accounting().input_tokens,
                usage.accounting().output_tokens
            ),
            (117, 24)
        );
        assert!(!usage.extra_body.contains_key("iterations"));
        let wire = encode::encode_response(response, "claude-test");
        assert_eq!(wire["content"], native["content"]);
        assert_eq!(wire["usage"]["iterations"][0]["input_tokens"], 100);
        assert_eq!(wire["usage"]["input_tokens"], 10);
        assert_eq!(wire["usage"]["output_tokens"], 4);
    }
}

#[tokio::test]
async fn messages_compaction_snapshot_and_unknown_blocks_preserve_opaque_content() {
    assert_block_roundtrip(vec![
        json!({"type":"compaction","content":"Whole summary","signature":"opaque-summary-signature"}),
        json!({"type":"future_block","payload":{"thinking":"not reasoning","encrypted_content":"opaque"}}),
    ]).await;
}

#[tokio::test]
async fn messages_ping_unknown_events_and_cumulative_usage_do_not_duplicate_output() {
    let mut wire = frames(&[json!({"type":"text","text":"Hello"})], "end_turn");
    wire.splice(
        1..1,
        [
            json!({"type":"ping"}),
            json!({"type":"future_event","payload":7}),
            json!({"type":"message_delta","delta":{},"usage":{"output_tokens":2}}),
            json!({"type":"message_delta","delta":{},"usage":{"output_tokens":5}}),
        ],
    );
    let events = decode_stream(wire).await;
    assert_eq!(terminal(&events).usage.unwrap().output_tokens, 7);
    assert_eq!(events.iter().filter(|event| matches!(event, UrpStreamEvent::ProviderControl {event_name,..} if event_name=="ping")).count(), 1);
    assert_eq!(
        encode::encode_response(&terminal(&events), "claude-test")["content"],
        json!([{"type":"text","text":"Hello"}])
    );
    let restored = terminal(&decode_stream(encode_stream(events).await).await);
    assert_eq!(restored.usage.unwrap().output_tokens, 7);
}

#[tokio::test]
async fn messages_invalid_json_and_disconnect_are_errors_without_success_terminal() {
    let start = frames(&[], "end_turn")[0].clone();
    let malformed = format!(
        "event: message_start\ndata: {start}\n\nevent: content_block_delta\ndata: {{bad\n\n"
    );
    let errors = decode_raw_stream(malformed, &[]).await;
    assert!(errors.iter().any(|event| matches!(event, UrpStreamEvent::Error {code:Some(code),..} if code=="messages_invalid_sse_json")));
    let mut truncated = frames(&[json!({"type":"text","text":"partial"})], "end_turn");
    truncated.truncate(3);
    let disconnect = decode_stream(truncated).await;
    assert!(disconnect.iter().any(|event| matches!(event, UrpStreamEvent::Error {code:Some(code),..} if code=="upstream_stream_missing_terminal")));
    for events in [errors, disconnect] {
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, UrpStreamEvent::ResponseDone { .. }))
        );
        let wire = encode_stream(events).await;
        assert_eq!(
            wire.iter().filter(|frame| frame["type"] == "error").count(),
            1
        );
        assert!(!wire.iter().any(|frame| frame["type"] == "message_stop"));
    }
}

#[tokio::test]
async fn messages_stream_lifecycle_invalid_indices_and_reuse_fail_explicitly() {
    let start = frames(&[], "end_turn")[0].clone();
    let block =
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}});
    let delta =
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"x"}});
    let stop = json!({"type":"content_block_stop","index":0});
    for invalid in [
        vec![delta.clone()],
        vec![stop.clone()],
        vec![block.clone(), block.clone()],
        vec![block.clone(), stop.clone(), delta],
        vec![block.clone(), stop.clone(), stop],
        vec![block],
    ] {
        let mut wire = vec![start.clone()];
        wire.extend(invalid);
        wire.push(json!({"type":"message_stop"}));
        let events = decode_stream(wire).await;
        assert!(
            events
                .iter()
                .any(|event| matches!(event, UrpStreamEvent::Error { .. })),
            "{events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, UrpStreamEvent::ResponseDone { .. }))
        );
    }
}

#[test]
fn messages_malformed_request_envelopes_and_provider_error_are_not_empty_success() {
    for native in [
        json!(null),
        json!([]),
        json!({"messages":[]}),
        json!({"model":"claude"}),
        json!({"model":"claude","messages":{}}),
        json!({"model":"claude","messages":[42]}),
    ] {
        assert!(decode::decode_request(&native).is_err(), "{native}");
    }
    for error_type in [
        "invalid_request_error",
        "authentication_error",
        "permission_error",
        "not_found_error",
        "request_too_large",
        "rate_limit_error",
        "api_error",
        "overloaded_error",
    ] {
        let native = json!({"type":"error","error":{"type":error_type,"message":"Rejected"}});
        assert_eq!(decode::decode_response(&native).unwrap_err(), "Rejected");
    }
}

#[tokio::test]
async fn messages_native_tool_error_results_are_data_not_terminal_errors() {
    assert_block_roundtrip(vec![
        json!({"type":"web_search_tool_result","tool_use_id":"srvtoolu_search","content":{"type":"web_search_tool_result_error","error_code":"max_uses_exceeded"}}),
        json!({"type":"code_execution_tool_result","tool_use_id":"srvtoolu_code","content":{"type":"code_execution_tool_result_error","error_code":"execution_time_exceeded"}}),
    ]).await;
    for stream in [false, true] {
        let native = json!({"model":"claude-test","stream":stream,"messages":[{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"toolu_client","is_error":true,"content":[{"type":"text","text":"Could not connect"}]}]}]});
        let canonical = decode::decode_request(&native).unwrap();
        assert!(matches!(
            &canonical.input[0],
            Node::ToolResult { is_error: true, .. }
        ));
        let replay = encode::encode_request_checked(&canonical, "claude-test").unwrap();
        assert_eq!(replay["messages"], native["messages"]);
        assert!(matches!(
            &decode::decode_request(&replay).unwrap().input[0],
            Node::ToolResult { is_error: true, .. }
        ));
    }
}

#[tokio::test]
async fn messages_tool_json_validation_distinguishes_truncation_from_success() {
    for partial in ["{\"x\":", "[1,2]", "42", "null", "{\"x\":1}garbage"] {
        for reason in ["tool_use", "end_turn", "max_tokens"] {
            let mut wire = frames(&[], reason);
            wire.splice(1..1, [
                json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu","name":"f","input":{}}}),
                json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":partial}}),
                json!({"type":"content_block_stop","index":0}),
            ]);
            let events = decode_stream(wire).await;
            if reason == "max_tokens" && partial == "{\"x\":" {
                let canonical = terminal(&events);
                assert_eq!(canonical.finish_reason, Some(super::FinishReason::Length));
                assert!(
                    matches!(&canonical.output[0],Node::ToolCall {arguments,..} if arguments==partial)
                );
            } else {
                assert!(events.iter().any(|event| matches!(event,UrpStreamEvent::Error {code:Some(code),..} if code=="messages_tool_input_invalid")), "{events:?}");
                assert!(
                    !events
                        .iter()
                        .any(|event| matches!(event, UrpStreamEvent::ResponseDone { .. }))
                );
            }
        }
    }
}

#[tokio::test]
async fn messages_first_terminal_stops_late_events_and_preserves_partial_error_output() {
    let mut successful = frames(&[json!({"type":"text","text":"first"})], "end_turn");
    successful.push(json!({"type":"error","error":{"type":"api_error","message":"too late"}}));
    successful.extend(frames(
        &[json!({"type":"text","text":"second"})],
        "end_turn",
    ));
    let events = decode_stream(successful).await;
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, UrpStreamEvent::ResponseDone { .. }))
            .count(),
        1
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, UrpStreamEvent::Error { .. }))
    );
    assert_eq!(
        encode::encode_response(&terminal(&events), "claude-test")["content"],
        json!([{"type":"text","text":"first"}])
    );
    let mut failed = frames(&[json!({"type":"text","text":"partial"})], "end_turn");
    failed.truncate(3);
    failed.push(json!({"type":"error","error":{"type":"overloaded_error","message":"retry"}}));
    failed.push(json!({"type":"message_stop"}));
    let events = decode_stream(failed).await;
    assert!(events.iter().any(|event| matches!(event,UrpStreamEvent::NodeDelta {delta:super::NodeDelta::Text {content,..},..} if content=="partial")));
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
}

#[tokio::test]
async fn messages_truncated_tool_json_is_not_fabricated_as_raw_object() {
    let mut canonical = decode::decode_response(&response(vec![
        json!({"type":"tool_use","id":"toolu","name":"f","input":{}}),
    ]))
    .unwrap();
    canonical.finish_reason = Some(super::FinishReason::Length);
    if let Node::ToolCall { arguments, .. } = &mut canonical.output[0] {
        *arguments = "{\"x\":".into();
    }
    assert!(encode::encode_response_checked(&canonical, "claude-test").is_err());
    let error = encode::encode_response(&canonical, "claude-test");
    assert_eq!(error["type"], "error");
    assert!(!error.to_string().contains("_raw"));
    let (tx, rx) = mpsc::channel(1024);
    let result = crate::urp::stream_encode::anthropic::emit_synthetic_messages_stream(
        "claude-test",
        &canonical,
        None,
        tx,
    )
    .await;
    assert!(result.is_err());
    let wire = sse_json(rx).await;
    assert_eq!(wire.len(), 1);
    assert_eq!(wire[0]["type"], "error");
    for stream in [false, true] {
        let mut request =
            decode::decode_request(&json!({"model":"claude-test","stream":stream,"messages":[]}))
                .unwrap();
        request.input = canonical.output.clone();
        assert!(encode::encode_request_checked(&request, "claude-test").is_err());
    }
}

#[test]
fn messages_invalid_thinking_combinations_are_rejected_before_dispatch() {
    for stream in [false, true] {
        for patch in [
            json!({"thinking":{"type":"enabled","budget_tokens":1023}}),
            json!({"thinking":{"type":"enabled","budget_tokens":4096}}),
            json!({"thinking":{"type":"enabled"}}),
            json!({"thinking":{"type":"disabled","budget_tokens":1024}}),
            json!({"thinking":{"type":"disabled","display":"summarized"}}),
            json!({"thinking":{"type":"adaptive","budget_tokens":1024}}),
            json!({"thinking":{"type":"adaptive","display":"raw"}}),
            json!({"thinking":{"type":"adaptive"},"tool_choice":{"type":"any"}}),
            json!({"thinking":{"type":"adaptive"},"temperature":0.5}),
            json!({"thinking":{"type":"adaptive"},"top_k":5}),
            json!({"thinking":{"type":"adaptive"},"top_p":0.5}),
        ] {
            let mut native = json!({"model":"claude-sonnet-4-6","max_tokens":4096,"stream":stream,
                "messages":[{"role":"user","content":"Hello"}]});
            native
                .as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            let canonical = decode::decode_request(&native).unwrap();
            assert!(
                encode::encode_request_checked(&canonical, "claude-sonnet-4-6").is_err(),
                "{native}"
            );
        }
    }
}

#[tokio::test]
async fn messages_reasoning_typed_mutations_and_deletions_win_in_every_output_mode() {
    let mut canonical = decode::decode_response(&response(vec![
        json!({"type":"thinking","thinking":"old summary","signature":"old cipher"}),
        json!({"type":"redacted_thinking","data":"old redacted"}),
    ]))
    .unwrap();
    if let Node::Reasoning {
        summary,
        encrypted,
        extra_body,
        ..
    } = &mut canonical.output[0]
    {
        *summary = Some("new summary".into());
        *encrypted = Some(json!("new cipher"));
        extra_body.insert("thinking".into(), json!("stale summary"));
        extra_body.insert("signature".into(), json!("stale cipher"));
    }
    if let Node::Reasoning { encrypted, .. } = &mut canonical.output[1] {
        *encrypted = None;
    }
    let expected = json!([{"type":"thinking","thinking":"new summary","signature":"new cipher"}]);
    assert_eq!(
        encode::encode_response(&canonical, "claude-test")["content"],
        expected
    );
    let synthetic = synthetic_stream(&canonical).await;
    let restored = terminal(&decode_stream(synthetic.clone()).await);
    assert_eq!(
        encode::encode_response(&restored, "claude-test")["content"],
        expected
    );
    let events = decode_stream(synthetic).await;
    let live = terminal(&decode_stream(encode_stream(events).await).await);
    assert_eq!(
        encode::encode_response(&live, "claude-test")["content"],
        expected
    );
}

#[tokio::test]
async fn messages_known_malformed_or_mismatched_deltas_are_errors() {
    for (block, delta) in [
        (
            json!({"type":"text","text":""}),
            json!({"type":"text_delta","text":42}),
        ),
        (
            json!({"type":"text","text":""}),
            json!({"type":"thinking_delta","thinking":"wrong channel"}),
        ),
        (
            json!({"type":"thinking","thinking":"","signature":""}),
            json!({"type":"text_delta","text":"wrong channel"}),
        ),
        (
            json!({"type":"thinking","thinking":"","signature":""}),
            json!({"type":"signature_delta","signature":{"opaque":true}}),
        ),
        (
            json!({"type":"text","text":""}),
            json!({"type":"citations_delta","citation":[]}),
        ),
        (
            json!({"type":"tool_use","id":"toolu","name":"f","input":{}}),
            json!({"type":"input_json_delta","partial_json":{}}),
        ),
    ] {
        let mut wire = frames(&[], "end_turn");
        wire.splice(
            1..1,
            [
                json!({"type":"content_block_start","index":0,"content_block":block}),
                json!({"type":"content_block_delta","index":0,"delta":delta}),
                json!({"type":"content_block_stop","index":0}),
            ],
        );
        let events = decode_stream(wire).await;
        assert!(
            events
                .iter()
                .any(|event| matches!(event, UrpStreamEvent::Error { .. })),
            "{events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, UrpStreamEvent::ResponseDone { .. }))
        );
    }
}

#[tokio::test]
async fn messages_message_lifecycle_requires_one_start_before_blocks_or_terminal() {
    let start = frames(&[], "end_turn")[0].clone();
    for wire in [
        vec![json!({"type":"message_stop"})],
        vec![json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}})],
        vec![
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"x"}}),
        ],
        vec![start.clone(), start, json!({"type":"message_stop"})],
    ] {
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
    }
}

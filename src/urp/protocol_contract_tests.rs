use super::*;
use axum::response::{IntoResponse, Sse, sse::Event};
use serde_json::json;
use std::convert::Infallible;
use tokio::sync::mpsc;

fn reasoning(content: Option<&str>, summary: Option<&str>, encrypted: Option<Value>) -> Node {
    Node::Reasoning {
        id: Some("rs_contract".into()),
        content: content.map(str::to_owned),
        summary: summary.map(str::to_owned),
        encrypted,
        source: None,
        metadata: Default::default(),
        extra_body: HashMap::new(),
    }
}

fn response(nodes: Vec<Node>) -> UrpResponse {
    UrpResponse {
        outcome: None,
        id: "resp_contract".into(),
        model: "model-contract".into(),
        created_at: Some(1),
        output: nodes,
        finish_reason: Some(FinishReason::Stop),
        usage: None,
        extra_body: HashMap::new(),
    }
}

fn reasoning_values(nodes: &[Node]) -> (String, String, Vec<Value>) {
    let mut result = (String::new(), String::new(), Vec::new());
    for node in nodes {
        if let Node::Reasoning {
            content,
            summary,
            encrypted,
            ..
        } = node
        {
            result.0.push_str(content.as_deref().unwrap_or_default());
            result.1.push_str(summary.as_deref().unwrap_or_default());
            if let Some(value) = encrypted {
                result.2.push(value.clone());
            }
        }
    }
    result
}

fn replay_request(protocol: ProviderProtocol, wire: Value, stream: bool) -> UrpRequest {
    match protocol {
        ProviderProtocol::Responses => decode::openai_responses::decode_request(&json!({
            "model":"model-contract", "stream":stream, "input":wire["output"]
        }))
        .unwrap(),
        ProviderProtocol::ChatCompletion => decode::openai_chat::decode_request(&json!({
            "model":"model-contract", "stream":stream, "messages":[wire["choices"][0]["message"]]
        }))
        .unwrap(),
        ProviderProtocol::Messages => decode::anthropic::decode_request(&json!({
            "model":"model-contract", "stream":stream, "max_tokens":4096,
            "messages":[{"role":"assistant","content":wire["content"]}]
        }))
        .unwrap(),
        _ => unreachable!(),
    }
}

fn encode_response(protocol: ProviderProtocol, response: &UrpResponse) -> Value {
    match protocol {
        ProviderProtocol::Responses => {
            encode::openai_responses::encode_response(response, "model-contract")
        }
        ProviderProtocol::ChatCompletion => {
            encode::openai_chat::encode_response(response, "model-contract")
        }
        ProviderProtocol::Messages => {
            encode::anthropic::encode_response(response, "model-contract")
        }
        _ => unreachable!(),
    }
}

fn protocols() -> [ProviderProtocol; 3] {
    [
        ProviderProtocol::Responses,
        ProviderProtocol::ChatCompletion,
        ProviderProtocol::Messages,
    ]
}

async fn collect_wire(rx: mpsc::Receiver<Event>) -> String {
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    let body = Sse::new(futures_util::StreamExt::map(stream, Ok::<_, Infallible>))
        .into_response()
        .into_body();
    String::from_utf8(
        axum::body::to_bytes(body, usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

async fn synthetic_decode(
    protocol: ProviderProtocol,
    canonical: &UrpResponse,
) -> Vec<UrpStreamEvent> {
    let (tx, rx) = mpsc::channel(4096);
    match protocol {
        ProviderProtocol::Responses => {
            stream_encode::openai_responses::emit_synthetic_responses_stream(
                "model-contract",
                canonical,
                None,
                Some(600),
                tx,
            )
            .await
            .unwrap()
        }
        ProviderProtocol::ChatCompletion => stream_encode::openai_chat::emit_synthetic_chat_stream(
            "model-contract",
            canonical,
            Some(600),
            tx,
        )
        .await
        .unwrap(),
        ProviderProtocol::Messages => stream_encode::anthropic::emit_synthetic_messages_stream(
            "model-contract",
            canonical,
            Some(600),
            tx,
        )
        .await
        .unwrap(),
        _ => unreachable!(),
    }
    let wire = collect_wire(rx).await;
    let upstream = reqwest::Response::from(
        axum::http::Response::builder()
            .header("content-type", "text/event-stream")
            .body(wire)
            .unwrap(),
    );
    let request = crate::handlers::UrpRequest {
        model: "model-contract".into(),
        max_multiplier: None,
        audio_output_format: None,
        server_tool_usage_classes: Vec::new(),
        messages_custom_tool_names: Default::default(),
        affinity_explicit: None,
        affinity_prefix_hash: String::new(),
    };
    let (tx, mut rx) = mpsc::channel(4096);
    match protocol {
        ProviderProtocol::Responses => {
            stream_decode::openai_responses::stream_responses_to_urp_events(
                &request, None, upstream, tx, None, None, 1000,
            )
            .await
            .unwrap()
        }
        ProviderProtocol::ChatCompletion => stream_decode::openai_chat::stream_chat_to_urp_events(
            &request, upstream, tx, None, None, 1000,
        )
        .await
        .unwrap(),
        ProviderProtocol::Messages => stream_decode::anthropic::stream_messages_to_urp_events(
            &request, upstream, tx, None, None, 1000,
        )
        .await
        .unwrap(),
        _ => unreachable!(),
    }
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    events
}

#[test]
fn opaque_reasoning_client_replay_preserves_payload_and_provider_binding() {
    for origin in protocols() {
        for downstream in protocols() {
            for opaque in [
                json!("opaque.sign_+/=漢字"),
                json!({"ciphertext":[0,255],"version":3}),
            ] {
                let mut original = response(vec![reasoning(
                    None,
                    Some("summary-only"),
                    Some(opaque.clone()),
                )]);
                wrap_reasoning_envelopes_in_response(
                    &mut original,
                    origin.as_str(),
                    "origin-model",
                );
                let once = serde_json::to_value(&original).unwrap();
                wrap_reasoning_envelopes_in_response(
                    &mut original,
                    origin.as_str(),
                    "origin-model",
                );
                assert_eq!(serde_json::to_value(&original).unwrap(), once);
                for stream in [false, true] {
                    let mut replay =
                        replay_request(downstream, encode_response(downstream, &original), stream);
                    let before = reasoning_values(&replay.input);
                    assert_eq!(before.0, "", "{origin:?} via {downstream:?}");
                    assert_eq!(before.1, "summary-only");
                    assert_eq!(before.2.len(), 1);
                    assert_eq!(
                        parse_reasoning_envelope(&before.2[0]).unwrap().payload,
                        opaque
                    );
                    let mut wrong = replay.input.clone();
                    filter_and_unwrap_reasoning_envelopes_for_upstream(
                        &mut wrong,
                        origin.as_str(),
                        "wrong-model",
                        true,
                    );
                    assert!(
                        reasoning_values(&wrong).2.is_empty(),
                        "ciphertext survived a model mismatch: {origin:?} via {downstream:?}"
                    );
                    filter_and_unwrap_reasoning_envelopes_for_upstream(
                        &mut replay.input,
                        origin.as_str(),
                        "origin-model",
                        true,
                    );
                    assert_eq!(
                        reasoning_values(&replay.input),
                        (String::new(), "summary-only".into(), vec![opaque.clone()])
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn opaque_reasoning_synthetic_sse_preserves_summary_and_exact_envelope() {
    for origin in protocols() {
        for downstream in protocols() {
            let opaque = json!({"opaque":"ciphertext-not-readable","bytes":[1,2,3]});
            let mut original = response(vec![reasoning(
                None,
                Some("visible summary"),
                Some(opaque.clone()),
            )]);
            wrap_reasoning_envelopes_in_response(&mut original, origin.as_str(), "origin-model");
            let events = synthetic_decode(downstream, &original).await;
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, UrpStreamEvent::Error { .. })),
                "{events:?}"
            );
            let terminals: Vec<_> = events
                .iter()
                .filter_map(|event| match event {
                    UrpStreamEvent::ResponseDone { output, .. } => Some(output),
                    _ => None,
                })
                .collect();
            assert_eq!(terminals.len(), 1, "{origin:?} via {downstream:?}");
            let (raw, summary, encrypted) = reasoning_values(terminals[0]);
            assert!(raw.is_empty(), "ciphertext or summary became raw: {raw}");
            assert_eq!(summary, "visible summary");
            assert_eq!(encrypted.len(), 1);
            let envelope = parse_reasoning_envelope(&encrypted[0]).unwrap();
            assert_eq!(envelope.payload, opaque);
            assert_eq!(envelope.provider_type, origin.as_str());
        }
    }
}

#[tokio::test]
async fn raw_and_summary_remain_independent_on_chat_and_responses_sse() {
    for protocol in [
        ProviderProtocol::ChatCompletion,
        ProviderProtocol::Responses,
    ] {
        for (raw, summary, encrypted) in [
            (Some("RAW 原文"), None, None),
            (None, Some("SUMMARY 摘要"), Some(json!("opaque"))),
            (
                Some("RAW 原文"),
                Some("SUMMARY 摘要"),
                Some(json!("opaque")),
            ),
        ] {
            let canonical = response(vec![reasoning(raw, summary, encrypted.clone())]);
            let events = synthetic_decode(protocol, &canonical).await;
            let output = events
                .iter()
                .find_map(|event| match event {
                    UrpStreamEvent::ResponseDone { output, .. } => Some(output),
                    _ => None,
                })
                .unwrap();
            assert_eq!(
                reasoning_values(output),
                (
                    raw.unwrap_or_default().into(),
                    summary.unwrap_or_default().into(),
                    encrypted.into_iter().collect()
                ),
                "{protocol:?}"
            );
            for stream in [false, true] {
                let replay =
                    replay_request(protocol, encode_response(protocol, &canonical), stream);
                assert_eq!(
                    reasoning_values(&replay.input),
                    reasoning_values(&canonical.output)
                );
            }
        }
    }
}

#[test]
fn reasoning_field_deletions_survive_serialization_and_native_replay() {
    for protocol in [
        ProviderProtocol::ChatCompletion,
        ProviderProtocol::Responses,
    ] {
        for deleted in ["content", "summary", "encrypted"] {
            let canonical = response(vec![reasoning(
                Some("RAW"),
                Some("SUMMARY"),
                Some(json!("CIPHER")),
            )]);
            let mut replay = replay_request(protocol, encode_response(protocol, &canonical), false);
            for node in &mut replay.input {
                if let Node::Reasoning {
                    content,
                    summary,
                    encrypted,
                    ..
                } = node
                {
                    match deleted {
                        "content" => *content = None,
                        "summary" => *summary = None,
                        _ => *encrypted = None,
                    }
                }
            }
            let serialized = serde_json::to_value(&replay).unwrap();
            let replay: UrpRequest = serde_json::from_value(serialized).unwrap();
            let wire = match protocol {
                ProviderProtocol::ChatCompletion => {
                    encode::openai_chat::encode_request(&replay, "model-contract")
                }
                _ => encode::openai_responses::encode_request(&replay, "model-contract"),
            };
            let decoded = match protocol {
                ProviderProtocol::ChatCompletion => {
                    decode::openai_chat::decode_request(&wire).unwrap()
                }
                _ => decode::openai_responses::decode_request(&wire).unwrap(),
            };
            let (raw, summary, encrypted) = reasoning_values(&decoded.input);
            assert_eq!(raw, if deleted == "content" { "" } else { "RAW" });
            assert_eq!(summary, if deleted == "summary" { "" } else { "SUMMARY" });
            assert_eq!(
                encrypted,
                if deleted == "encrypted" {
                    vec![]
                } else {
                    vec![json!("CIPHER")]
                }
            );
        }
    }
}

#[test]
fn malformed_envelopes_are_not_decoded_as_plaintext() {
    for value in [
        json!("mz2.not-base64!"),
        json!("mz2.e30"),
        json!("mz1..payload"),
        json!("mz1.id."),
        json!({"v":2}),
        Value::Null,
    ] {
        assert!(parse_reasoning_envelope(&value).is_none());
        let mut nodes = vec![reasoning(None, Some("summary"), Some(value.clone()))];
        filter_and_unwrap_reasoning_envelopes_for_upstream(&mut nodes, "responses", "m", true);
        assert_eq!(
            reasoning_values(&nodes),
            (String::new(), "summary".into(), vec![value])
        );
    }
}

fn encrypted_delta(index: u32, value: Value) -> UrpStreamEvent {
    UrpStreamEvent::NodeDelta {
        node_index: index,
        delta: NodeDelta::Reasoning {
            content: None,
            summary: None,
            encrypted: Some(value),
            source: None,
            metadata: Default::default(),
        },
        usage: None,
        extra_body: HashMap::new(),
    }
}

#[test]
fn encrypted_fragment_accumulation_is_per_node_and_terminal_snapshot_is_atomic() {
    for origin in ["messages", "chat_completion"] {
        let mut state = ReasoningEnvelopeStreamState::default();
        for (index, fragment) in [(0, "first-"), (1, "second-"), (0, "old"), (1, "last")] {
            assert!(
                state
                    .wrap_event(encrypted_delta(index, json!(fragment)), origin, "m")
                    .is_empty()
            );
        }
        for (index, terminal, expected) in [
            (0, Some(json!("replacement")), json!("replacement")),
            (1, None, json!("second-last")),
        ] {
            let events = state.wrap_event(
                UrpStreamEvent::NodeDone {
                    node_index: index,
                    node: reasoning(None, None, terminal),
                    usage: None,
                    extra_body: HashMap::new(),
                },
                origin,
                "m",
            );
            assert_eq!(events.len(), 2);
            let UrpStreamEvent::NodeDelta {
                delta:
                    NodeDelta::Reasoning {
                        encrypted: Some(value),
                        content,
                        summary,
                        ..
                    },
                ..
            } = &events[0]
            else {
                panic!("{events:?}")
            };
            assert!(content.is_none() && summary.is_none());
            assert_eq!(parse_reasoning_envelope(value).unwrap().payload, expected);
            let UrpStreamEvent::NodeDone { node, .. } = &events[1] else {
                panic!()
            };
            assert_eq!(
                reasoning_values(std::slice::from_ref(node)).2,
                vec![value.clone()]
            );
        }
    }
}

#[test]
fn envelope_error_clears_pending_fragments_without_fabricating_output() {
    let mut state = ReasoningEnvelopeStreamState::default();
    assert!(
        state
            .wrap_event(encrypted_delta(0, json!("unfinished")), "messages", "m")
            .is_empty()
    );
    let events = state.wrap_event(
        UrpStreamEvent::Error {
            code: Some("broken".into()),
            message: "failed".into(),
            extra_body: HashMap::new(),
        },
        "messages",
        "m",
    );
    assert!(matches!(events.as_slice(), [UrpStreamEvent::Error { .. }]));
    let events = state.wrap_event(
        UrpStreamEvent::NodeDone {
            node_index: 0,
            node: reasoning(None, None, None),
            usage: None,
            extra_body: HashMap::new(),
        },
        "messages",
        "m",
    );
    assert_eq!(events.len(), 1);
    let UrpStreamEvent::NodeDone { node, .. } = &events[0] else {
        panic!()
    };
    assert!(reasoning_values(std::slice::from_ref(node)).2.is_empty());
}

#[test]
fn logprob_utf8_fragments_remain_bound_to_text_and_nullable_bytes() {
    let scores = logprobs::decode(Some(&json!([
        {"token":"partial-a","bytes":[230],"logprob":-0.1,"top_logprobs":[]},
        {"token":"partial-b","bytes":[177,137],"logprob":-0.2,"top_logprobs":[]},
        {"token":"x","bytes":null,"logprob":-0.3,"top_logprobs":[{"token":"y","bytes":null,"logprob":-1.0}]}
    ])));
    let valid = logprobs::valid(&scores, "汉x").unwrap();
    assert!(logprobs::valid(&scores, "改x").is_none());
    let fragments = logprobs::fragments(valid);
    assert_eq!(
        fragments.iter().map(|v| v.0.as_str()).collect::<String>(),
        "汉x"
    );
    assert_eq!(
        fragments
            .iter()
            .flat_map(|v| v.1.clone())
            .collect::<Vec<_>>(),
        valid
    );
    assert_eq!(fragments.len(), 2);
    let wire = serde_json::to_value(valid).unwrap();
    assert_eq!(wire[2].get("bytes"), Some(&Value::Null));
    assert_eq!(wire[2]["top_logprobs"][0].get("bytes"), Some(&Value::Null));
    for invalid in [
        json!({}),
        json!([{"token":"x","bytes":[999],"logprob":0}]),
        json!([{"token":"x","logprob":"invalid"}]),
    ] {
        assert!(logprobs::decode(Some(&invalid)).is_none());
    }
}

#[test]
fn citation_url_mapping_rebases_answer_offsets_and_preserves_unknown_fields() {
    let native = json!({"type":"url_citation","outer":"keep","url_citation":{"url":"https://example.com","title":"source","start_index":1,"end_index":3,"future":true}});
    let citation = Citation::decode(native.clone(), ProviderProtocol::ChatCompletion);
    assert_eq!(
        citation.encode(ProviderProtocol::ChatCompletion, 0),
        Some(native)
    );
    let cross = citation.encode(ProviderProtocol::Responses, 5).unwrap();
    assert_eq!(
        cross,
        json!({"type":"url_citation","url":"https://example.com","title":"source","start_index":6,"end_index":8})
    );
    assert!(citation.encode(ProviderProtocol::Messages, 0).is_none());
    for (kind, start, end) in [
        ("char_location", "start_char_index", "end_char_index"),
        ("page_location", "start_page_number", "end_page_number"),
        (
            "content_block_location",
            "start_block_index",
            "end_block_index",
        ),
    ] {
        let native = json!({"type":kind,"document_index":2,"document_title":"doc","cited_text":"source text",start:1,end:3});
        let citation = Citation::decode(native.clone(), ProviderProtocol::Messages);
        assert!(citation.answer_range.is_none());
        assert_eq!(
            citation.encode(ProviderProtocol::Messages, 100),
            Some(native)
        );
        assert!(citation.encode(ProviderProtocol::Responses, 0).is_none());
    }
}

#[test]
fn citation_unknown_origin_and_file_shapes_do_not_change_semantics() {
    for kind in ["file_citation", "container_file_citation", "file_path"] {
        let native = json!({"type":kind,"file_id":"file-1","index":2});
        let citation = Citation::decode(native.clone(), ProviderProtocol::Responses);
        assert_eq!(
            citation.encode(ProviderProtocol::Responses, 0),
            Some(native)
        );
        assert!(
            citation
                .encode(ProviderProtocol::ChatCompletion, 0)
                .is_none()
        );
    }
    let native = json!({"type":"future_citation","payload":{"x":1}});
    let citation = Citation::decode(native.clone(), ProviderProtocol::Messages);
    assert_eq!(citation.encode(ProviderProtocol::Messages, 0), Some(native));
    assert!(citation.encode(ProviderProtocol::Responses, 0).is_none());
}

#[test]
fn messages_iteration_accounting_uses_each_iteration_once() {
    let wire = json!({"id":"msg","model":"m","type":"message","role":"assistant","content":[],"stop_reason":"end_turn","usage":{
        "input_tokens":10,"output_tokens":4,"cache_read_input_tokens":3,"cache_creation_input_tokens":2,
        "iterations":[
            {"type":"compaction","input_tokens":100,"output_tokens":20,"cache_read_input_tokens":30,"cache_creation_input_tokens":5},
            {"type":"message","input_tokens":10,"output_tokens":4,"cache_read_input_tokens":3,"cache_creation_input_tokens":2}
        ]
    }});
    let decoded = decode::anthropic::decode_response(&wire).unwrap();
    let usage = decoded.usage.as_ref().unwrap();
    assert_eq!((usage.input_tokens, usage.output_tokens), (15, 4));
    let total = usage.accounting();
    assert_eq!((total.input_tokens, total.output_tokens), (150, 24));
    assert_eq!(usage.total_tokens(), 174);
    assert_eq!(total.input_details.as_ref().unwrap().cache_read_tokens, 33);
    let replay = encode::anthropic::encode_response(&decoded, "m");
    assert_eq!(replay["usage"]["input_tokens"], 10);
    assert_eq!(replay["usage"]["iterations"][0]["input_tokens"], 100);
    for target in [
        ProviderProtocol::ChatCompletion,
        ProviderProtocol::Responses,
    ] {
        let wire = encode_response(target, &decoded);
        assert_eq!(
            wire["usage"][if target == ProviderProtocol::Responses {
                "input_tokens"
            } else {
                "prompt_tokens"
            }],
            150
        );
        assert_eq!(
            wire["usage"][if target == ProviderProtocol::Responses {
                "output_tokens"
            } else {
                "completion_tokens"
            }],
            24
        );
    }
}

#[test]
fn typed_outcome_wins_over_finish_reason_and_stale_native_fields() {
    for status in [ResponseStatus::Failed, ResponseStatus::Cancelled] {
        let mut canonical = response(vec![Node::text(OrdinaryRole::Assistant, "partial")]);
        canonical.outcome = Some(ResponseOutcome {
            status,
            error: Some(outcome::ResponseError {
                code: Some("failure".into()),
                message: Some("failed".into()),
                extra_body: HashMap::new(),
            }),
            incomplete_reason: None,
            incomplete_extra: HashMap::new(),
        });
        canonical
            .extra_body
            .insert("status".into(), json!("completed"));
        canonical
            .extra_body
            .insert("error".into(), json!({"message":"stale"}));
        let wire = encode_response(ProviderProtocol::Responses, &canonical);
        assert_eq!(wire["status"], json!(status));
        assert_eq!(wire["error"]["message"], "failed");
        assert_eq!(wire["output"][0]["content"][0]["text"], "partial");
        for target in [ProviderProtocol::ChatCompletion, ProviderProtocol::Messages] {
            let wire = encode_response(target, &canonical);
            assert_eq!(wire["error"]["message"], "failed");
            assert!(wire.get("choices").is_none() && wire.get("stop_reason").is_none());
        }
        canonical.outcome = None;
        let wire = encode_response(ProviderProtocol::Responses, &canonical);
        assert_eq!(wire["status"], "completed");
        assert!(wire["error"].is_null());
    }
}

#[test]
fn citation_typed_absence_cannot_be_restored_by_unknown_field_storage() {
    let mut citation = Citation::decode(
        json!({"type":"url_citation","url":"https://current.example","title":"old","start_index":0,"end_index":5}),
        ProviderProtocol::Responses,
    );
    if let citations::CitationSource::Url { title, .. } = &mut citation.source {
        *title = None;
    }
    citation.answer_range = None;
    citation.extra_body = HashMap::from([
        ("title".into(), json!("stale")),
        ("start_index".into(), json!(1)),
        ("end_index".into(), json!(2)),
        ("url".into(), json!("https://stale.example")),
        ("future".into(), json!(true)),
    ]);
    let wire = citation.encode(ProviderProtocol::Responses, 0).unwrap();
    assert_eq!(wire["url"], "https://current.example");
    assert!(wire.get("title").is_none());
    assert!(wire.get("start_index").is_none() && wire.get("end_index").is_none());
    assert_eq!(wire["future"], true);
}

#[test]
fn envelope_missing_id_and_disabled_binding_do_not_change_raw_or_summary() {
    for protocol in protocols() {
        let mut node = reasoning(Some("raw"), Some("summary"), Some(json!("cipher")));
        if let Node::Reasoning { id, .. } = &mut node {
            *id = None;
        }
        let mut canonical = response(vec![node]);
        wrap_reasoning_envelopes_in_response(&mut canonical, protocol.as_str(), "source");
        let mut mismatched = canonical.output.clone();
        filter_and_unwrap_reasoning_envelopes_for_upstream(
            &mut mismatched,
            "different-protocol",
            "source",
            true,
        );
        assert!(mismatched.is_empty());
        filter_and_unwrap_reasoning_envelopes_for_upstream(
            &mut canonical.output,
            "different-protocol",
            "different-model",
            false,
        );
        assert_eq!(
            reasoning_values(&canonical.output),
            ("raw".into(), "summary".into(), vec![json!("cipher")])
        );
        assert!(canonical.output[0].id().is_none());
    }
}

#[tokio::test]
async fn failed_and_cancelled_outcomes_never_synthesize_success_in_any_protocol() {
    for status in [ResponseStatus::Failed, ResponseStatus::Cancelled] {
        let mut canonical = response(vec![Node::text(OrdinaryRole::Assistant, "partial")]);
        canonical.outcome = Some(ResponseOutcome {
            status,
            error: Some(outcome::ResponseError {
                code: Some("failure".into()),
                message: Some("failed".into()),
                extra_body: HashMap::new(),
            }),
            incomplete_reason: None,
            incomplete_extra: HashMap::new(),
        });
        canonical.usage = Some(Usage {
            input_tokens: 7,
            output_tokens: 3,
            ..Default::default()
        });
        for protocol in protocols() {
            let events = synthetic_decode(protocol, &canonical).await;
            if protocol == ProviderProtocol::Responses {
                let terminals: Vec<_> = events
                    .iter()
                    .filter_map(|event| match event {
                        UrpStreamEvent::ResponseDone {
                            outcome,
                            output,
                            usage,
                            ..
                        } => Some((outcome, output, usage)),
                        _ => None,
                    })
                    .collect();
                assert_eq!(terminals.len(), 1, "{events:?}");
                assert_eq!(terminals[0].0.as_ref().unwrap().status, status);
                assert_eq!(terminals[0].2.as_ref().unwrap().total_tokens(), 10);
                assert!(
                    terminals[0]
                        .1
                        .iter()
                        .any(|node| matches!(node,Node::Text {content,..} if content=="partial"))
                );
            } else {
                assert_eq!(
                    events
                        .iter()
                        .filter(|event| matches!(event, UrpStreamEvent::Error { .. }))
                        .count(),
                    1,
                    "{protocol:?} {events:?}"
                );
                assert!(
                    !events
                        .iter()
                        .any(|event| matches!(event, UrpStreamEvent::ResponseDone { .. })),
                    "{protocol:?} {events:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn chat_citation_offsets_follow_unicode_output_in_buffered_and_streamed_modes() {
    let mut nodes = vec![
        Node::text(OrdinaryRole::Assistant, "汉🙂"),
        Node::text(OrdinaryRole::Assistant, "abc"),
    ];
    for node in &mut nodes {
        if let Node::Text { citations, .. } = node {
            citations.push(Citation::decode(json!({"type":"url_citation","url":"https://example.com","title":"source","start_index":0,"end_index":1}),ProviderProtocol::Responses));
        }
    }
    let canonical = response(nodes);
    let wire = encode_response(ProviderProtocol::ChatCompletion, &canonical);
    assert_eq!(wire["choices"][0]["message"]["content"], "汉🙂\n\nabc");
    assert_eq!(
        wire["choices"][0]["message"]["annotations"][1]["url_citation"]["start_index"],
        4
    );
    let events = synthetic_decode(ProviderProtocol::ChatCompletion, &canonical).await;
    let output = events
        .iter()
        .find_map(|event| match event {
            UrpStreamEvent::ResponseDone { output, .. } => Some(output),
            _ => None,
        })
        .unwrap();
    let texts: Vec<_> = output
        .iter()
        .filter_map(|node| match node {
            Node::Text {
                content, citations, ..
            } => Some((content, citations)),
            _ => None,
        })
        .collect();
    assert_eq!(texts.len(), 1);
    assert_eq!(texts[0].0, "汉🙂abc");
    assert_eq!(texts[0].1.len(), 2);
    assert_eq!(texts[0].1[1].answer_range.as_ref().unwrap().start, 2);
}

#[test]
fn empty_encrypted_payloads_do_not_become_nonempty_reasoning_envelopes() {
    for empty in [Value::Null, json!(""), json!([]), json!({})] {
        let mut canonical = response(vec![reasoning(None, None, Some(empty.clone()))]);
        wrap_reasoning_envelopes_in_response(&mut canonical, "responses", "m");
        let (_, _, encrypted) = reasoning_values(&canonical.output);
        assert_eq!(encrypted, vec![empty]);
        assert!(parse_reasoning_envelope(&encrypted[0]).is_none());
        let wire = encode_response(ProviderProtocol::Responses, &canonical);
        assert_eq!(wire["output"], json!([]));
    }
}

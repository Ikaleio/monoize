//! Chat stream lifecycle at node and terminal boundaries (STR-15, PC7g, DC7b).

use super::ChatStreamEncoder;
use crate::urp::stream_encode::{self, StreamEncodeOptions};
use crate::urp::{Node, NodeDelta, NodeHeader, TerminalNode, UrpStreamEvent};
use axum::response::{IntoResponse, Sse};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::time::Instant;
use tokio::sync::mpsc;

fn node(value: Value) -> Node {
    serde_json::from_value(value).unwrap()
}

fn start(node_index: u32, node: &Node) -> UrpStreamEvent {
    UrpStreamEvent::NodeStart {
        node_index,
        header: NodeHeader::from(node),
        extra_body: HashMap::new(),
    }
}

fn delta(node_index: u32, delta: NodeDelta) -> UrpStreamEvent {
    UrpStreamEvent::NodeDelta {
        node_index,
        delta,
        usage: None,
        extra_body: HashMap::new(),
    }
}

fn done(node_index: u32, node: &Node) -> UrpStreamEvent {
    UrpStreamEvent::NodeDone {
        node_index,
        node: node.clone(),
        usage: None,
        extra_body: HashMap::new(),
    }
}

async fn encode_live(events: Vec<UrpStreamEvent>) -> Vec<String> {
    let (event_tx, event_rx) = mpsc::channel(64);
    for event in events {
        event_tx.send(event).await.unwrap();
    }
    drop(event_tx);
    let (tx, mut rx) = mpsc::channel(256);
    let options = StreamEncodeOptions {
        logical_model: "m".into(),
        sse_max_frame_length: None,
        mask_sensitive_info: false,
        chat_include_usage: false,
        responses_request_echo: None,
        started_at: Instant::now(),
        replay_reasoning_duration_secs: None,
    };
    stream_encode::run(ChatStreamEncoder::new(tx, options), event_rx)
        .await
        .unwrap();
    let mut frames = Vec::new();
    while let Some(event) = rx.recv().await {
        frames.push(Ok::<_, Infallible>(event));
    }
    let body = Sse::new(futures_util::stream::iter(frames))
        .into_response()
        .into_body();
    let wire = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    String::from_utf8(wire.to_vec())
        .unwrap()
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(str::to_string)
        .collect()
}

#[tokio::test]
async fn node_done_emits_unsent_fragments_and_terminal_entries_resolve_by_index() {
    let reasoning = node(json!({"type":"reasoning","summary":"plan","encrypted":"sealed"}));
    let text = node(json!({"type":"text","role":"assistant","content":"Hello"}));
    let call =
        node(json!({"type":"tool_call","call_id":"call_1","name":"f","arguments":"{\"a\":1}"}));
    let frames = encode_live(vec![
        UrpStreamEvent::ResponseStart {
            usage: None,
            id: "chatcmpl-1".into(),
            model: "upstream".into(),
            created_at: Some(7),
            extra_body: HashMap::new(),
        },
        start(0, &reasoning),
        delta(
            0,
            NodeDelta::Reasoning {
                metadata: Default::default(),
                content: None,
                encrypted: None,
                summary: Some("plan".into()),
                source: None,
            },
        ),
        done(0, &reasoning),
        start(1, &text),
        delta(
            1,
            NodeDelta::Text {
                logprobs: None,
                citations: Vec::new(),
                signature: None,
                content: "Hel".into(),
            },
        ),
        done(1, &text),
        start(2, &call),
        delta(
            2,
            NodeDelta::ToolCallArguments {
                arguments: "{\"a\"".into(),
            },
        ),
        done(2, &call),
        UrpStreamEvent::ResponseDone {
            outcome: None,
            finish_reason: None,
            usage: None,
            output: vec![
                TerminalNode::streamed(0, reasoning),
                TerminalNode::streamed(1, text),
                TerminalNode::streamed(2, call.clone()),
                // Neither a claimed index nor an equal `call_id` marks an entry as streamed.
                TerminalNode::streamed(2, call.clone()),
                TerminalNode::unannounced(call),
            ],
            extra_body: HashMap::new(),
        },
    ])
    .await;

    assert_eq!(frames.last().map(String::as_str), Some("[DONE]"));
    let chunks: Vec<Value> = frames[..frames.len() - 1]
        .iter()
        .map(|frame| serde_json::from_str(frame).unwrap())
        .collect();
    let mut content = String::new();
    let mut details = Vec::new();
    let mut calls: BTreeMap<u64, (String, String)> = BTreeMap::new();
    let mut finish_reasons = Vec::new();
    for chunk in &chunks {
        assert_eq!(chunk["id"], "chatcmpl-1");
        assert_eq!(chunk["created"], 7);
        let choice = &chunk["choices"][0];
        if let Some(reason) = choice["finish_reason"].as_str() {
            finish_reasons.push(reason.to_string());
        }
        let delta = &choice["delta"];
        content.push_str(delta["content"].as_str().unwrap_or_default());
        for detail in delta["reasoning_details"].as_array().into_iter().flatten() {
            details.push((detail["type"].clone(), detail["data"].clone()));
        }
        for call in delta["tool_calls"].as_array().into_iter().flatten() {
            let entry = calls.entry(call["index"].as_u64().unwrap()).or_default();
            if let Some(id) = call["id"].as_str() {
                entry.0 = id.to_string();
            }
            entry
                .1
                .push_str(call["function"]["arguments"].as_str().unwrap_or_default());
        }
    }
    // STR-15: the text suffix and the never-streamed encrypted payload go out at `NodeDone`.
    assert_eq!(content, "Hello");
    assert!(details.contains(&(json!("reasoning.encrypted"), json!("sealed"))));
    assert_eq!(
        calls.into_iter().collect::<Vec<_>>(),
        (0..3u64)
            .map(|index| (index, ("call_1".to_string(), "{\"a\":1}".to_string())))
            .collect::<Vec<_>>()
    );
    assert_eq!(finish_reasons, ["tool_calls"]);
}

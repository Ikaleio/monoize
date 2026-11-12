use super::stream_chat_to_urp_events;
use crate::urp::{CHAT_CHUNK_EXTRA_KEY, FinishReason, Node, NodeHeader, UrpStreamEvent};
use serde_json::{Value, json};
use std::collections::HashSet;
use tokio::sync::mpsc;

async fn decode(frames: &[Value]) -> Vec<UrpStreamEvent> {
    let mut wire: String = frames
        .iter()
        .map(|frame| format!("data: {frame}\n\n"))
        .collect();
    wire.push_str("data: [DONE]\n\n");
    let upstream = reqwest::Response::from(
        axum::http::Response::builder()
            .header("content-type", "text/event-stream")
            .body(wire)
            .unwrap(),
    );
    let request = crate::handlers::UrpRequest {
        audio_output_format: None,
        model: "chat-test".into(),
        max_multiplier: None,
        server_tool_usage_classes: vec![],
        messages_custom_tool_names: HashSet::new(),
        affinity_explicit: None,
        affinity_prefix_hash: String::new(),
    };
    let (tx, mut rx) = mpsc::channel(64);
    stream_chat_to_urp_events(&request, upstream, tx, None, None, 1000)
        .await
        .unwrap();
    let mut events = vec![];
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    events
}

fn terminal(events: &[UrpStreamEvent]) -> (&[crate::urp::TerminalNode], Option<FinishReason>) {
    match events.last() {
        Some(UrpStreamEvent::ResponseDone {
            output,
            finish_reason,
            ..
        }) => (output.as_slice(), *finish_reason),
        other => panic!("missing ResponseDone: {other:?}"),
    }
}

/// DC7b, DC6a, STR-8a: the first chunk's `created` is typed, its unknown members stay chunk
/// extras, and every terminal entry carries the index of the node it completes.
#[tokio::test]
async fn chunk_created_is_typed_and_terminal_entries_keep_streamed_indices() {
    let events = decode(&[
        json!({"id":"chatcmpl-1","object":"chat.completion.chunk","created":1700000000,"model":"up","system_fingerprint":"fp_1","choices":[{"index":0,"delta":{"role":"assistant","reasoning_content":"think"},"finish_reason":null}]}),
        json!({"id":"chatcmpl-1","created":1700000001,"choices":[{"index":0,"delta":{"content":"answer"},"finish_reason":null}]}),
        json!({"id":"chatcmpl-1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"lookup","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}),
    ])
    .await;
    let Some(UrpStreamEvent::ResponseStart {
        id,
        created_at,
        extra_body,
        ..
    }) = events.first()
    else {
        panic!("missing ResponseStart: {events:?}");
    };
    assert_eq!(id, "chatcmpl-1");
    assert_eq!(*created_at, Some(1_700_000_000));
    assert_eq!(
        extra_body.get(CHAT_CHUNK_EXTRA_KEY),
        Some(&json!({"system_fingerprint":"fp_1"}))
    );
    let started: Vec<u32> = events
        .iter()
        .filter_map(|event| match event {
            UrpStreamEvent::NodeStart { node_index, .. } => Some(*node_index),
            _ => None,
        })
        .collect();
    assert_eq!(started, [0, 1, 2]);
    let (output, finish_reason) = terminal(&events);
    let indices: Vec<Option<u32>> = output.iter().map(|entry| entry.node_index).collect();
    assert_eq!(indices, [Some(0), Some(1), Some(2)]);
    assert!(matches!(output[0].node, Node::Reasoning { .. }));
    assert!(matches!(&output[1].node, Node::Text { content, .. } if content == "answer"));
    assert!(matches!(&output[2].node, Node::ToolCall { call_id, .. } if call_id == "call_1"));
    assert_eq!(finish_reason, Some(FinishReason::ToolCalls));
}

/// PC7e, PC7f: an unrecognized block is one provider item, never a tool call, and the
/// terminal message that repeats it does not emit it twice.
#[tokio::test]
async fn unrecognized_block_is_one_provider_item_across_terminal_snapshot() {
    let block = json!({"type":"web_search_call","id":"ws_1","status":"completed"});
    let events = decode(&[
        json!({"choices":[{"index":0,"delta":{"role":"assistant","content":[block]},"finish_reason":null}]}),
        json!({"choices":[{"index":0,"message":{"role":"assistant","content":[block]},"finish_reason":"stop"}]}),
    ])
    .await;
    assert!(!events.iter().any(|event| matches!(
        event,
        UrpStreamEvent::NodeStart {
            header: NodeHeader::ToolCall { .. },
            ..
        }
    )));
    let (output, finish_reason) = terminal(&events);
    assert_eq!(output.len(), 1, "{output:?}");
    assert_eq!(output[0].node_index, Some(0));
    assert!(
        matches!(&output[0].node, Node::ProviderItem { id, body, .. } if id.as_deref() == Some("ws_1") && *body == block)
    );
    assert_eq!(finish_reason, Some(FinishReason::Stop));
}

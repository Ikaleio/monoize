#[cfg(test)]
mod tests {
    use super::*;
    use crate::request_capture::with_sse_capture;
    use crate::urp::{FinishReason, Node, NodeHeader, OrdinaryRole, UrpResponse};
    use std::sync::Arc;
    use tokio::sync::Mutex;

    fn empty_map() -> HashMap<String, Value> {
        HashMap::new()
    }

    #[test]
    fn responses_stream_provider_item_filters_nested_internal_metadata() {
        let native_body = json!({
            "type": "compaction",
            "vendor_unknown": {
                "keep": 1,
                "_monoize_nested": "drop",
                "rows": [{ "keep_row": true, "_monoize_row": "drop" }]
            },
            "_monoize_top": "drop"
        });

        let start_extra = HashMap::from([
            (
                "vendor_unknown".to_string(),
                json!({
                    "keep": 1,
                    "_monoize_nested": "drop",
                    "rows": [{ "keep_row": true, "_monoize_row": "drop" }]
                }),
            ),
            ("_monoize_top".to_string(), json!("drop")),
        ]);
        let start = stream_output_item_start_stub_from_node_header(
            ResponsesOutputZone::ProviderItem,
            &NodeHeader::ProviderItem {
                id: None,
                origin_protocol: urp::ProviderProtocol::Responses,
                role: OrdinaryRole::Assistant,
                item_type: "compaction".to_string(),
            },
            &start_extra,
            &HashMap::new(),
        );
        let done = encode_responses_provider_output_item(
            "compaction",
            &native_body,
            &HashMap::new(),
            None,
        );

        assert_eq!(
            start,
            json!({
                "type": "compaction",
                "vendor_unknown": { "keep": 1, "rows": [{ "keep_row": true }] }
            })
        );
        assert_eq!(
            done,
            json!({
                "type": "compaction",
                "vendor_unknown": { "keep": 1, "rows": [{ "keep_row": true }] }
            })
        );
        assert_eq!(native_body["_monoize_top"], json!("drop"));
    }

    fn captured_responses_json_frames(frames: &[String]) -> Vec<(String, Value)> {
        frames
            .iter()
            .filter_map(|frame| {
                let mut event_name = None;
                let mut data = None;
                for line in frame.lines() {
                    if let Some(value) = line.strip_prefix("event: ") {
                        event_name = Some(value.to_string());
                    } else if let Some(value) = line.strip_prefix("data: ")
                        && value != "[DONE]"
                    {
                        data = Some(
                            serde_json::from_str(value).expect("Responses SSE frame JSON payload"),
                        );
                    }
                }
                event_name.zip(data)
            })
            .collect()
    }

    async fn send_completed_tool_node(
        event_tx: &mpsc::Sender<UrpStreamEvent>,
        node_index: u32,
        item_id: &str,
        call_id: &str,
        name: &str,
        arguments: &str,
    ) {
        event_tx
            .send(UrpStreamEvent::NodeStart {
                node_index,
                header: NodeHeader::ToolCall {
                    id: Some(item_id.to_string()),
                    call_id: call_id.to_string(),
                    name: name.to_string(),
                },
                extra_body: HashMap::new(),
            })
            .await
            .expect("tool node start");
        event_tx
            .send(UrpStreamEvent::NodeDelta {
                node_index,
                delta: urp::NodeDelta::ToolCallArguments {
                    arguments: arguments.to_string(),
                },
                usage: None,
                extra_body: HashMap::new(),
            })
            .await
            .expect("tool arguments delta");
        event_tx
            .send(UrpStreamEvent::NodeDone {
                node_index,
                node: Node::ToolCall {
                    id: Some(item_id.to_string()),
                    call_id: call_id.to_string(),
                    name: name.to_string(),
                    arguments: arguments.to_string(),
                    extra_body: HashMap::new(),
                },
                usage: None,
                extra_body: HashMap::new(),
            })
            .await
            .expect("tool node done");
    }

    #[test]
    fn reasoning_start_stub_uses_header_or_envelope_item_id() {
        let from_header = stream_output_item_start_stub_from_node_header(
            ResponsesOutputZone::Reasoning,
            &urp::NodeHeader::Reasoning {
                id: Some("rs_header".to_string()),
            },
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(from_header["id"], json!("rs_header"));

        let envelope_extra = HashMap::from([("id".to_string(), json!("rs_envelope"))]);
        let from_envelope = stream_output_item_start_stub_from_node_header(
            ResponsesOutputZone::Reasoning,
            &urp::NodeHeader::Reasoning { id: None },
            &HashMap::new(),
            &envelope_extra,
        );
        assert_eq!(from_envelope["id"], json!("rs_envelope"));
    }

    #[test]
    fn image_generation_call_uses_native_top_level_stream_items() {
        let native_item = json!({
            "type": "image_generation_call",
            "id": "ig_1",
            "status": "completed",
            "result": "QUJD",
            "output_format": "webp",
            "future_field": 7
        });
        let extra_body = HashMap::from([(
            urp::RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY.to_string(),
            native_item.clone(),
        )]);
        let header = urp::NodeHeader::Image {
            id: Some("ig_1".to_string()),
            role: OrdinaryRole::Assistant,
        };
        assert_eq!(
            zone_from_node_header(&header, &extra_body),
            ResponsesOutputZone::ImageGenerationCall
        );
        let start = stream_output_item_start_stub_from_node_header(
            ResponsesOutputZone::ImageGenerationCall,
            &header,
            &extra_body,
            &HashMap::new(),
        );
        assert_eq!(start["type"], json!("image_generation_call"));
        assert_eq!(start["status"], json!("in_progress"));
        assert!(start.get("result").is_none());

        let done = encode_stream_output_item_from_node(&urp::Node::Image {
            id: Some("ig_1".to_string()),
            role: OrdinaryRole::Assistant,
            source: urp::ImageSource::Base64 {
                media_type: "image/webp".to_string(),
                data: "QUJD".to_string(),
            },
            extra_body,
        });
        assert_eq!(done, native_item);
        assert!(done.get("content").is_none());
    }

    #[test]
    fn streamed_completion_uses_nonstream_response_output_shape_for_merged_items() {
        let output = vec![
            urp::Node::Reasoning {
                id: None,
                content: Some("think".to_string()),
                encrypted: Some(json!("sig_1")),
                summary: None,
                source: None,
                extra_body: empty_map(),
            },
            urp::Node::NextDownstreamEnvelopeExtra {
                extra_body: {
                    let mut map = empty_map();
                    map.insert("custom_message_field".to_string(), json!(true));
                    map
                },
            },
            urp::Node::Text {
                id: None,
                role: OrdinaryRole::Assistant,
                content: "answer".to_string(),
                phase: Some("analysis".to_string()),
                extra_body: empty_map(),
            },
            urp::Node::ToolCall {
                id: None,
                call_id: "call_1".to_string(),
                name: "lookup".to_string(),
                arguments: "{}".to_string(),
                extra_body: empty_map(),
            },
        ];

        let encoded = urp::encode::openai_responses::encode_response(
            &UrpResponse {
                id: "resp_1".to_string(),
                model: "gpt-5.4".to_string(),
                created_at: None,
                output,
                finish_reason: Some(FinishReason::ToolCalls),
                usage: None,
                extra_body: empty_map(),
            },
            "gpt-5.4",
        );
        let output = encoded["output"].as_array().expect("output array");
        assert_eq!(output.len(), 3);
        assert_eq!(output[0]["type"], json!("reasoning"));
        assert_eq!(output[1]["type"], json!("message"));
        assert_eq!(output[1]["phase"], json!("analysis"));
        assert_eq!(output[1]["custom_message_field"], json!(true));
        assert_eq!(output[2]["type"], json!("function_call"));
    }

    #[test]
    fn reasoning_duration_helper_preserves_existing_duration() {
        let item = json!({
            "type": "reasoning",
            "id": "rs_1",
            "summary": [{ "type": "summary_text", "text": "summary" }],
            "duration": 7
        });

        let with_duration = reasoning_item_with_duration(item, Some(3));

        assert_eq!(with_duration["duration"], json!(7));
    }

    #[test]
    fn reasoning_duration_helper_synthesizes_missing_duration() {
        let item = json!({
            "type": "reasoning",
            "id": "rs_1",
            "summary": [{ "type": "summary_text", "text": "summary" }]
        });

        let with_duration = reasoning_item_with_duration(item, Some(3));

        assert_eq!(with_duration["duration"], json!(3));
    }

    #[test]
    fn reasoning_duration_uses_stream_elapsed_when_node_lifecycle_is_short() {
        let stream_started_at = Instant::now() - std::time::Duration::from_secs(5);
        let node_state = StreamedNodeState {
            output_index: 0,
            zone: ResponsesOutputZone::Reasoning,
            content_index: None,
            item_id: "rs_1".to_string(),
            phase: None,
            call_id: None,
            name: None,
            reasoning_summary_part_added_sent: false,
            message_start_emitted: true,
            output_item_start_emitted: true,
            output_item_start: None,
            header: None,
            node_extra_body: HashMap::new(),
            completed_item: None,
            is_shared_message_output: false,
            reasoning_started_at: Some(Instant::now()),
        };

        assert_eq!(
            reasoning_duration_secs(&node_state, stream_started_at),
            Some(5)
        );
    }

    #[test]
    fn terminal_reasoning_added_item_gets_duration_for_openwebui_intermediate_render() {
        let item = json!({
            "type": "reasoning",
            "id": "rs_1",
            "status": "completed",
            "summary": [{ "type": "summary_text", "text": "summary" }]
        });

        let item = maybe_reasoning_added_item_with_duration(item, 9);

        assert_eq!(item["duration"], json!(9));
    }

    #[test]
    fn live_empty_reasoning_added_item_does_not_get_duration() {
        let item = json!({
            "type": "reasoning",
            "id": "rs_1",
            "status": "in_progress",
            "summary": []
        });

        let item = maybe_reasoning_added_item_with_duration(item, 9);

        assert!(item.get("duration").is_none());
    }

    #[tokio::test]
    async fn response_done_output_removes_completed_cache_item_and_emits_terminal_only_lifecycle()
    {
        let (event_tx, event_rx) = mpsc::channel(16);
        let (sse_tx, mut sse_rx) = mpsc::channel(64);
        let frames = Arc::new(Mutex::new(Vec::new()));

        event_tx
            .send(UrpStreamEvent::ResponseStart {
                id: "resp_terminal_authority".to_string(),
                model: "gpt-5.4".to_string(),
                extra_body: HashMap::new(),
            })
            .await
            .expect("response start");
        send_completed_tool_node(
            &event_tx,
            0,
            "fc_removed",
            "call_removed",
            "removed_tool",
            "{\"removed\":true}",
        )
        .await;
        event_tx
            .send(UrpStreamEvent::ResponseDone {
                finish_reason: Some(FinishReason::ToolCalls),
                usage: None,
                output: vec![Node::ToolCall {
                    id: Some("fc_terminal".to_string()),
                    call_id: "call_terminal".to_string(),
                    name: "terminal_tool".to_string(),
                    arguments: "{\"terminal\":true}".to_string(),
                    extra_body: HashMap::new(),
                }],
                extra_body: HashMap::new(),
            })
            .await
            .expect("response done");
        drop(event_tx);

        with_sse_capture(frames.clone(), async {
            encode_urp_stream_as_responses(
                event_rx,
                sse_tx,
                "gpt-5.4",
                Instant::now(),
                None,
            )
            .await
            .expect("encode Responses stream");
        })
        .await;
        while sse_rx.recv().await.is_some() {}

        let frames = frames.lock().await;
        let json_frames = captured_responses_json_frames(&frames);
        let completed = json_frames
            .iter()
            .find(|(event, _)| event == "response.completed")
            .map(|(_, payload)| payload)
            .expect("response.completed frame");
        assert_eq!(
            completed["response"]["output"],
            json!([{
                "type": "function_call",
                "id": "fc_terminal",
                "call_id": "call_terminal",
                "name": "terminal_tool",
                "arguments": "{\"terminal\":true}",
                "status": "completed"
            }])
        );

        let added: Vec<&Value> = json_frames
            .iter()
            .filter(|(event, _)| event == "response.output_item.added")
            .map(|(_, payload)| payload)
            .collect();
        let done: Vec<&Value> = json_frames
            .iter()
            .filter(|(event, _)| event == "response.output_item.done")
            .map(|(_, payload)| payload)
            .collect();
        assert_eq!(added.len(), 2, "{}", frames.join(""));
        assert_eq!(done.len(), 2, "{}", frames.join(""));
        assert_eq!(added[0]["item"]["id"], json!("fc_removed"));
        assert_eq!(added[0]["output_index"], json!(0));
        assert_eq!(added[1]["item"]["id"], json!("fc_terminal"));
        assert_eq!(added[1]["output_index"], json!(1));
        assert_eq!(done[1]["item"]["id"], json!("fc_terminal"));
        assert_eq!(done[1]["output_index"], json!(1));
        let terminal_added_position = json_frames
            .iter()
            .position(|(event, payload)| {
                event == "response.output_item.added"
                    && payload["item"]["id"] == json!("fc_terminal")
            })
            .expect("terminal-only added event");
        let terminal_arguments_delta_position = json_frames
            .iter()
            .position(|(event, payload)| {
                event == "response.function_call_arguments.delta"
                    && payload["item_id"] == json!("fc_terminal")
            })
            .expect("terminal-only arguments delta");
        let terminal_arguments_done_position = json_frames
            .iter()
            .position(|(event, payload)| {
                event == "response.function_call_arguments.done"
                    && payload["item_id"] == json!("fc_terminal")
            })
            .expect("terminal-only arguments done");
        let terminal_done_position = json_frames
            .iter()
            .position(|(event, payload)| {
                event == "response.output_item.done"
                    && payload["item"]["id"] == json!("fc_terminal")
            })
            .expect("terminal-only output item done");
        assert!(
            terminal_added_position < terminal_arguments_delta_position
                && terminal_arguments_delta_position < terminal_arguments_done_position
                && terminal_arguments_done_position < terminal_done_position,
            "{}",
            frames.join("")
        );
    }

    #[tokio::test]
    async fn response_done_output_reorders_and_replaces_same_id_items_without_repeating_lifecycles()
    {
        let (event_tx, event_rx) = mpsc::channel(16);
        let (sse_tx, mut sse_rx) = mpsc::channel(64);
        let frames = Arc::new(Mutex::new(Vec::new()));

        event_tx
            .send(UrpStreamEvent::ResponseStart {
                id: "resp_terminal_reorder".to_string(),
                model: "gpt-5.4".to_string(),
                extra_body: HashMap::new(),
            })
            .await
            .expect("response start");
        send_completed_tool_node(
            &event_tx,
            0,
            "fc_a",
            "call_a",
            "tool_a",
            "{\"version\":1}",
        )
        .await;
        send_completed_tool_node(
            &event_tx,
            1,
            "fc_b",
            "call_b",
            "tool_b",
            "{\"version\":1}",
        )
        .await;
        event_tx
            .send(UrpStreamEvent::ResponseDone {
                finish_reason: Some(FinishReason::ToolCalls),
                usage: None,
                output: vec![
                    Node::ToolCall {
                        id: Some("fc_b".to_string()),
                        call_id: "call_b".to_string(),
                        name: "tool_b_replaced".to_string(),
                        arguments: "{\"version\":2}".to_string(),
                        extra_body: HashMap::new(),
                    },
                    Node::ToolCall {
                        id: Some("fc_a".to_string()),
                        call_id: "call_a".to_string(),
                        name: "tool_a_replaced".to_string(),
                        arguments: "{\"version\":3}".to_string(),
                        extra_body: HashMap::new(),
                    },
                ],
                extra_body: HashMap::new(),
            })
            .await
            .expect("response done");
        drop(event_tx);

        with_sse_capture(frames.clone(), async {
            encode_urp_stream_as_responses(
                event_rx,
                sse_tx,
                "gpt-5.4",
                Instant::now(),
                None,
            )
            .await
            .expect("encode Responses stream");
        })
        .await;
        while sse_rx.recv().await.is_some() {}

        let frames = frames.lock().await;
        let json_frames = captured_responses_json_frames(&frames);
        let completed = json_frames
            .iter()
            .find(|(event, _)| event == "response.completed")
            .map(|(_, payload)| payload)
            .expect("response.completed frame");
        let output = completed["response"]["output"]
            .as_array()
            .expect("completed output");
        assert_eq!(output.len(), 2);
        assert_eq!(output[0]["id"], json!("fc_b"));
        assert_eq!(output[0]["name"], json!("tool_b_replaced"));
        assert_eq!(output[0]["arguments"], json!("{\"version\":2}"));
        assert_eq!(output[1]["id"], json!("fc_a"));
        assert_eq!(output[1]["name"], json!("tool_a_replaced"));
        assert_eq!(output[1]["arguments"], json!("{\"version\":3}"));
        assert_eq!(
            json_frames
                .iter()
                .filter(|(event, _)| event == "response.output_item.added")
                .count(),
            2,
            "{}",
            frames.join("")
        );
        assert_eq!(
            json_frames
                .iter()
                .filter(|(event, _)| event == "response.output_item.done")
                .count(),
            2,
            "{}",
            frames.join("")
        );
    }
}

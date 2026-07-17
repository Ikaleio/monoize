#[cfg(test)]
mod tests {
    use super::*;
    use crate::request_capture::with_sse_capture;
    use crate::urp::{FinishReason, Node, NodeDelta, NodeHeader, OrdinaryRole, UrpResponse};
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

    #[tokio::test]
    async fn responses_provider_control_filters_nested_internal_metadata_without_mutation() {
        let (event_tx, event_rx) = mpsc::channel(2);
        let (sse_tx, mut sse_rx) = mpsc::channel(8);
        let frames = Arc::new(Mutex::new(Vec::new()));
        let canonical = json!({
            "type": "response.vendor_control",
            "vendor": {
                "keep": 1,
                "_monoize_nested": "canonical",
                "rows": [
                    { "keep_row": true, "_monoize_row": "canonical" },
                    [{ "keep_deep": 2, "_monoize_deep": "canonical" }]
                ]
            },
            "_monoize_top": "canonical"
        });

        event_tx
            .send(UrpStreamEvent::ProviderControl {
                protocol: "responses".to_string(),
                event_name: "response.vendor_control".to_string(),
                data: canonical.clone(),
                extra_body: HashMap::new(),
            })
            .await
            .expect("provider control");
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
            .expect("encode Responses provider control");
        })
        .await;
        while sse_rx.recv().await.is_some() {}

        let frames = frames.lock().await;
        let json_frames = captured_responses_json_frames(&frames);
        let replay = json_frames
            .iter()
            .find(|(event, _)| event == "response.vendor_control")
            .map(|(_, payload)| payload)
            .expect("provider control replay");
        assert_eq!(
            replay["vendor"],
            json!({
                "keep": 1,
                "rows": [{ "keep_row": true }, [{ "keep_deep": 2 }]]
            })
        );
        assert!(replay.get("_monoize_top").is_none());
        assert_eq!(canonical["_monoize_top"], json!("canonical"));
        assert_eq!(
            canonical["vendor"]["rows"][1][0]["_monoize_deep"],
            json!("canonical")
        );
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
                    tool_type: urp::ToolCallType::Function,
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
                    tool_type: urp::ToolCallType::Function,
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

    #[tokio::test]
    async fn responses_stream_terminal_usage_preserves_nested_unknown_details_and_typed_counters_win()
    {
        let (event_tx, event_rx) = mpsc::channel(4);
        let (sse_tx, mut sse_rx) = mpsc::channel(8);
        let frames = Arc::new(Mutex::new(Vec::new()));

        event_tx
            .send(UrpStreamEvent::ResponseStart {
                id: "resp_nested_usage".to_string(),
                model: "gpt-5.4".to_string(),
                extra_body: HashMap::new(),
            })
            .await
            .expect("response start");
        event_tx
            .send(UrpStreamEvent::ResponseDone {
                finish_reason: Some(FinishReason::Stop),
                usage: Some(urp::Usage {
                    input_tokens: 12,
                    output_tokens: 8,
                    input_details: Some(urp::InputDetails {
                        cache_read_tokens: 3,
                        cache_creation_tokens: 4,
                        tool_prompt_tokens: 2,
                        ..urp::InputDetails::default()
                    }),
                    output_details: Some(urp::OutputDetails {
                        reasoning_tokens: 5,
                        accepted_prediction_tokens: 6,
                        rejected_prediction_tokens: 7,
                        ..urp::OutputDetails::default()
                    }),
                    extra_body: HashMap::from([
                        (
                            "input_tokens_details".to_string(),
                            json!({
                                "cached_tokens": 999,
                                "cache_write_tokens": 999,
                                "cache_creation_tokens": 999,
                                "tool_prompt_tokens": 999,
                                "future_input_detail": { "kind": "warm" },
                                "_monoize_hidden": true
                            }),
                        ),
                        (
                            "output_tokens_details".to_string(),
                            json!({
                                "reasoning_tokens": 999,
                                "accepted_prediction_tokens": 999,
                                "rejected_prediction_tokens": 999,
                                "future_output_detail": [1, 2]
                            }),
                        ),
                    ]),
                }),
                output: Vec::new(),
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
        let usage = json_frames
            .iter()
            .find(|(event, _)| event == "response.completed")
            .and_then(|(_, payload)| payload.get("response"))
            .and_then(|response| response.get("usage"))
            .expect("terminal Responses usage");
        assert_eq!(usage["input_tokens"], json!(12));
        assert_eq!(usage["output_tokens"], json!(8));
        assert_eq!(usage["total_tokens"], json!(20));
        assert_eq!(usage["input_tokens_details"]["cached_tokens"], json!(3));
        assert_eq!(
            usage["input_tokens_details"]["cache_write_tokens"],
            json!(4)
        );
        assert_eq!(
            usage["input_tokens_details"]["cache_creation_tokens"],
            json!(4)
        );
        assert_eq!(
            usage["input_tokens_details"]["tool_prompt_tokens"],
            json!(2)
        );
        assert_eq!(
            usage["input_tokens_details"]["future_input_detail"],
            json!({ "kind": "warm" })
        );
        assert!(
            usage["input_tokens_details"]
                .get("_monoize_hidden")
                .is_none()
        );
        assert_eq!(
            usage["output_tokens_details"]["reasoning_tokens"],
            json!(5)
        );
        assert_eq!(
            usage["output_tokens_details"]["accepted_prediction_tokens"],
            json!(6)
        );
        assert_eq!(
            usage["output_tokens_details"]["rejected_prediction_tokens"],
            json!(7)
        );
        assert_eq!(
            usage["output_tokens_details"]["future_output_detail"],
            json!([1, 2])
        );
    }

    async fn send_completed_media_node(
        event_tx: &mpsc::Sender<UrpStreamEvent>,
        node_index: u32,
        node: Node,
    ) {
        let (header, extra_body) = match &node {
            Node::Image {
                id,
                role,
                extra_body,
                ..
            } => (
                NodeHeader::Image {
                    id: id.clone(),
                    role: *role,
                },
                extra_body.clone(),
            ),
            Node::File {
                id,
                role,
                extra_body,
                ..
            } => (
                NodeHeader::File {
                    id: id.clone(),
                    role: *role,
                },
                extra_body.clone(),
            ),
            _ => panic!("media helper requires an image or file node"),
        };
        event_tx
            .send(UrpStreamEvent::NodeStart {
                node_index,
                header,
                extra_body,
            })
            .await
            .expect("media node start");
        event_tx
            .send(UrpStreamEvent::NodeDone {
                node_index,
                node,
                usage: None,
                extra_body: HashMap::new(),
            })
            .await
            .expect("media node done");
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
                tool_type: urp::ToolCallType::Function,
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
            message_allocation_deferred: false,
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
    async fn custom_tool_call_input_delta_and_done_encode() {
        let (event_tx, event_rx) = mpsc::channel(16);
        let (sse_tx, mut sse_rx) = mpsc::channel(64);
        let frames = Arc::new(Mutex::new(Vec::new()));
        let node = Node::ToolCall {
            id: Some("ctc_1".to_string()),
            tool_type: urp::ToolCallType::Custom,
            call_id: "call_custom".to_string(),
            name: "grammar".to_string(),
            arguments: "SELECT 4".to_string(),
            extra_body: HashMap::new(),
        };

        event_tx
            .send(UrpStreamEvent::ResponseStart {
                id: "resp_custom".to_string(),
                model: "gpt-5.4".to_string(),
                extra_body: HashMap::new(),
            })
            .await
            .unwrap();
        event_tx
            .send(UrpStreamEvent::NodeStart {
                node_index: 0,
                header: NodeHeader::ToolCall {
                    id: Some("ctc_1".to_string()),
                    tool_type: urp::ToolCallType::Custom,
                    call_id: "call_custom".to_string(),
                    name: "grammar".to_string(),
                },
                extra_body: HashMap::new(),
            })
            .await
            .unwrap();
        event_tx
            .send(UrpStreamEvent::NodeDelta {
                node_index: 0,
                delta: NodeDelta::ToolCallArguments {
                    arguments: "SELECT 4".to_string(),
                },
                usage: None,
                extra_body: HashMap::new(),
            })
            .await
            .unwrap();
        event_tx
            .send(UrpStreamEvent::NodeDone {
                node_index: 0,
                node: node.clone(),
                usage: None,
                extra_body: HashMap::new(),
            })
            .await
            .unwrap();
        event_tx
            .send(UrpStreamEvent::ResponseDone {
                finish_reason: Some(FinishReason::ToolCalls),
                usage: None,
                output: vec![node],
                extra_body: HashMap::new(),
            })
            .await
            .unwrap();
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
            .expect("encode custom Responses stream");
        })
        .await;
        while sse_rx.recv().await.is_some() {}

        let frames = frames.lock().await;
        let json_frames = captured_responses_json_frames(&frames);
        let added = json_frames
            .iter()
            .find(|(event, _)| event == "response.output_item.added")
            .map(|(_, payload)| payload)
            .expect("custom output item added");
        assert_eq!(added["item"]["type"], json!("custom_tool_call"));
        assert!(json_frames.iter().any(|(event, payload)| {
            event == "response.custom_tool_call_input.delta"
                && payload["delta"] == json!("SELECT 4")
        }));
        assert!(json_frames.iter().any(|(event, payload)| {
            event == "response.custom_tool_call_input.done"
                && payload["input"] == json!("SELECT 4")
        }));
        assert!(!json_frames.iter().any(|(event, _)| {
            matches!(
                event.as_str(),
                "response.function_call_arguments.delta"
                    | "response.function_call_arguments.done"
            )
        }));
        let completed = json_frames
            .iter()
            .find(|(event, _)| event == "response.completed")
            .map(|(_, payload)| payload)
            .expect("response.completed");
        assert_eq!(
            completed["response"]["output"][0]["type"],
            json!("custom_tool_call")
        );
        assert_eq!(
            completed["response"]["output"][0]["input"],
            json!("SELECT 4")
        );
    }

    #[tokio::test]
    async fn responses_stream_omits_non_openai_file_ids_from_every_lifecycle_frame() {
        fn origin_extra(origin: &str) -> HashMap<String, Value> {
            HashMap::from([(
                urp::FILE_ID_ORIGIN_EXTRA_KEY.to_string(),
                json!(origin),
            )])
        }

        let messages_image = Node::Image {
            id: Some("msg_img_node".to_string()),
            role: OrdinaryRole::Assistant,
            source: urp::ImageSource::FileId {
                file_id: "anthropic_img_secret".to_string(),
                detail: None,
            },
            extra_body: origin_extra(urp::FILE_ID_ORIGIN_MESSAGES),
        };
        let messages_file = Node::File {
            id: Some("msg_file_node".to_string()),
            role: OrdinaryRole::Assistant,
            source: urp::FileSource::FileId {
                file_id: "anthropic_file_secret".to_string(),
            },
            extra_body: origin_extra(urp::FILE_ID_ORIGIN_MESSAGES),
        };
        let unscoped_file = Node::File {
            id: Some("unscoped_file_node".to_string()),
            role: OrdinaryRole::Assistant,
            source: urp::FileSource::FileId {
                file_id: "unscoped_file_secret".to_string(),
            },
            extra_body: HashMap::new(),
        };
        let openai_image = Node::Image {
            id: Some("openai_img_node".to_string()),
            role: OrdinaryRole::Assistant,
            source: urp::ImageSource::FileId {
                file_id: "openai_img_ok".to_string(),
                detail: Some("high".to_string()),
            },
            extra_body: origin_extra(urp::FILE_ID_ORIGIN_OPENAI),
        };
        let openai_file = Node::File {
            id: Some("openai_file_node".to_string()),
            role: OrdinaryRole::Assistant,
            source: urp::FileSource::FileId {
                file_id: "openai_file_ok".to_string(),
            },
            extra_body: origin_extra(urp::FILE_ID_ORIGIN_OPENAI),
        };
        let tool_result = Node::ToolResult {
            id: Some("fco_1".to_string()),
            tool_type: urp::ToolCallType::Function,
            call_id: "call_1".to_string(),
            content: vec![
                urp::ToolResultContent::Text {
                    text: "visible result".to_string(),
                    extra_body: HashMap::new(),
                },
                urp::ToolResultContent::File {
                    source: urp::FileSource::FileId {
                        file_id: "anthropic_tool_file_secret".to_string(),
                    },
                    extra_body: origin_extra(urp::FILE_ID_ORIGIN_MESSAGES),
                },
            ],
            is_error: false,
            extra_body: HashMap::new(),
        };
        let output = vec![
            messages_image.clone(),
            messages_file.clone(),
            unscoped_file.clone(),
            openai_image.clone(),
            openai_file.clone(),
            tool_result.clone(),
        ];

        let (event_tx, event_rx) = mpsc::channel(32);
        let (sse_tx, mut sse_rx) = mpsc::channel(256);
        let frames = Arc::new(Mutex::new(Vec::new()));
        event_tx
            .send(UrpStreamEvent::ResponseStart {
                id: "resp_file_origin".to_string(),
                model: "gpt-5.4".to_string(),
                extra_body: HashMap::new(),
            })
            .await
            .unwrap();
        for (node_index, node) in [
            messages_image,
            messages_file,
            unscoped_file,
            openai_image,
            openai_file,
        ]
        .into_iter()
        .enumerate()
        {
            send_completed_media_node(&event_tx, node_index as u32, node).await;
        }
        event_tx
            .send(UrpStreamEvent::NodeStart {
                node_index: 5,
                header: NodeHeader::ToolResult {
                    id: Some("fco_1".to_string()),
                    tool_type: urp::ToolCallType::Function,
                    call_id: "call_1".to_string(),
                },
                extra_body: HashMap::new(),
            })
            .await
            .unwrap();
        event_tx
            .send(UrpStreamEvent::NodeDone {
                node_index: 5,
                node: tool_result,
                usage: None,
                extra_body: HashMap::new(),
            })
            .await
            .unwrap();
        event_tx
            .send(UrpStreamEvent::ResponseDone {
                finish_reason: Some(FinishReason::ToolCalls),
                usage: None,
                output,
                extra_body: HashMap::new(),
            })
            .await
            .unwrap();
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
            .expect("encode file provenance stream");
        })
        .await;
        while sse_rx.recv().await.is_some() {}

        let frames = frames.lock().await;
        let raw = frames.join("");
        for secret in [
            "anthropic_img_secret",
            "anthropic_file_secret",
            "unscoped_file_secret",
            "anthropic_tool_file_secret",
        ] {
            assert!(!raw.contains(secret), "leaked {secret}: {raw}");
        }
        assert!(raw.contains("openai_img_ok"), "{raw}");
        assert!(raw.contains("openai_file_ok"), "{raw}");

        let json_frames = captured_responses_json_frames(&frames);
        let content_done: Vec<&Value> = json_frames
            .iter()
            .filter(|(event, _)| event == "response.content_part.done")
            .map(|(_, payload)| payload)
            .collect();
        assert_eq!(content_done.len(), 2, "{raw}");
        assert_eq!(content_done[0]["content_index"], json!(0));
        assert_eq!(content_done[1]["content_index"], json!(1));
        let completed = json_frames
            .iter()
            .find(|(event, _)| event == "response.completed")
            .map(|(_, payload)| payload)
            .expect("response.completed");
        assert!(completed.to_string().contains("openai_img_ok"));
        assert!(completed.to_string().contains("openai_file_ok"));
        assert!(completed.to_string().contains("visible result"));
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
                    tool_type: urp::ToolCallType::Function,
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
                        tool_type: urp::ToolCallType::Function,
                        call_id: "call_b".to_string(),
                        name: "tool_b_replaced".to_string(),
                        arguments: "{\"version\":2}".to_string(),
                        extra_body: HashMap::new(),
                    },
                    Node::ToolCall {
                        id: Some("fc_a".to_string()),
                        tool_type: urp::ToolCallType::Function,
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

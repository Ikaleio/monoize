#[cfg(test)]
mod tests {
    use super::*;
    use crate::urp::{FinishReason, OrdinaryRole, UrpResponse};

    fn empty_map() -> HashMap<String, Value> {
        HashMap::new()
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
}

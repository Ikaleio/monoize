#[cfg(test)]
mod provider_item_tests {
    use super::*;

    fn empty_map() -> HashMap<String, Value> {
        HashMap::new()
    }

    fn request_with_input(input: Vec<Node>) -> UrpRequest {
        UrpRequest {
            model: "logical-model".to_string(),
            input,
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            response_format: None,
            user: None,
            extra_body: empty_map(),
        }
    }

    #[test]
    fn messages_provider_block_round_trips_only_for_messages_protocol() {
        let native_block = json!({
            "type": "server_tool_result",
            "payload": { "x": 1 }
        });
        let req = request_with_input(vec![Node::ProviderItem {
            id: None,
            origin_protocol: ProviderProtocol::Messages,
            role: OrdinaryRole::User,
            item_type: "server_tool_result".to_string(),
            body: native_block.clone(),
            extra_body: empty_map(),
        }]);

        let encoded = encode_request(&req, "claude-sonnet-4.5");
        assert_eq!(encoded["messages"][0]["content"][0], native_block);

        let cross_protocol_req = request_with_input(vec![Node::ProviderItem {
            id: Some("cmp_1".to_string()),
            origin_protocol: ProviderProtocol::Responses,
            role: OrdinaryRole::User,
            item_type: "compaction".to_string(),
            body: json!({
                "type": "compaction",
                "encrypted_content": "opaque"
            }),
            extra_body: empty_map(),
        }]);
        let cross_protocol = encode_request(&cross_protocol_req, "claude-sonnet-4.5");
        let wire = serde_json::to_string(&cross_protocol).expect("messages json");
        assert_eq!(cross_protocol["messages"], json!([]));
        assert!(!wire.contains("compaction"));
        assert!(!wire.contains("opaque"));
    }

    #[test]
    fn messages_provider_block_filters_nested_internal_metadata_on_wire() {
        let native_block = json!({
            "type": "server_tool_result",
            "payload": {
                "keep": 1,
                "_monoize_nested": "drop",
                "rows": [{ "keep_row": true, "_monoize_row": "drop" }]
            },
            "_monoize_top": "drop"
        });
        let req = request_with_input(vec![Node::ProviderItem {
            id: None,
            origin_protocol: ProviderProtocol::Messages,
            role: OrdinaryRole::User,
            item_type: "server_tool_result".to_string(),
            body: native_block.clone(),
            extra_body: empty_map(),
        }]);

        let encoded = encode_request(&req, "claude-sonnet-4.5");

        assert_eq!(
            encoded["messages"][0]["content"][0],
            json!({
                "type": "server_tool_result",
                "payload": { "keep": 1, "rows": [{ "keep_row": true }] }
            })
        );
        assert!(matches!(
            &req.input[0],
            Node::ProviderItem { body, .. } if body == &native_block
        ));
    }

    #[test]
    fn response_control_extra_applies_to_message_envelope_not_content_block() {
        let response = UrpResponse {
            id: "msg_1".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            created_at: None,
            output: vec![
                Node::NextDownstreamEnvelopeExtra {
                    extra_body: [("vendor_message".to_string(), json!({ "trace_id": "t1" }))]
                        .into_iter()
                        .collect(),
                },
                Node::Text {
                    id: None,
                    role: OrdinaryRole::Assistant,
                    content: "ok".to_string(),
                    phase: None,
                    extra_body: [("cache_control".to_string(), json!({ "type": "ephemeral" }))]
                        .into_iter()
                        .collect(),
                },
            ],
            finish_reason: Some(FinishReason::Stop),
            usage: None,
            extra_body: empty_map(),
        };

        let encoded = encode_response(&response, "claude-sonnet-4-6");
        assert_eq!(encoded["vendor_message"], json!({ "trace_id": "t1" }));
        let block = &encoded["content"][0];
        assert!(block.get("vendor_message").is_none());
        assert_eq!(block["cache_control"], json!({ "type": "ephemeral" }));
    }
}

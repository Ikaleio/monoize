#[cfg(test)]
mod tests {
    use super::*;
    use crate::urp::decode::openai_responses as decode_responses;
    use crate::urp::internal_legacy_bridge::{Item, Part, Role, items_to_nodes, nodes_to_items};
    use crate::urp::{
        CustomToolDefinition, FunctionDefinition, InputDetails, Node, OrdinaryRole, OutputDetails,
        Usage,
    };

    fn empty_map() -> HashMap<String, Value> {
        HashMap::new()
    }

    #[test]
    fn responses_provider_item_filters_nested_internal_metadata_on_wire() {
        let native_item = json!({
            "type": "compaction",
            "vendor_unknown": {
                "keep": 1,
                "_monoize_nested": "drop",
                "rows": [{ "keep_row": true, "_monoize_row": "drop" }]
            },
            "_monoize_top": "drop"
        });
        let req = UrpRequest {
            model: "gpt-5.4".to_string(),
            input: vec![Node::ProviderItem {
                id: Some("cmp_1".to_string()),
                origin_protocol: ProviderProtocol::Responses,
                role: OrdinaryRole::User,
                item_type: "compaction".to_string(),
                body: native_item.clone(),
                extra_body: empty_map(),
            }],
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            stop: None,
            verbosity: None,
            response_format: None,
            user: None,
            extra_body: empty_map(),
        };

        let encoded = encode_request(&req, "gpt-5.4");

        assert_eq!(
            encoded["input"][0],
            json!({
                "type": "compaction",
                "vendor_unknown": { "keep": 1, "rows": [{ "keep_row": true }] }
            })
        );
        assert!(matches!(
            &req.input[0],
            Node::ProviderItem { body, .. } if body == &native_item
        ));
    }

    #[test]
    fn responses_function_tool_preserves_extras() {
        let req = UrpRequest {
            model: "gpt-5.4".to_string(),
            input: items_to_nodes(vec![Item::Message {
                id: None,
                role: Role::User,
                parts: vec![Part::Text {
                    content: "hello".to_string(),
                    extra_body: empty_map(),
                }],
                extra_body: empty_map(),
            }]),
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: Some(vec![ToolDefinition {
                tool_type: "function".to_string(),
                name: None,
                description: None,
                function: Some(FunctionDefinition {
                    name: "lookup".to_string(),
                    description: Some("Lookup a value".to_string()),
                    parameters: Some(json!({
                        "type": "object",
                        "properties": {"id": {"type": "string"}},
                        "required": ["id"]
                    })),
                    strict: Some(true),
                    extra_body: HashMap::from([("defer_loading".to_string(), json!(true))]),
                }),
                custom: None,
                extra_body: HashMap::from([("x_vendor".to_string(), json!("ok"))]),
            }]),
            tool_choice: None,
            parallel_tool_calls: None,
            stop: None,
            verbosity: None,
            response_format: None,
            user: None,
            extra_body: empty_map(),
        };

        let encoded = encode_request(&req, "gpt-5.4");
        let tool = &encoded["tools"].as_array().expect("tools array")[0];
        assert_eq!(tool["type"], json!("function"));
        assert_eq!(tool["name"], json!("lookup"));
        assert_eq!(tool["description"], json!("Lookup a value"));
        assert_eq!(tool["strict"], json!(true));
        assert_eq!(
            tool["parameters"]["properties"]["id"]["type"],
            json!("string")
        );
        assert_eq!(tool["defer_loading"], json!(true));
        assert_eq!(tool["x_vendor"], json!("ok"));
    }

    #[test]
    fn tool_definition_extra_collision_semantic_fields_win() {
        let req = UrpRequest {
            model: "gpt-5.4".to_string(),
            input: items_to_nodes(vec![Item::Message {
                id: None,
                role: Role::User,
                parts: vec![Part::Text {
                    content: "hello".to_string(),
                    extra_body: empty_map(),
                }],
                extra_body: empty_map(),
            }]),
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: Some(vec![ToolDefinition {
                tool_type: "function".to_string(),
                name: None,
                description: None,
                function: Some(FunctionDefinition {
                    name: "tool_a".to_string(),
                    description: Some("semantic description".to_string()),
                    parameters: Some(json!({
                        "type": "object",
                        "properties": {"id": {"type": "string"}}
                    })),
                    strict: Some(true),
                    extra_body: HashMap::from([
                        ("type".to_string(), json!("wrong_function_type")),
                        ("name".to_string(), json!("wrong_function_name")),
                        ("parameters".to_string(), json!({"type": "array"})),
                        (
                            "description".to_string(),
                            json!("wrong function description"),
                        ),
                        ("strict".to_string(), json!(false)),
                        ("defer_loading".to_string(), json!(true)),
                    ]),
                }),
                custom: None,
                extra_body: HashMap::from([
                    ("type".to_string(), json!("wrong_tool_type")),
                    ("name".to_string(), json!("wrong_tool_name")),
                    ("parameters".to_string(), json!({"type": "null"})),
                    ("description".to_string(), json!("wrong tool description")),
                    ("strict".to_string(), json!(false)),
                    ("x_vendor".to_string(), json!("ok")),
                ]),
            }]),
            tool_choice: None,
            parallel_tool_calls: None,
            stop: None,
            verbosity: None,
            response_format: None,
            user: None,
            extra_body: empty_map(),
        };

        let encoded = encode_request(&req, "gpt-5.4");
        let tool = &encoded["tools"].as_array().expect("tools array")[0];
        assert_eq!(tool["type"], json!("function"));
        assert_eq!(tool["name"], json!("tool_a"));
        assert_eq!(tool["parameters"]["type"], json!("object"));
        assert_eq!(tool["description"], json!("semantic description"));
        assert_eq!(tool["strict"], json!(true));
        assert_eq!(tool["defer_loading"], json!(true));
        assert_eq!(tool["x_vendor"], json!("ok"));
    }

    #[test]
    fn responses_custom_tool_preserves_flat_fields() {
        let encoded = encode_tools(&[ToolDefinition {
            tool_type: "custom".to_string(),
            name: None,
            description: None,
            function: None,
            custom: Some(CustomToolDefinition {
                name: "freeform_lookup".to_string(),
                description: Some("Freeform lookup".to_string()),
                format: Some(json!({
                    "type": "grammar",
                    "syntax": "lark",
                    "definition": "start: /[a-z]+/"
                })),
                extra_body: HashMap::from([
                    ("defer_loading".to_string(), json!(true)),
                    ("x_custom".to_string(), json!("kept")),
                    ("name".to_string(), json!("wrong_extra_name")),
                    ("format".to_string(), json!({ "type": "text" })),
                ]),
            }),
            extra_body: HashMap::from([
                ("type".to_string(), json!("function")),
                ("description".to_string(), json!("wrong outer description")),
                ("x_tool".to_string(), json!("kept")),
            ]),
        }]);

        let tool = encoded.first().expect("custom tool");
        assert_eq!(tool["type"], json!("custom"));
        assert_eq!(tool["name"], json!("freeform_lookup"));
        assert_eq!(tool["description"], json!("Freeform lookup"));
        assert_eq!(tool["format"]["type"], json!("grammar"));
        assert_eq!(tool["defer_loading"], json!(true));
        assert_eq!(tool["x_custom"], json!("kept"));
        assert_eq!(tool["x_tool"], json!("kept"));
        assert!(
            tool.get("custom").is_none(),
            "Responses custom tools stay flat"
        );
    }

    #[test]
    fn responses_builtins_remain_native_and_preserve_config() {
        let encoded = encode_tools(&[
            ToolDefinition {
                tool_type: "file_search".to_string(),
                name: None,
                description: None,
                function: None,
                custom: None,
                extra_body: HashMap::from([(
                    "vector_store_ids".to_string(),
                    json!(["vs_1", "vs_2"]),
                )]),
            },
            ToolDefinition {
                tool_type: "code_interpreter".to_string(),
                name: None,
                description: None,
                function: None,
                custom: None,
                extra_body: HashMap::from([(
                    "container".to_string(),
                    json!({ "type": "auto", "file_ids": ["file_1"] }),
                )]),
            },
            ToolDefinition {
                tool_type: "web_search".to_string(),
                name: None,
                description: None,
                function: None,
                custom: None,
                extra_body: HashMap::from([
                    ("search_context_size".to_string(), json!("medium")),
                    (
                        "user_location".to_string(),
                        json!({ "type": "approximate", "country": "US" }),
                    ),
                ]),
            },
            ToolDefinition {
                tool_type: "mcp".to_string(),
                name: None,
                description: None,
                function: None,
                custom: None,
                extra_body: HashMap::from([
                    ("server_label".to_string(), json!("docs")),
                    ("server_url".to_string(), json!("https://mcp.example.test")),
                    ("allowed_tools".to_string(), json!(["search"])),
                    ("defer_loading".to_string(), json!(true)),
                ]),
            },
            ToolDefinition {
                tool_type: "namespace".to_string(),
                name: Some("app_tools".to_string()),
                description: Some("Application tools".to_string()),
                function: None,
                custom: None,
                extra_body: HashMap::from([(
                    "tools".to_string(),
                    json!([{ "name": "fetch_docs", "description": "Fetch docs" }]),
                )]),
            },
            ToolDefinition {
                tool_type: "tool_search".to_string(),
                name: None,
                description: Some("Discover tools".to_string()),
                function: None,
                custom: None,
                extra_body: HashMap::from([
                    ("execution".to_string(), json!("server")),
                    (
                        "parameters".to_string(),
                        json!({ "type": "object", "properties": {} }),
                    ),
                ]),
            },
            ToolDefinition {
                tool_type: "image_generation".to_string(),
                name: None,
                description: None,
                function: None,
                custom: None,
                extra_body: HashMap::from([("output_format".to_string(), json!("png"))]),
            },
        ]);

        for tool in &encoded {
            assert_ne!(
                tool["type"],
                json!("function"),
                "built-ins stay native: {tool}"
            );
            assert!(
                tool.get("function").is_none(),
                "built-ins are not wrapped: {tool}"
            );
        }
        assert_eq!(encoded[0]["vector_store_ids"], json!(["vs_1", "vs_2"]));
        assert_eq!(encoded[1]["container"]["file_ids"], json!(["file_1"]));
        assert_eq!(encoded[2]["search_context_size"], json!("medium"));
        assert_eq!(encoded[2]["user_location"]["country"], json!("US"));
        assert_eq!(encoded[3]["server_label"], json!("docs"));
        assert_eq!(encoded[3]["allowed_tools"], json!(["search"]));
        assert_eq!(encoded[4]["name"], json!("app_tools"));
        assert_eq!(encoded[4]["description"], json!("Application tools"));
        assert_eq!(encoded[4]["tools"][0]["name"], json!("fetch_docs"));
        assert_eq!(encoded[5]["description"], json!("Discover tools"));
        assert_eq!(encoded[5]["parameters"]["type"], json!("object"));
        assert_eq!(encoded[6]["type"], json!("image_generation"));
        assert_eq!(encoded[6]["output_format"], json!("png"));
    }

    #[test]
    fn encode_response_preserves_message_phase_and_order() {
        let resp = UrpResponse {
            id: "resp_1".to_string(),
            model: "gpt-5.4".to_string(),
            created_at: None,
            output: items_to_nodes(vec![Item::Message {
                id: None,
                role: Role::Assistant,
                parts: vec![
                    Part::Text {
                        content: "thinking".to_string(),
                        extra_body: {
                            let mut m = empty_map();
                            m.insert("phase".to_string(), json!("commentary"));
                            m
                        },
                    },
                    Part::ToolCall {
                        id: None,
                        tool_type: ToolCallType::Function,
                        call_id: "call_1".to_string(),
                        name: "tool_a".to_string(),
                        arguments: "{}".to_string(),
                        extra_body: empty_map(),
                    },
                    Part::Text {
                        content: "done".to_string(),
                        extra_body: {
                            let mut m = empty_map();
                            m.insert("phase".to_string(), json!("final_answer"));
                            m
                        },
                    },
                ],
                extra_body: {
                    let mut m = empty_map();
                    m.insert("custom_message_field".to_string(), json!(true));
                    m
                },
            }]),
            finish_reason: Some(FinishReason::ToolCalls),
            usage: None,
            extra_body: empty_map(),
        };

        let encoded = encode_response(&resp, "gpt-5.4");
        let output = encoded["output"].as_array().expect("output array");

        assert_eq!(output.len(), 3);
        assert_eq!(output[0]["type"], Value::String("message".to_string()));
        assert_eq!(output[0]["phase"], Value::String("commentary".to_string()));
        assert_eq!(output[0]["custom_message_field"], json!(true));
        assert_eq!(
            output[1]["type"],
            Value::String("function_call".to_string())
        );
        assert_eq!(output[2]["type"], Value::String("message".to_string()));
        assert_eq!(
            output[2]["phase"],
            Value::String("final_answer".to_string())
        );
    }

    #[test]
    fn responses_round_trip_keeps_phase_order_and_unknown_fields() {
        let source = json!({
            "id": "resp_1",
            "model": "gpt-5.4",
            "status": "completed",
            "output": [
                {
                    "type": "message",
                    "role": "assistant",
                    "phase": "commentary",
                    "custom_message_field": true,
                    "content": [{
                        "type": "output_text",
                        "text": "one",
                        "future_part_field": "part-only"
                    }]
                },
                {
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "tool_a",
                    "arguments": "{}"
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "phase": "final_answer",
                    "content": [{ "type": "output_text", "text": "two" }]
                }
            ]
        });

        let decoded = decode_responses::decode_response(&source).expect("decode response");
        let reencoded = encode_response(&decoded, "gpt-5.4");
        let output = reencoded["output"].as_array().expect("output array");

        assert_eq!(output.len(), 3);
        assert_eq!(output[0]["phase"], json!("commentary"));
        assert_eq!(output[0]["custom_message_field"], json!(true));
        assert!(output[0]["content"][0]
            .get("custom_message_field")
            .is_none());
        assert!(output[0].get("future_part_field").is_none());
        assert_eq!(
            output[0]["content"][0]["future_part_field"],
            json!("part-only")
        );
        assert_eq!(output[1]["type"], json!("function_call"));
        assert_eq!(output[2]["phase"], json!("final_answer"));
    }

    #[test]
    fn responses_round_trip_filters_internal_top_level_source_fields() {
        let source = json!({
            "id": "resp_internal_filter",
            "object": "response",
            "model": "gpt-5.4",
            "status": "completed",
            "_monoize_private": "must-not-leak",
            "public_extension": "preserved",
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": "ok" }]
            }]
        });

        let decoded = decode_responses::decode_response(&source).expect("decode response");
        let reencoded = encode_response(&decoded, "gpt-5.4");

        assert!(reencoded.get("_monoize_private").is_none());
        assert_eq!(reencoded["public_extension"], json!("preserved"));
    }

    #[test]
    fn responses_round_trip_content_content_boundary() {
        let source = json!({
            "id": "resp_cc",
            "model": "gpt-5.4",
            "status": "completed",
            "output": [
                {
                    "type": "reasoning",
                    "text": "hmm"
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "phase": "commentary",
                    "content": [{ "type": "output_text", "text": "phase A" }]
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "phase": "final_answer",
                    "content": [{ "type": "output_text", "text": "phase B" }]
                },
                {
                    "type": "function_call",
                    "call_id": "call_2",
                    "name": "tool_b",
                    "arguments": "{}"
                }
            ]
        });

        let decoded = decode_responses::decode_response(&source).expect("decode");
        assert_eq!(
            decoded.output.len(),
            4,
            "canonical flat output must preserve node order"
        );
        assert_eq!(
            nodes_to_items(&decoded.output).len(),
            2,
            "bridge regrouping must preserve the old 2-item assistant shape"
        );

        let reencoded = encode_response(&decoded, "gpt-5.4");
        let output = reencoded["output"].as_array().expect("output array");

        assert_eq!(output.len(), 4);
        assert_eq!(output[0]["type"], json!("reasoning"));
        assert_eq!(output[1]["type"], json!("message"));
        assert_eq!(output[1]["phase"], json!("commentary"));
        assert_eq!(output[2]["type"], json!("message"));
        assert_eq!(output[2]["phase"], json!("final_answer"));
        assert_eq!(output[3]["type"], json!("function_call"));
    }

    #[test]
    fn encode_request_keeps_phased_developer_message_as_input_message() {
        let req = UrpRequest {
            model: "gpt-5.4".to_string(),
            input: items_to_nodes(vec![Item::Message {
                id: None,
                role: Role::Developer,
                parts: vec![Part::Text {
                    content: "preface".to_string(),
                    extra_body: {
                        let mut m = empty_map();
                        m.insert("phase".to_string(), json!("commentary"));
                        m
                    },
                }],
                extra_body: empty_map(),
            }]),
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            stop: None,
            verbosity: None,
            response_format: None,
            user: None,
            extra_body: empty_map(),
        };

        let encoded = encode_request(&req, "gpt-5.4");
        assert!(encoded.get("instructions").is_none());
        assert_eq!(encoded["input"][0]["type"], json!("message"));
        assert_eq!(encoded["input"][0]["phase"], json!("commentary"));
    }

    #[test]
    fn encode_request_preserves_raw_cot_reasoning_during_tool_loop_replay() {
        let req = UrpRequest {
            model: "gpt-5.4".to_string(),
            input: items_to_nodes(vec![Item::Message {
                id: None,
                role: Role::Assistant,
                parts: vec![
                    Part::Reasoning {
                        id: Some("rs_signed".to_string()),
                        content: Some("signed think".to_string()),
                        encrypted: Some(json!("sig_1")),
                        summary: Some("signed summary".to_string()),
                        source: None,
                        extra_body: empty_map(),
                    },
                    Part::Reasoning {
                        id: None,
                        content: Some("plain think".to_string()),
                        encrypted: None,
                        summary: None,
                        source: None,
                        extra_body: empty_map(),
                    },
                    Part::ToolCall {
                        id: Some("fc_1".to_string()),
                        tool_type: ToolCallType::Function,
                        call_id: "call_1".to_string(),
                        name: "lookup".to_string(),
                        arguments: "{}".to_string(),
                        extra_body: empty_map(),
                    },
                ],
                extra_body: empty_map(),
            }]),
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            stop: None,
            verbosity: None,
            response_format: None,
            user: None,
            extra_body: empty_map(),
        };

        let encoded = encode_request(&req, "gpt-5.4");
        let input = encoded["input"].as_array().expect("input array");
        assert_eq!(input.len(), 3);
        assert_eq!(input[0]["type"], json!("reasoning"));
        assert_eq!(input[0]["encrypted_content"], json!("sig_1"));
        assert!(input[0].get("text").is_none());
        assert_eq!(
            input[0]["summary"],
            json!([{ "type": "summary_text", "text": "signed summary" }])
        );
        assert!(input[0].get("source").is_none());
        assert_eq!(input[1]["type"], json!("reasoning"));
        assert!(input[1].get("id").is_none());
        assert!(input[1].get("text").is_none());
        assert_eq!(input[1]["summary"], json!([]));
        assert_eq!(
            input[1]["content"],
            json!([{ "type": "reasoning_text", "text": "plain think" }])
        );
        assert!(input[1].get("source").is_none());
        assert_eq!(input[2]["type"], json!("function_call"));
        assert_eq!(input[2]["call_id"], json!("call_1"));
    }

    #[test]
    fn encode_request_drops_encrypted_reasoning_without_stable_item_id() {
        let req = UrpRequest {
            model: "gpt-5.4".to_string(),
            input: items_to_nodes(vec![Item::Message {
                id: None,
                role: Role::Assistant,
                parts: vec![
                    Part::Reasoning {
                        id: None,
                        content: None,
                        encrypted: Some(json!("encrypted_without_bound_item_id")),
                        summary: None,
                        source: None,
                        extra_body: empty_map(),
                    },
                    Part::Text {
                        content: "prior answer".to_string(),
                        extra_body: empty_map(),
                    },
                ],
                extra_body: empty_map(),
            }]),
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            stop: None,
            verbosity: None,
            response_format: None,
            user: None,
            extra_body: empty_map(),
        };

        let encoded = encode_request(&req, "gpt-5.4");
        let input = encoded["input"].as_array().expect("input array");
        assert!(
            input
                .iter()
                .all(|item| item.get("type").and_then(Value::as_str) != Some("reasoning")),
            "encrypted reasoning replay without a stable item id must not be sent upstream"
        );
        assert!(
            input.iter().any(|item| {
                item.get("type").and_then(Value::as_str) == Some("message")
                    && item.get("role").and_then(Value::as_str) == Some("assistant")
            }),
            "dropping the unsafe reasoning item must not drop adjacent assistant text"
        );
    }

    #[test]
    fn encode_request_uses_output_text_blocks_for_assistant_messages() {
        let req = UrpRequest {
            model: "gpt-5.4".to_string(),
            input: items_to_nodes(vec![
                Item::Message {
                    id: None,
                    role: Role::User,
                    parts: vec![Part::Text {
                        content: "question".to_string(),
                        extra_body: empty_map(),
                    }],
                    extra_body: empty_map(),
                },
                Item::Message {
                    id: None,
                    role: Role::Assistant,
                    parts: vec![Part::Text {
                        content: "commentary".to_string(),
                        extra_body: empty_map(),
                    }],
                    extra_body: empty_map(),
                },
            ]),
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            stop: None,
            verbosity: None,
            response_format: None,
            user: None,
            extra_body: empty_map(),
        };

        let encoded = encode_request(&req, "gpt-5.4");
        let input = encoded["input"].as_array().expect("input array");
        assert_eq!(input[0]["role"], json!("user"));
        assert_eq!(input[0]["content"][0]["type"], json!("input_text"));
        assert_eq!(input[1]["role"], json!("assistant"));
        assert_eq!(input[1]["content"][0]["type"], json!("output_text"));
    }

    #[test]
    fn encode_request_does_not_default_responses_reasoning_summary() {
        let req = UrpRequest {
            model: "gpt-5.4".to_string(),
            input: items_to_nodes(vec![Item::Message {
                id: None,
                role: Role::User,
                parts: vec![Part::Text {
                    content: "hello".to_string(),
                    extra_body: empty_map(),
                }],
                extra_body: empty_map(),
            }]),
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            stop: None,
            verbosity: None,
            response_format: None,
            user: None,
            extra_body: empty_map(),
        };

        let encoded = encode_request(&req, "gpt-5.4");
        assert!(encoded.get("reasoning").is_none());
        assert_eq!(
            encoded["include"],
            json!(["reasoning.encrypted_content"])
        );
    }

    #[test]
    fn encode_request_preserves_explicit_responses_reasoning_summary() {
        let req = UrpRequest {
            model: "gpt-5.4".to_string(),
            input: items_to_nodes(vec![Item::Message {
                id: None,
                role: Role::User,
                parts: vec![Part::Text {
                    content: "hello".to_string(),
                    extra_body: empty_map(),
                }],
                extra_body: empty_map(),
            }]),
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: Some(crate::urp::ReasoningConfig {
                effort: Some("high".to_string()),
                extra_body: HashMap::from([
                    ("summary".to_string(), json!("concise")),
                    (
                        crate::urp::MESSAGES_THINKING_CONFIG_EXTRA_KEY.to_string(),
                        json!({ "type": "adaptive" }),
                    ),
                    (
                        crate::urp::CHAT_REASONING_CONFIG_EXTRA_KEY.to_string(),
                        json!({ "effort": "high" }),
                    ),
                ]),
            }),
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            stop: None,
            verbosity: None,
            response_format: None,
            user: None,
            extra_body: empty_map(),
        };

        let encoded = encode_request(&req, "gpt-5.4");
        assert_eq!(encoded["reasoning"]["effort"], json!("high"));
        assert_eq!(encoded["reasoning"]["summary"], json!("concise"));
        assert!(
            encoded["reasoning"]
                .get(crate::urp::MESSAGES_THINKING_CONFIG_EXTRA_KEY)
                .is_none()
        );
        assert!(
            encoded["reasoning"]
                .get(crate::urp::CHAT_REASONING_CONFIG_EXTRA_KEY)
                .is_none()
        );
    }

    #[test]
    fn responses_usage_round_trips_all_typed_usage_fields_without_detail_leakage() {
        let mut usage_extra = HashMap::new();
        usage_extra.insert("upstream_counter".to_string(), json!(42));
        let response = UrpResponse {
            id: "resp_usage".to_string(),
            model: "gpt-5.4".to_string(),
            created_at: None,
            output: items_to_nodes(vec![Item::new_message(Role::Assistant)]),
            finish_reason: Some(FinishReason::Stop),
            usage: Some(Usage {
                input_tokens: 30,
                output_tokens: 12,
                input_details: Some(InputDetails {
                    standard_tokens: 0,
                    cache_read_tokens: 2,
                    cache_read_modality_breakdown: None,
                    cache_creation_tokens: 3,
                    cache_creation_5m_tokens: 0,
                    cache_creation_1h_tokens: 0,
                    tool_prompt_tokens: 4,
                    modality_breakdown: None,
                }),
                output_details: Some(OutputDetails {
                    standard_tokens: 0,
                    reasoning_tokens: 5,
                    accepted_prediction_tokens: 6,
                    rejected_prediction_tokens: 7,
                    modality_breakdown: None,
                }),
                extra_body: usage_extra,
            }),
            extra_body: empty_map(),
        };

        let encoded = encode_response(&response, "gpt-5.4");
        assert_eq!(
            encoded["usage"]["input_tokens_details"]["cached_tokens"],
            json!(2)
        );
        assert_eq!(
            encoded["usage"]["input_tokens_details"]["cache_creation_tokens"],
            json!(3)
        );
        assert_eq!(
            encoded["usage"]["input_tokens_details"]["tool_prompt_tokens"],
            json!(4)
        );
        assert_eq!(
            encoded["usage"]["output_tokens_details"]["reasoning_tokens"],
            json!(5)
        );
        assert_eq!(
            encoded["usage"]["output_tokens_details"]["accepted_prediction_tokens"],
            json!(6)
        );
        assert_eq!(
            encoded["usage"]["output_tokens_details"]["rejected_prediction_tokens"],
            json!(7)
        );

        let decoded = decode_responses::decode_response(&encoded).expect("decode response");
        let decoded_usage = decoded.usage.expect("usage should decode");
        let input = decoded_usage.input_details.expect("input details");
        let output = decoded_usage.output_details.expect("output details");
        assert_eq!(input.cache_read_tokens, 2);
        assert_eq!(input.cache_creation_tokens, 3);
        assert_eq!(input.tool_prompt_tokens, 4);
        assert_eq!(output.reasoning_tokens, 5);
        assert_eq!(output.accepted_prediction_tokens, 6);
        assert_eq!(output.rejected_prediction_tokens, 7);
        assert!(
            !decoded_usage
                .extra_body
                .contains_key("input_tokens_details")
        );
        assert!(
            !decoded_usage
                .extra_body
                .contains_key("output_tokens_details")
        );
        assert_eq!(
            decoded_usage.extra_body.get("upstream_counter"),
            Some(&json!(42))
        );
    }

    #[test]
    fn responses_usage_nested_extra_preserves_siblings_and_typed_counters_win() {
        let response = UrpResponse {
            id: "resp_nested_usage".to_string(),
            model: "gpt-5.4".to_string(),
            created_at: None,
            output: Vec::new(),
            finish_reason: Some(FinishReason::Stop),
            usage: Some(Usage {
                input_tokens: 14,
                output_tokens: 9,
                input_details: Some(InputDetails {
                    cache_read_tokens: 4,
                    ..InputDetails::default()
                }),
                output_details: Some(OutputDetails {
                    reasoning_tokens: 6,
                    ..OutputDetails::default()
                }),
                extra_body: HashMap::from([
                    (
                        "input_tokens_details".to_string(),
                        json!({
                            "cached_tokens": 999,
                            "vendor_input_detail": { "kind": "warm" },
                            "_monoize_hidden": true
                        }),
                    ),
                    (
                        "output_tokens_details".to_string(),
                        json!({
                            "reasoning_tokens": 999,
                            "vendor_output_detail": [3, 4]
                        }),
                    ),
                ]),
            }),
            extra_body: empty_map(),
        };

        let encoded = encode_response(&response, "gpt-5.4");
        assert_eq!(
            encoded["usage"]["input_tokens_details"],
            json!({
                "cached_tokens": 4,
                "cache_write_tokens": 0,
                "cache_creation_tokens": 0,
                "tool_prompt_tokens": 0,
                "vendor_input_detail": { "kind": "warm" }
            })
        );
        assert_eq!(
            encoded["usage"]["output_tokens_details"],
            json!({
                "reasoning_tokens": 6,
                "accepted_prediction_tokens": 0,
                "rejected_prediction_tokens": 0,
                "vendor_output_detail": [3, 4]
            })
        );
    }

    #[test]
    fn responses_response_round_trip_preserves_reasoning_summary_separately_from_content() {
        let response = UrpResponse {
            id: "resp_roundtrip_reasoning".to_string(),
            model: "gpt-5.4".to_string(),
            created_at: None,
            output: items_to_nodes(vec![Item::Message {
                id: None,
                role: Role::Assistant,
                parts: vec![Part::Reasoning {
                    id: None,
                    content: Some("full reasoning".to_string()),
                    encrypted: Some(json!("sig_1")),
                    summary: Some("brief summary".to_string()),
                    source: None,
                    extra_body: empty_map(),
                }],
                extra_body: empty_map(),
            }]),
            finish_reason: Some(FinishReason::Stop),
            usage: None,
            extra_body: empty_map(),
        };

        let encoded = encode_response(&response, "gpt-5.4");
        let reasoning_item = encoded["output"]
            .as_array()
            .and_then(|items| items.first())
            .expect("reasoning output item");
        assert!(reasoning_item.get("status").is_none());
        assert!(reasoning_item.get("text").is_none());
        assert_eq!(
            reasoning_item["content"],
            json!([{ "type": "reasoning_text", "text": "full reasoning" }])
        );
        assert_eq!(reasoning_item["encrypted_content"].as_str(), Some("sig_1"));

        let decoded = decode_responses::decode_response(&encoded).expect("decode response");
        let decoded_outputs = nodes_to_items(&decoded.output);
        let Item::Message { parts, .. } = &decoded_outputs[0] else {
            panic!("expected assistant output");
        };

        assert!(matches!(
            &parts[0],
            Part::Reasoning {
                content: Some(content),
                summary: Some(summary),
                encrypted: Some(Value::String(sig)),
                ..
            } if content == "full reasoning" && summary == "brief summary" && sig == "sig_1"
        ));
    }

    #[test]
    fn responses_response_round_trip_does_not_invent_summary_from_plain_reasoning_content() {
        let response = UrpResponse {
            id: "resp_roundtrip_plain_reasoning".to_string(),
            model: "gpt-5.4".to_string(),
            created_at: None,
            output: items_to_nodes(vec![Item::Message {
                id: None,
                role: Role::Assistant,
                parts: vec![Part::Reasoning {
                    id: None,
                    content: Some("plain reasoning".to_string()),
                    encrypted: None,
                    summary: None,
                    source: None,
                    extra_body: empty_map(),
                }],
                extra_body: empty_map(),
            }]),
            finish_reason: Some(FinishReason::Stop),
            usage: None,
            extra_body: empty_map(),
        };

        let encoded = encode_response(&response, "gpt-5.4");
        let reasoning_item = encoded["output"]
            .as_array()
            .and_then(|items| items.first())
            .expect("reasoning output item");
        assert_eq!(
            reasoning_item["summary"].as_array().map(|a| a.len()),
            Some(0)
        );
        assert!(reasoning_item.get("text").is_none());
        assert_eq!(
            reasoning_item["content"],
            json!([{ "type": "reasoning_text", "text": "plain reasoning" }])
        );

        let decoded = decode_responses::decode_response(&encoded).expect("decode response");
        let decoded_outputs = nodes_to_items(&decoded.output);
        let Item::Message { parts, .. } = &decoded_outputs[0] else {
            panic!("expected assistant output");
        };
        assert!(matches!(
            &parts[0],
            Part::Reasoning {
                content: Some(content),
                summary: None,
                encrypted: None,
                ..
            } if content == "plain reasoning"
        ));
    }

    #[test]
    fn responses_response_omits_reasoning_without_meaningful_payload() {
        let response = UrpResponse {
            id: "resp_empty_reasoning".to_string(),
            model: "gpt-5.6-sol".to_string(),
            created_at: None,
            output: items_to_nodes(vec![Item::Message {
                id: None,
                role: Role::Assistant,
                parts: vec![
                    Part::Reasoning {
                        id: Some("rs_empty".to_string()),
                        content: Some(String::new()),
                        encrypted: None,
                        summary: Some(String::new()),
                        source: None,
                        extra_body: empty_map(),
                    },
                    Part::Text {
                        content: "answer".to_string(),
                        extra_body: empty_map(),
                    },
                ],
                extra_body: empty_map(),
            }]),
            finish_reason: Some(FinishReason::Stop),
            usage: None,
            extra_body: empty_map(),
        };

        let encoded = encode_response(&response, "gpt-5.6-sol");
        let output = encoded["output"].as_array().expect("response output");
        assert_eq!(output.len(), 1);
        assert_eq!(output[0]["type"], json!("message"));
        assert_eq!(output[0]["content"][0]["text"], json!("answer"));
    }

    #[test]
    fn encode_request_keeps_json_object_response_format() {
        let req = UrpRequest {
            model: "gpt-5.4".to_string(),
            input: items_to_nodes(vec![Item::Message {
                id: None,
                role: Role::User,
                parts: vec![Part::Text {
                    content: "hello".to_string(),
                    extra_body: empty_map(),
                }],
                extra_body: empty_map(),
            }]),
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            stop: None,
            verbosity: None,
            response_format: Some(ResponseFormat::JsonObject),
            user: None,
            extra_body: empty_map(),
        };

        let encoded = encode_request(&req, "gpt-5.4");
        assert_eq!(encoded["text"]["format"]["type"], json!("json_object"));
        assert!(encoded["text"]["format"].get("schema").is_none());
        assert!(encoded["text"]["format"].get("name").is_none());
    }
}

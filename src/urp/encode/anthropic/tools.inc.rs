#[cfg(test)]
mod tests {
    use super::*;
    use crate::urp::decode::anthropic as decode_anthropic;
    use crate::urp::decode::parse_tool_definition;
    use crate::urp::internal_legacy_bridge::{Item, Part, Role, items_to_nodes, nodes_to_items};
    use crate::urp::{
        FunctionDefinition, OutputDetails, ResponseFormat, UrpRequest, UrpResponse, Usage,
    };
    use std::collections::HashMap;

    fn empty_map() -> HashMap<String, Value> {
        HashMap::new()
    }

    #[test]
    fn anthropic_function_tool_preserves_extras_and_strict() {
        let mut function_extra = HashMap::new();
        function_extra.insert("cache_control".to_string(), json!({ "type": "ephemeral" }));
        function_extra.insert("defer_loading".to_string(), json!(true));

        let mut tool_extra = HashMap::new();
        tool_extra.insert(
            "input_examples".to_string(),
            json!([{ "location": "Paris" }]),
        );
        tool_extra.insert(
            "allowed_callers".to_string(),
            json!(["code_execution_20260120"]),
        );
        tool_extra.insert("eager_input_streaming".to_string(), json!(true));

        let req = UrpRequest {
            model: "claude-sonnet-4-5".to_string(),
            input: items_to_nodes(vec![Item::text(Role::User, "weather")]),
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
                    name: "get_weather".to_string(),
                    description: Some("Get weather".to_string()),
                    parameters: Some(json!({
                        "type": "object",
                        "properties": {
                            "location": { "type": "string" }
                        },
                        "required": ["location"],
                        "additionalProperties": false
                    })),
                    strict: Some(true),
                    extra_body: function_extra,
                }),
                custom: None,
                extra_body: tool_extra,
            }]),
            tool_choice: None,
            parallel_tool_calls: None,
            stop: None,
            verbosity: None,
            response_format: None,
            user: None,
            extra_body: empty_map(),
        };

        let encoded = encode_request(&req, "claude-sonnet-4-5");
        let tool = &encoded["tools"].as_array().expect("tools array")[0];

        assert_eq!(tool["name"], json!("get_weather"));
        assert_eq!(tool["description"], json!("Get weather"));
        assert_eq!(tool["strict"], json!(true));
        assert_eq!(tool["cache_control"], json!({ "type": "ephemeral" }));
        assert_eq!(tool["defer_loading"], json!(true));
        assert_eq!(tool["input_examples"], json!([{ "location": "Paris" }]));
        assert_eq!(tool["allowed_callers"], json!(["code_execution_20260120"]));
        assert_eq!(tool["eager_input_streaming"], json!(true));

        let input_schema = tool["input_schema"].as_object().expect("input schema");
        for key in [
            "cache_control",
            "defer_loading",
            "input_examples",
            "allowed_callers",
            "eager_input_streaming",
        ] {
            assert!(
                !input_schema.contains_key(key),
                "{key} must stay on the Anthropic tool object"
            );
        }
    }

    #[test]
    fn anthropic_tool_extra_layering_is_stable() {
        let mut function_extra = HashMap::new();
        function_extra.insert("name".to_string(), json!("bad_function_name"));
        function_extra.insert("description".to_string(), json!("bad function description"));
        function_extra.insert("input_schema".to_string(), json!({ "type": "string" }));
        function_extra.insert("strict".to_string(), json!(false));
        function_extra.insert("cache_control".to_string(), json!({ "type": "ephemeral" }));

        let mut tool_extra = HashMap::new();
        tool_extra.insert("name".to_string(), json!("bad_tool_name"));
        tool_extra.insert("description".to_string(), json!("bad tool description"));
        tool_extra.insert("input_schema".to_string(), json!({ "type": "array" }));
        tool_extra.insert("strict".to_string(), json!(true));
        tool_extra.insert("cache_control".to_string(), json!({ "type": "tool" }));
        tool_extra.insert("input_examples".to_string(), json!([{ "city": "Berlin" }]));

        let req = UrpRequest {
            model: "claude-sonnet-4-5".to_string(),
            input: items_to_nodes(vec![Item::text(Role::User, "weather")]),
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
                    name: "stable_weather".to_string(),
                    description: Some("Stable weather".to_string()),
                    parameters: Some(json!({
                        "type": "object",
                        "properties": {
                            "city": { "type": "string" }
                        },
                        "additionalProperties": false
                    })),
                    strict: Some(false),
                    extra_body: function_extra,
                }),
                custom: None,
                extra_body: tool_extra,
            }]),
            tool_choice: None,
            parallel_tool_calls: None,
            stop: None,
            verbosity: None,
            response_format: None,
            user: None,
            extra_body: empty_map(),
        };

        let encoded = encode_request(&req, "claude-sonnet-4-5");
        let tool = &encoded["tools"].as_array().expect("tools array")[0];

        assert_eq!(tool["name"], json!("stable_weather"));
        assert_eq!(tool["description"], json!("Stable weather"));
        assert_eq!(tool["input_schema"]["type"], json!("object"));
        assert_eq!(
            tool["input_schema"]["properties"]["city"]["type"],
            json!("string")
        );
        assert_eq!(tool["strict"], json!(false));
        assert_eq!(tool["cache_control"], json!({ "type": "ephemeral" }));
        assert_eq!(tool["input_examples"], json!([{ "city": "Berlin" }]));
        assert!(
            tool["input_schema"].get("cache_control").is_none(),
            "provider metadata must not be nested in input_schema"
        );
    }

    #[test]
    fn anthropic_tool_metadata_round_trips() {
        let raw_tool = json!({
            "type": "custom",
            "name": "structured_lookup",
            "description": "Structured lookup",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"],
                "additionalProperties": false
            },
            "strict": true,
            "cache_control": { "type": "ephemeral" },
            "defer_loading": true,
            "allowed_callers": ["code_execution_20260120"],
            "input_examples": [{ "query": "docs" }],
            "eager_input_streaming": true
        });
        let parsed_tool = parse_tool_definition(&raw_tool).expect("Anthropic custom tool");
        let req = UrpRequest {
            model: "claude-sonnet-4-5".to_string(),
            input: items_to_nodes(vec![Item::text(Role::User, "lookup")]),
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: Some(vec![parsed_tool]),
            tool_choice: None,
            parallel_tool_calls: None,
            stop: None,
            verbosity: None,
            response_format: None,
            user: None,
            extra_body: empty_map(),
        };

        let encoded = encode_request(&req, "claude-sonnet-4-5");
        let tool = &encoded["tools"].as_array().expect("tools array")[0];

        assert_eq!(tool["type"], json!("custom"));
        assert_eq!(tool["name"], json!("structured_lookup"));
        assert_eq!(tool["description"], json!("Structured lookup"));
        assert_eq!(tool["strict"], json!(true));
        assert_eq!(tool["cache_control"], json!({ "type": "ephemeral" }));
        assert_eq!(tool["defer_loading"], json!(true));
        assert_eq!(tool["allowed_callers"], json!(["code_execution_20260120"]));
        assert_eq!(tool["input_examples"], json!([{ "query": "docs" }]));
        assert_eq!(tool["eager_input_streaming"], json!(true));
        assert_eq!(
            tool["input_schema"]["properties"]["query"]["type"],
            json!("string")
        );

        let input_schema = tool["input_schema"]
            .as_object()
            .expect("input_schema object");
        for key in [
            "cache_control",
            "defer_loading",
            "allowed_callers",
            "input_examples",
            "eager_input_streaming",
        ] {
            assert!(
                !input_schema.contains_key(key),
                "{key} must stay on the Anthropic tool object"
            );
        }
    }

    #[test]
    fn anthropic_builtin_tool_stays_non_function() {
        let tools = [
            json!({
                "type": "computer_20251124",
                "name": "computer",
                "display_width_px": 1280,
                "display_height_px": 720,
                "display_number": 1,
                "enable_zoom": true
            }),
            json!({
                "type": "web_search_20260209",
                "name": "web_search",
                "max_uses": 3,
                "allowed_domains": ["example.com"],
                "user_location": {
                    "type": "approximate",
                    "country": "US",
                    "region": "CA",
                    "city": "San Francisco"
                }
            }),
            json!({
                "type": "mcp_toolset",
                "mcp_server_name": "docs",
                "default_config": { "enabled": true },
                "configs": {
                    "search": { "enabled": true, "defer_loading": true }
                },
                "cache_control": { "type": "ephemeral" }
            }),
        ]
        .into_iter()
        .map(|tool| parse_tool_definition(&tool).expect("Anthropic builtin tool"))
        .collect::<Vec<_>>();

        let req = UrpRequest {
            model: "claude-sonnet-4-5".to_string(),
            input: items_to_nodes(vec![Item::text(Role::User, "use native tools")]),
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: Some(tools),
            tool_choice: None,
            parallel_tool_calls: None,
            stop: None,
            verbosity: None,
            response_format: None,
            user: None,
            extra_body: empty_map(),
        };

        let encoded = encode_request(&req, "claude-sonnet-4-5");
        let tools = encoded["tools"].as_array().expect("tools array");

        assert_eq!(tools.len(), 3);
        assert!(
            tools
                .iter()
                .all(|tool| tool.get("function").is_none() && tool.get("custom").is_none()),
            "Anthropic built-ins must stay flat non-function descriptors: {encoded}"
        );
        assert_eq!(tools[0]["type"], json!("computer_20251124"));
        assert_eq!(tools[0]["name"], json!("computer"));
        assert_eq!(tools[0]["display_width_px"], json!(1280));
        assert_eq!(tools[0]["display_height_px"], json!(720));
        assert_eq!(tools[0]["display_number"], json!(1));
        assert_eq!(tools[0]["enable_zoom"], json!(true));

        assert_eq!(tools[1]["type"], json!("web_search_20260209"));
        assert_eq!(tools[1]["name"], json!("web_search"));
        assert_eq!(tools[1]["max_uses"], json!(3));
        assert_eq!(tools[1]["allowed_domains"], json!(["example.com"]));
        assert_eq!(tools[1]["user_location"]["city"], json!("San Francisco"));

        assert_eq!(tools[2]["type"], json!("mcp_toolset"));
        assert_eq!(tools[2]["mcp_server_name"], json!("docs"));
        assert_eq!(tools[2]["default_config"], json!({ "enabled": true }));
        assert_eq!(tools[2]["configs"]["search"]["defer_loading"], json!(true));
        assert_eq!(tools[2]["cache_control"], json!({ "type": "ephemeral" }));
    }

    #[test]
    fn encode_request_does_not_emit_fake_response_format() {
        let req = UrpRequest {
            model: "claude-sonnet-4-5".to_string(),
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

        let encoded = encode_request(&req, "claude-sonnet-4-5");
        assert!(
            encoded.get("response_format").is_none(),
            "Anthropic requests must omit unsupported response_format"
        );
        assert_eq!(
            encoded["max_tokens"],
            json!(ANTHROPIC_DEFAULT_MAX_TOKENS),
            "Anthropic requests without a downstream cap must default to Anthropic's max output budget"
        );
    }

    #[test]
    fn messages_structured_output_uses_output_config_format() {
        let mut req = UrpRequest {
            model: "claude-sonnet-4-6".to_string(),
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
                extra_body: HashMap::from([(
                    MESSAGES_OUTPUT_CONFIG_EXTRA_KEY.to_string(),
                    json!({
                        "effort": "max",
                        "format": {
                            "type": "json_schema",
                            "schema": { "type": "string" },
                            "messages_extension": true
                        },
                        "vendor_control": [1, 2, 3]
                    }),
                )]),
            }),
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            stop: None,
            verbosity: None,
            response_format: Some(ResponseFormat::JsonSchema {
                json_schema: crate::urp::JsonSchemaDefinition {
                    name: "openai_name_must_not_leak".to_string(),
                    description: Some("OpenAI-only description".to_string()),
                    schema: json!({
                        "type": "object",
                        "properties": { "answer": { "type": "string" } }
                    }),
                    strict: Some(true),
                    extra_body: HashMap::from([("openai_extension".to_string(), json!(true))]),
                },
            }),
            user: None,
            extra_body: empty_map(),
        };

        let encoded = encode_request(&req, "claude-sonnet-4-6");
        assert_eq!(encoded["output_config"]["effort"], json!("max"));
        assert_eq!(encoded["output_config"]["vendor_control"], json!([1, 2, 3]));
        assert_eq!(
            encoded["output_config"]["format"],
            json!({
                "type": "json_schema",
                "schema": {
                    "type": "object",
                    "properties": { "answer": { "type": "string" } }
                },
                "messages_extension": true
            })
        );
        assert!(encoded.get("response_format").is_none());

        req.response_format = Some(ResponseFormat::JsonObject);
        let encoded = encode_request(&req, "claude-sonnet-4-6");
        assert!(encoded["output_config"].get("format").is_none());
        assert_eq!(encoded["output_config"]["vendor_control"], json!([1, 2, 3]));
    }

    #[test]
    fn encode_request_preserves_explicit_max_output_tokens() {
        let req = UrpRequest {
            model: "claude-sonnet-4-5".to_string(),
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
            max_output_tokens: Some(321),
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

        let encoded = encode_request(&req, "claude-sonnet-4-5");
        assert_eq!(encoded["max_tokens"], json!(321));
    }

    #[test]
    fn anthropic_text_block_phase_round_trips_to_responses_compatible_urp() {
        let source = json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "model": "claude",
            "content": [
                { "type": "text", "text": "prep", "phase": "commentary" },
                { "type": "tool_use", "id": "call_1", "name": "tool", "input": {} },
                { "type": "text", "text": "done", "phase": "final_answer" }
            ],
            "stop_reason": "tool_use"
        });

        let decoded = decode_anthropic::decode_response(&source).expect("decode response");
        let encoded = encode_response(&decoded, "claude");
        let content = encoded["content"].as_array().expect("content array");

        assert_eq!(content[0]["phase"], json!("commentary"));
        assert_eq!(content[1]["type"], json!("tool_use"));
        assert_eq!(content[2]["phase"], json!("final_answer"));
    }

    #[test]
    fn anthropic_usage_round_trips_extension_fields_without_leaking_nested_aliases() {
        let mut usage_extra = HashMap::new();
        usage_extra.insert("native_counter".to_string(), json!(7));
        usage_extra.insert("input_tokens".to_string(), json!(999));
        usage_extra.insert("reasoning_output_tokens".to_string(), json!(999));
        usage_extra.insert(
            "output_tokens_details".to_string(),
            json!({ "thinking_tokens": 999, "native_detail": 9 }),
        );
        usage_extra.insert("cache_creation".to_string(), json!({ "stale": true }));
        usage_extra.insert("_monoize_usage_snapshot".to_string(), json!("internal"));
        let response = UrpResponse {
            id: "msg_usage".to_string(),
            model: "claude".to_string(),
            created_at: None,
            output: items_to_nodes(vec![Item::new_message(Role::Assistant)]),
            finish_reason: Some(FinishReason::Stop),
            usage: Some(Usage {
                input_tokens: 11,
                output_tokens: 5,
                input_details: Some(crate::urp::InputDetails {
                    standard_tokens: 0,
                    cache_read_tokens: 2,
                    cache_read_modality_breakdown: None,
                    cache_creation_tokens: 3,
                    cache_creation_5m_tokens: 1,
                    cache_creation_1h_tokens: 2,
                    tool_prompt_tokens: 4,
                    modality_breakdown: None,
                }),
                output_details: Some(OutputDetails {
                    standard_tokens: 0,
                    reasoning_tokens: 6,
                    accepted_prediction_tokens: 7,
                    rejected_prediction_tokens: 8,
                    modality_breakdown: None,
                }),
                extra_body: usage_extra,
            }),
            extra_body: empty_map(),
        };

        let encoded = encode_response(&response, "claude");
        let usage = encoded["usage"].as_object().expect("usage object");
        assert_eq!(usage.get("tool_prompt_input_tokens"), Some(&json!(4)));
        assert!(usage.get("reasoning_output_tokens").is_none());
        assert_eq!(usage["output_tokens_details"]["thinking_tokens"], json!(6));
        assert_eq!(usage["output_tokens_details"]["native_detail"], json!(9));
        assert_eq!(
            usage.get("accepted_prediction_output_tokens"),
            Some(&json!(7))
        );
        assert_eq!(
            usage.get("rejected_prediction_output_tokens"),
            Some(&json!(8))
        );
        assert_eq!(usage.get("native_counter"), Some(&json!(7)));
        assert_eq!(usage.get("input_tokens"), Some(&json!(6)));
        assert!(usage.get("reasoning_output_tokens").is_none());
        assert_eq!(usage["cache_creation"]["ephemeral_5m_input_tokens"], json!(1));
        assert_eq!(usage["cache_creation"]["ephemeral_1h_input_tokens"], json!(2));
        assert!(usage.get("_monoize_usage_snapshot").is_none());

        let decoded = decode_anthropic::decode_response(&encoded).expect("decode response");
        let decoded_usage = decoded.usage.expect("usage should decode");
        assert_eq!(
            decoded_usage
                .input_details
                .expect("input details")
                .tool_prompt_tokens,
            4
        );
        let decoded_output = decoded_usage.output_details.expect("output details");
        assert_eq!(decoded_output.reasoning_tokens, 6);
        assert_eq!(decoded_output.accepted_prediction_tokens, 7);
        assert_eq!(decoded_output.rejected_prediction_tokens, 8);
        assert!(
            !decoded_usage
                .extra_body
                .contains_key("tool_prompt_input_tokens")
        );
        assert!(
            !decoded_usage
                .extra_body
                .contains_key("reasoning_output_tokens")
        );
        assert_eq!(
            decoded_usage.extra_body.get("output_tokens_details"),
            Some(&json!({ "native_detail": 9 }))
        );
        assert_eq!(
            decoded_usage.extra_body.get("native_counter"),
            Some(&json!(7))
        );
    }

    #[test]
    fn anthropic_response_round_trip_normalizes_thinking_text_as_summary() {
        let response = UrpResponse {
            id: "msg_roundtrip_reasoning".to_string(),
            model: "claude".to_string(),
            created_at: None,
            output: items_to_nodes(vec![Item::Message {
                id: None,
                role: Role::Assistant,
                parts: vec![Part::Reasoning {
                    id: None,
                    content: Some("full reasoning".to_string()),
                    encrypted: Some(json!("sig_1")),
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

        let encoded = encode_response(&response, "claude");
        let decoded = decode_anthropic::decode_response(&encoded).expect("decode response");
        let decoded_outputs = nodes_to_items(&decoded.output);
        let Item::Message { parts, .. } = &decoded_outputs[0] else {
            panic!("expected assistant output");
        };

        assert_eq!(
            parts.len(),
            1,
            "thinking block should decode to one reasoning part"
        );
        assert!(matches!(
            &parts[0],
            Part::Reasoning {
                content: None,
                encrypted: Some(Value::String(sig)),
                summary: Some(summary),
                ..
            } if summary == "full reasoning" && sig == "sig_1"
        ));
    }

    #[test]
    fn encode_request_strips_orphaned_tool_use_via_shared_pre_encode() {
        use crate::handlers::strip_orphaned_tool_calls;
        use crate::urp::ToolResultContent;

        let mut req = UrpRequest {
            model: "claude-sonnet-4-6".to_string(),
            input: items_to_nodes(vec![
                Item::text(Role::User, "list files"),
                Item::Message {
                    id: None,
                    role: Role::Assistant,
                    parts: vec![
                        Part::ToolCall {
                            id: None,
                            tool_type: ToolCallType::Function,
                            call_id: "answered".to_string(),
                            name: "bash".to_string(),
                            arguments: r#"{"command":"ls"}"#.to_string(),
                            extra_body: empty_map(),
                        },
                        Part::ToolCall {
                            id: None,
                            tool_type: ToolCallType::Function,
                            call_id: "orphan".to_string(),
                            name: "bash".to_string(),
                            arguments: r#"{"command":"cat x"}"#.to_string(),
                            extra_body: empty_map(),
                        },
                    ],
                    extra_body: empty_map(),
                },
                Item::ToolResult {
                    id: None,
                    tool_type: ToolCallType::Function,
                    call_id: "answered".to_string(),
                    is_error: false,
                    content: vec![ToolResultContent::Text {
                        text: "file1.txt".to_string(),
                        extra_body: empty_map(),
                    }],
                    extra_body: empty_map(),
                },
                Item::text(Role::User, "thanks"),
            ]),
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: Some(256),
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

        strip_orphaned_tool_calls(&mut req);
        let encoded = encode_request(&req, "claude-sonnet-4-6");
        let messages = encoded["messages"].as_array().expect("messages array");

        let assistant_msg = &messages[1];
        let assistant_content = assistant_msg["content"].as_array().expect("content array");
        assert_eq!(
            assistant_content.len(),
            1,
            "orphaned tool_use should be stripped"
        );
        assert_eq!(assistant_content[0]["id"], json!("answered"));
    }

    fn req_with_effort(model: &str, effort: &str) -> UrpRequest {
        UrpRequest {
            model: model.to_string(),
            input: items_to_nodes(vec![Item::Message {
                id: None,
                role: Role::User,
                parts: vec![Part::Text {
                    content: "hi".to_string(),
                    extra_body: empty_map(),
                }],
                extra_body: empty_map(),
            }]),
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: Some(crate::urp::ReasoningConfig {
                effort: Some(effort.to_string()),
                extra_body: HashMap::new(),
            }),
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            stop: None,
            verbosity: None,
            response_format: None,
            user: None,
            extra_body: empty_map(),
        }
    }

    #[test]
    fn adaptive_model_detection_defaults_new_claude_families_to_adaptive() {
        for m in [
            "claude-opus-4-6",
            "claude-opus-4.6",
            "claude-opus-4-7-20260101",
            "claude-opus-4.7",
            "claude-sonnet-4-6-20250101",
            "claude-sonnet-4-7",
            "claude-haiku-4-6",
            "claude-opus-4-8",
            "claude-opus-5-0",
            "claude-sonnet-6-0",
            "claude-fable-5",
            "claude-mythos-5-max",
            "claude-future-family",
            "opus-4-7",
            "opus-4-6",
            "sonnet-4.7",
            "sonnet-4.6",
            "haiku-4-6",
        ] {
            assert!(
                model_supports_adaptive(m),
                "{m} must be detected as adaptive-thinking model"
            );
        }
        for m in [
            "claude-opus-4-5",
            "claude-opus-4.5",
            "claude-opus-4-20250514",
            "claude-sonnet-4-0",
            "claude-sonnet-4-20250514",
            "claude-sonnet-3-7",
            "claude-3-7-sonnet-20250219",
            "claude-haiku-4-5",
            "claude-haiku-3-5",
            "claude-3-5-haiku-20241022",
            "claude-3-5-sonnet",
            "opus-4-5",
            "sonnet-4.5",
            "haiku-4.5",
            "gpt-5-mini-msg",
        ] {
            assert!(
                !model_supports_adaptive(m),
                "{m} must NOT be detected as adaptive-thinking model"
            );
        }
    }

    #[test]
    fn adaptive_fable_encoder_passes_xhigh_and_max_through_distinctly() {
        for effort in ["xhigh", "max"] {
            let encoded =
                encode_request(&req_with_effort("claude-fable-5", effort), "claude-fable-5");
            assert_eq!(encoded["thinking"], json!({ "type": "adaptive" }));
            assert_eq!(
                encoded["output_config"]["effort"],
                json!(effort),
                "adaptive path must forward {effort} as-is"
            );
        }
    }

    #[test]
    fn non_adaptive_encoder_uses_32000_for_both_xhigh_and_max() {
        for effort in ["xhigh", "max"] {
            let encoded = encode_request(
                &req_with_effort("claude-sonnet-4-5", effort),
                "claude-sonnet-4-5",
            );
            assert_eq!(
                encoded["thinking"],
                json!({
                    "type": "enabled",
                    "budget_tokens": 32000
                }),
                "non-adaptive {effort} must emit budget_tokens=32000"
            );
            assert!(
                encoded.get("output_config").is_none(),
                "non-adaptive path must not emit output_config"
            );
        }
    }

    #[test]
    fn non_adaptive_encoder_budget_table_is_stable() {
        for (effort, expected) in [
            ("minimum", 1024),
            ("low", 1024),
            ("medium", 4096),
            ("high", 16384),
            ("xhigh", 32000),
            ("max", 32000),
        ] {
            assert_eq!(
                effort_to_budget(effort),
                expected,
                "effort_to_budget({effort}) regressed"
            );
        }
    }

    #[test]
    fn explicit_messages_reasoning_controls_override_generation() {
        let mut req = req_with_effort("claude-sonnet-5", "high");
        let thinking = json!({
            "type": "disabled",
            "display": "omitted",
            "budget_tokens": 777,
            "vendor_flag": true
        });
        let output_config = json!({
            "effort": "max",
            "format": {
                "type": "json_schema",
                "schema": { "type": "object" }
            },
            "vendor_flag": "preserve"
        });
        let reasoning = req.reasoning.as_mut().expect("reasoning config");
        reasoning.extra_body.insert(
            MESSAGES_THINKING_CONFIG_EXTRA_KEY.to_string(),
            thinking.clone(),
        );
        reasoning.extra_body.insert(
            MESSAGES_OUTPUT_CONFIG_EXTRA_KEY.to_string(),
            output_config.clone(),
        );

        let encoded = encode_request(&req, "claude-sonnet-5");
        assert_eq!(encoded["thinking"], thinking);
        assert_eq!(encoded["output_config"], output_config);
    }

    #[test]
    fn adaptive_messages_upstream_keeps_thinking_without_explicit_effort() {
        let mut req = req_with_effort("claude-sonnet-5", "high");
        req.reasoning.as_mut().expect("reasoning config").effort = None;

        let encoded = encode_request(&req, "claude-sonnet-5");
        assert_eq!(encoded["thinking"], json!({ "type": "adaptive" }));
        assert!(encoded.get("output_config").is_none());
    }

    #[test]
    fn non_adaptive_messages_upstream_defaults_missing_effort_to_medium_budget() {
        let mut req = req_with_effort("claude-sonnet-4-5", "high");
        req.reasoning.as_mut().expect("reasoning config").effort = None;

        let encoded = encode_request(&req, "claude-sonnet-4-5");
        assert_eq!(
            encoded["thinking"],
            json!({
                "type": "enabled",
                "budget_tokens": 4096
            })
        );
    }
}

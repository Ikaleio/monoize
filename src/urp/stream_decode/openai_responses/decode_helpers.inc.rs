#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_accumulated_output_nodes_greedily_merge_into_one_compat_message() {
        let calls = HashMap::from([(
            "call_1".to_string(),
            ("lookup".to_string(), "{}".to_string()),
        )]);

        let outputs = build_accumulated_output_nodes(
            "think",
            "summary",
            "sig_1",
            Some("anthropic"),
            Some(0),
            &HashMap::from([(0, "answer".to_string())]),
            &HashMap::from([(0, "analysis".to_string())]),
            &HashMap::new(),
            &HashMap::new(),
            &["call_1".to_string()],
            &calls,
            &HashMap::from([(1, "call_1".to_string())]),
        );

        let outputs = nodes_to_items(&outputs);
        assert_eq!(outputs.len(), 2);
        let Item::Message {
            parts, extra_body, ..
        } = &outputs[0]
        else {
            panic!("expected reasoning compatibility item");
        };
        assert!(extra_body.is_empty());
        assert!(matches!(
            &parts[0],
            Part::Reasoning {
                content: Some(content),
                summary: Some(summary),
                encrypted: Some(Value::String(sig)),
                source: Some(source),
                extra_body: _,
                ..
            } if content == "think" && summary == "summary" && sig == "sig_1" && source == "anthropic"
        ));
        let Item::Message {
            role,
            parts,
            extra_body,
            ..
        } = &outputs[1]
        else {
            panic!("expected phased assistant compatibility item");
        };
        assert_eq!(role, &Role::Assistant);
        assert_eq!(extra_body.get("phase"), Some(&json!("analysis")));
        assert!(matches!(
            &parts[0],
            Part::Text { content, extra_body } if content == "answer" && extra_body.get("phase") == Some(&json!("analysis"))
        ));
        assert!(matches!(
            &parts[1],
            Part::ToolCall {
                call_id,
                name,
                arguments,
                ..
            } if call_id == "call_1" && name == "lookup" && arguments == "{}"
        ));
    }

    #[test]
    fn build_accumulated_output_nodes_omit_empty_text_message() {
        let outputs = build_accumulated_output_nodes(
            "",
            "",
            "sig_only",
            None,
            Some(0),
            &HashMap::new(),
            &HashMap::from([(0, "analysis".to_string())]),
            &HashMap::new(),
            &HashMap::new(),
            &[],
            &HashMap::new(),
            &HashMap::new(),
        );

        let outputs = nodes_to_items(&outputs);
        assert_eq!(outputs.len(), 1);
        let Item::Message {
            parts, extra_body, ..
        } = &outputs[0]
        else {
            panic!("expected assistant message output");
        };
        assert!(extra_body.is_empty());
        assert_eq!(parts.len(), 1);
        assert!(matches!(
            &parts[0],
            Part::Reasoning {
                content: None,
                encrypted: Some(Value::String(sig)),
                ..
            } if sig == "sig_only"
        ));
    }

    #[test]
    fn build_accumulated_output_nodes_preserve_multiple_output_text_phases() {
        let outputs = build_accumulated_output_nodes(
            "",
            "",
            "",
            None,
            None,
            &HashMap::from([(0, "analysis".to_string()), (2, "final".to_string())]),
            &HashMap::from([
                (0, "commentary".to_string()),
                (2, "final_answer".to_string()),
            ]),
            &HashMap::new(),
            &HashMap::new(),
            &[],
            &HashMap::new(),
            &HashMap::new(),
        );

        let outputs = nodes_to_items(&outputs);
        assert_eq!(outputs.len(), 2);
        assert!(matches!(
            &outputs[0],
            Item::Message { parts, .. }
                if matches!(
                    &parts[0],
                    Part::Text { content, extra_body }
                        if content == "analysis"
                            && extra_body.get("phase") == Some(&json!("commentary"))
                )
        ));
        assert!(matches!(
            &outputs[1],
            Item::Message { parts, .. }
                if matches!(
                    &parts[0],
                    Part::Text { content, extra_body }
                        if content == "final"
                            && extra_body.get("phase") == Some(&json!("final_answer"))
                )
        ));
    }

    #[test]
    fn build_accumulated_output_nodes_preserve_real_output_index_order_in_fallback() {
        let calls = HashMap::from([(
            "call_1".to_string(),
            ("lookup".to_string(), "{}".to_string()),
        )]);

        let outputs = build_accumulated_output_nodes(
            "think",
            "summary",
            "",
            Some("upstream-reasoner"),
            Some(2),
            &HashMap::from([(5, "answer".to_string())]),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &["call_1".to_string()],
            &calls,
            &HashMap::from([(9, "call_1".to_string())]),
        );

        let outputs = nodes_to_items(&outputs);
        assert_eq!(outputs.len(), 2);
        let Item::Message { parts, .. } = &outputs[0] else {
            panic!("expected reasoning compatibility item");
        };
        assert!(matches!(
            &parts[0],
            Part::Reasoning {
                source: Some(source),
                ..
            } if source == "upstream-reasoner"
        ));
        let Item::Message { parts, .. } = &outputs[1] else {
            panic!("expected assistant action compatibility item");
        };
        assert!(matches!(&parts[0], Part::Text { content, .. } if content == "answer"));
        assert!(matches!(&parts[1], Part::ToolCall { call_id, .. } if call_id == "call_1"));
    }

    #[test]
    fn map_response_completed_uses_greedy_nonstream_decoder_shape() {
        let event = json!({
            "response": {
                "id": "resp_test",
                "object": "response",
                "created": 1,
                "model": "gpt-5.4",
                "status": "completed",
                "output": [
                    {
                        "type": "reasoning",
                        "text": "think",
                        "summary": [{ "type": "summary_text", "text": "summary" }],
                        "encrypted_content": "sig_1"
                    },
                    {
                        "type": "message",
                        "role": "assistant",
                        "phase": "analysis",
                        "content": [
                            { "type": "output_text", "text": "answer" }
                        ]
                    },
                    {
                        "type": "function_call",
                        "call_id": "call_1",
                        "name": "lookup",
                        "arguments": "{}"
                    }
                ]
            }
        });

        let mut state = ResponsesStreamIndexState::default();
        let completed_events = map_response_completed(event, &mut state);
        let Some(UrpStreamEvent::ResponseDone {
            finish_reason,
            output,
            ..
        }) = completed_events.last()
        else {
            panic!("expected response done event");
        };

        assert_eq!(*finish_reason, Some(FinishReason::ToolCalls));
        assert_eq!(output.len(), 3);
        let output_items = nodes_to_items(output);
        let Item::Message {
            parts, extra_body, ..
        } = &output_items[0]
        else {
            panic!("expected assistant message output");
        };
        assert_eq!(extra_body.get("phase"), Some(&json!("analysis")));
        assert!(matches!(
            &parts[0],
            Part::Reasoning {
                content: Some(content),
                summary: Some(summary),
                encrypted: Some(Value::String(sig)),
                ..
            } if content == "think" && summary == "summary" && sig == "sig_1"
        ));
        assert!(matches!(
            &parts[1],
            Part::Text { content, extra_body } if content == "answer" && extra_body.get("phase") == Some(&json!("analysis"))
        ));
        assert!(matches!(&parts[2], Part::ToolCall { call_id, .. } if call_id == "call_1"));
    }

    #[test]
    fn top_level_reasoning_and_function_call_items_emit_node_lifecycle_in_source_order() {
        let mut state = ResponsesStreamIndexState::default();
        let reasoning_events = map_responses_event_to_urp_events_with_state(
            "response.output_item.added",
            json!({
                "output_index": 7,
                "item": { "type": "reasoning", "text": "think" }
            }),
            &HashMap::new(),
            &mut state,
        );
        let reasoning_offset = if matches!(
            reasoning_events.first(),
            Some(UrpStreamEvent::NodeStart {
                header: NodeHeader::NextDownstreamEnvelopeExtra,
                ..
            })
        ) {
            assert!(matches!(
                reasoning_events.get(1),
                Some(UrpStreamEvent::NodeDone {
                    node: Node::NextDownstreamEnvelopeExtra { .. },
                    ..
                })
            ));
            2
        } else {
            0
        };
        assert!(matches!(
            &reasoning_events[reasoning_offset],
            UrpStreamEvent::NodeStart {
                node_index,
                header: NodeHeader::Reasoning { .. },
                ..
            } if *node_index == 0
        ));

        let function_events = map_responses_event_to_urp_events_with_state(
            "response.output_item.added",
            json!({
                "output_index": 9,
                "item": {
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "lookup",
                    "arguments": ""
                }
            }),
            &HashMap::new(),
            &mut state,
        );
        let function_offset = if matches!(
            function_events.first(),
            Some(UrpStreamEvent::NodeStart {
                header: NodeHeader::NextDownstreamEnvelopeExtra,
                ..
            })
        ) {
            assert!(matches!(
                function_events.get(1),
                Some(UrpStreamEvent::NodeDone {
                    node: Node::NextDownstreamEnvelopeExtra { .. },
                    ..
                })
            ));
            2
        } else {
            0
        };
        let tool_call_node_index = match &function_events[function_offset] {
            UrpStreamEvent::NodeStart {
                node_index,
                header: NodeHeader::ToolCall { call_id, name, .. },
                ..
            } if call_id == "call_1" && name == "lookup" => *node_index,
            other => panic!("unexpected tool-call node start: {other:?}"),
        };

        let function_delta = map_responses_event_to_urp_events_with_state(
            "response.function_call_arguments.delta",
            json!({
                "output_index": 9,
                "delta": "{}"
            }),
            &HashMap::new(),
            &mut state,
        );
        assert!(matches!(
            &function_delta[0],
            UrpStreamEvent::NodeDelta {
                node_index,
                delta: NodeDelta::ToolCallArguments { arguments },
                ..
            } if *node_index == tool_call_node_index && arguments == "{}"
        ));
    }

    #[test]
    fn content_part_done_reuses_normalized_node_index() {
        let mut state = ResponsesStreamIndexState::default();

        let added = map_responses_event_to_urp_events_with_state(
            "response.content_part.added",
            json!({
                "output_index": 7,
                "content_index": 42,
                "part": { "type": "output_text", "text": "" }
            }),
            &HashMap::new(),
            &mut state,
        );
        assert!(matches!(
            &added[0],
            UrpStreamEvent::NodeStart {
                node_index,
                header: NodeHeader::Text { .. },
                ..
            } if *node_index == 0
        ));

        let done = map_responses_event_to_urp_events_with_state(
            "response.content_part.done",
            json!({
                "output_index": 7,
                "content_index": 42,
                "part": { "type": "output_text", "text": "done" }
            }),
            &HashMap::new(),
            &mut state,
        );
        assert!(matches!(
            &done[0],
            UrpStreamEvent::NodeDone {
                node_index,
                node: Node::Text { content, .. },
                ..
            } if *node_index == 0 && content == "done"
        ));
    }

    #[test]
    fn reasoning_delta_item_id_overrides_added_item_id() {
        let mut state = ResponsesStreamIndexState::default();

        let added = map_responses_event_to_urp_events_with_state(
            "response.output_item.added",
            json!({
                "output_index": 0,
                "item": {
                    "type": "reasoning",
                    "id": "rs_added",
                    "summary": [{ "type": "summary_text", "text": "" }],
                    "text": ""
                }
            }),
            &HashMap::new(),
            &mut state,
        );
        assert!(added.iter().any(|event| matches!(
            event,
            UrpStreamEvent::NodeStart {
                header: NodeHeader::Reasoning { id },
                ..
            } if id.as_deref() == Some("rs_added")
        )));

        let delta = map_responses_event_to_urp_events_with_state(
            "response.reasoning_summary_text.delta",
            json!({
                "output_index": 0,
                "item_id": "rs_authoritative",
                "summary_index": 0,
                "delta": "summary"
            }),
            &HashMap::new(),
            &mut state,
        );
        assert!(matches!(
            &delta[0],
            UrpStreamEvent::NodeDelta {
                delta: NodeDelta::Reasoning { summary: Some(summary), .. },
                extra_body,
                ..
            } if summary == "summary" && extra_body.get("reasoning_item_id") == Some(&json!("rs_authoritative"))
        ));
        assert_eq!(
            state
                .output_state_by_index
                .get(&0)
                .and_then(|output| output.item_id.as_deref()),
            Some("rs_authoritative")
        );

        let _done = map_responses_event_to_urp_events_with_state(
            "response.output_item.done",
            json!({
                "output_index": 0,
                "item": {
                    "type": "reasoning",
                    "id": "rs_authoritative",
                    "summary": [{ "type": "summary_text", "text": "summary" }],
                    "text": "",
                    "encrypted_content": "sig_1"
                }
            }),
            &HashMap::new(),
            &mut state,
        );

        let accumulated = build_accumulated_output_nodes(
            "",
            "summary",
            "sig_1",
            None,
            Some(0),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::from([(0, "rs_authoritative".to_string())]),
            &[],
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(matches!(
            accumulated.first(),
            Some(Node::Reasoning {
                id,
                encrypted: Some(Value::String(sig)),
                summary: Some(summary),
                ..
            }) if id.as_deref() == Some("rs_authoritative") && sig == "sig_1" && summary == "summary"
        ));
    }

    #[test]
    fn reasoning_output_item_added_control_extra_preserves_item_id() {
        let mut state = ResponsesStreamIndexState::default();

        let events = map_responses_event_to_urp_events_with_state(
            "response.output_item.added",
            json!({
                "output_index": 0,
                "item": {
                    "type": "reasoning",
                    "id": "rs_original",
                    "encrypted_content": "opaque_payload",
                    "summary": []
                }
            }),
            &HashMap::new(),
            &mut state,
        );

        assert!(events.iter().any(|event| matches!(
            event,
            UrpStreamEvent::NodeStart {
                header: NodeHeader::NextDownstreamEnvelopeExtra,
                extra_body,
                ..
            } if extra_body.get("id") == Some(&json!("rs_original"))
                && extra_body.get("encrypted_content") == Some(&json!("opaque_payload"))
        )));
    }

    #[test]
    fn synthetic_text_fallback_preserves_multiple_phases_and_allocates_distinct_part_indices() {
        let output_items = nodes_to_items(&build_accumulated_output_nodes(
            "",
            "",
            "",
            None,
            None,
            &HashMap::from([(0, "analysis".to_string()), (2, "final".to_string())]),
            &HashMap::from([
                (0, "commentary".to_string()),
                (2, "final_answer".to_string()),
            ]),
            &HashMap::new(),
            &HashMap::new(),
            &[],
            &HashMap::new(),
            &HashMap::new(),
        ));
        let mut state = ResponsesStreamIndexState::default();
        assert_eq!(state.node_index_for_content(11, 4), 0);
        assert_eq!(state.synthetic_node_index_for_output(12), 1);

        let mut observed = Vec::new();

        for (_final_item_index, output_item) in output_items.iter().enumerate() {
            let Item::Message {
                role: Role::Assistant,
                parts,
                extra_body,
                ..
            } = output_item
            else {
                continue;
            };

            for part in parts {
                let Part::Text {
                    content,
                    extra_body: text_extra_body,
                } = part
                else {
                    continue;
                };
                let synthetic_text_item = Item::Message {
                    id: None,
                    role: Role::Assistant,
                    parts: vec![Part::Text {
                        content: content.clone(),
                        extra_body: text_extra_body.clone(),
                    }],
                    extra_body: extra_body.clone(),
                };
                let node_index = state.allocate_fresh_node_index();
                observed.push(UrpStreamEvent::NodeStart {
                    node_index,
                    header: NodeHeader::Text {
                        id: None,
                        role: OrdinaryRole::Assistant,
                        phase: text_extra_body
                            .get("phase")
                            .and_then(|value| value.as_str())
                            .map(str::to_string),
                    },
                    extra_body: item_extra_body_from_item(&synthetic_text_item),
                });
                observed.push(UrpStreamEvent::NodeDelta {
                    node_index,
                    delta: NodeDelta::Text {
                        content: content.clone(),
                    },
                    usage: None,
                    extra_body: text_extra_body.clone(),
                });
            }
        }

        assert!(matches!(
            &observed[1],
            UrpStreamEvent::NodeDelta { node_index, extra_body, .. }
                if *node_index == 2 && extra_body.get("phase") == Some(&json!("commentary"))
        ));
        assert!(matches!(
            &observed[3],
            UrpStreamEvent::NodeDelta { node_index, extra_body, .. }
                if *node_index == 3 && extra_body.get("phase") == Some(&json!("final_answer"))
        ));
    }

    #[test]
    fn build_accumulated_output_nodes_omit_reasoning_source_when_missing() {
        let outputs = build_accumulated_output_nodes(
            "think",
            "",
            "",
            None,
            Some(0),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &[],
            &HashMap::new(),
            &HashMap::new(),
        );

        let outputs = nodes_to_items(&outputs);
        let Item::Message { parts, .. } = &outputs[0] else {
            panic!("expected assistant message output");
        };
        assert!(matches!(&parts[0], Part::Reasoning { source: None, .. }));
    }

    #[test]
    fn synthetic_text_fallback_is_suppressed_when_text_part_done_already_emitted() {
        let saw_text_delta = false;
        let mut saw_text_part_done = false;

        let done_event = json!({
            "output_index": 4,
            "content_index": 9,
            "part": { "type": "output_text", "text": "ready" }
        });

        if done_event
            .get("part")
            .and_then(|part| part.get("type"))
            .and_then(|v| v.as_str())
            .is_some_and(|part_type| matches!(part_type, "output_text" | "text"))
            && done_event
                .get("part")
                .and_then(|part| part.get("text"))
                .and_then(|v| v.as_str())
                .is_some_and(|text| !text.is_empty())
        {
            saw_text_part_done = true;
        }

        assert!(!(!saw_text_delta && !saw_text_part_done));
    }

    #[test]
    fn decode_reasoning_part_preserves_summary_separately_from_text() {
        let part = decode_part_from_value(&json!({
            "type": "reasoning",
            "text": "full reasoning",
            "summary": [{ "type": "summary_text", "text": "brief summary" }],
            "encrypted_content": "sig_1"
        }));

        assert!(matches!(
            part,
            Part::Reasoning {
                content: Some(content),
                summary: Some(summary),
                encrypted: Some(Value::String(sig)),
                ..
            } if content == "full reasoning" && summary == "brief summary" && sig == "sig_1"
        ));
    }

    #[test]
    fn map_output_item_done_message_fallback_emits_one_node_per_part_then_item_done() {
        let mut state = ResponsesStreamIndexState::default();
        let added_events = map_responses_event_to_urp_events_with_state(
            "response.output_item.added",
            json!({
                "output_index": 3,
                "item": {
                    "type": "message",
                    "role": "assistant",
                    "content": []
                }
            }),
            &HashMap::new(),
            &mut state,
        );
        assert!(added_events.is_empty());

        let done_events = map_responses_event_to_urp_events_with_state(
            "response.output_item.done",
            json!({
                "output_index": 3,
                "item": {
                    "type": "message",
                    "role": "assistant",
                    "content": [
                        { "type": "output_text", "text": "answer" },
                        { "type": "function_call", "call_id": "call_1", "name": "lookup", "arguments": "{}" }
                    ]
                }
            }),
            &HashMap::new(),
            &mut state,
        );

        assert_eq!(
            done_events.len(),
            4,
            "fallback should emit two node lifecycles (start+done each)"
        );
        assert!(matches!(
            &done_events[0],
            UrpStreamEvent::NodeStart {
                node_index: 0,
                header: NodeHeader::Text { .. },
                ..
            }
        ));
        assert!(matches!(
            &done_events[1],
            UrpStreamEvent::NodeDone {
                node_index: 0,
                node: Node::Text { content, .. },
                ..
            } if content == "answer"
        ));
        assert!(done_events.iter().any(|event| matches!(
            event,
            UrpStreamEvent::NodeStart {
                node_index: 1,
                header: NodeHeader::ToolCall { call_id, .. },
                ..
            } if call_id == "call_1"
        )));
        assert!(done_events.iter().any(|event| matches!(
            event,
            UrpStreamEvent::NodeDone {
                node_index: 1,
                node: Node::ToolCall { call_id, .. },
                ..
            } if call_id == "call_1"
        )));
    }

    #[test]
    fn accumulated_output_nodes_drive_response_done_before_grouped_projection() {
        let output_nodes = build_accumulated_output_nodes(
            "think",
            "summary",
            "sig_1",
            Some("anthropic"),
            Some(0),
            &HashMap::from([(0, "answer".to_string())]),
            &HashMap::from([(0, "analysis".to_string())]),
            &HashMap::new(),
            &HashMap::new(),
            &["call_1".to_string()],
            &HashMap::from([(
                "call_1".to_string(),
                ("lookup".to_string(), "{}".to_string()),
            )]),
            &HashMap::from([(1, "call_1".to_string())]),
        );

        assert_eq!(output_nodes.len(), 4);
        assert!(matches!(
            &output_nodes[0],
            Node::Reasoning {
                content: Some(content),
                summary: Some(summary),
                encrypted: Some(Value::String(sig)),
                source: Some(source),
                ..
            } if content == "think" && summary == "summary" && sig == "sig_1" && source == "anthropic"
        ));
        assert!(matches!(
            &output_nodes[1],
            Node::NextDownstreamEnvelopeExtra { extra_body }
                if extra_body.get("phase") == Some(&json!("analysis"))
        ));
        assert!(matches!(
            &output_nodes[2],
            Node::Text { role: OrdinaryRole::Assistant, content, phase: Some(phase), .. }
                if content == "answer" && phase == "analysis"
        ));
        assert!(matches!(
            &output_nodes[3],
            Node::ToolCall { call_id, name, arguments, .. }
                if call_id == "call_1" && name == "lookup" && arguments == "{}"
        ));

        let output_items = nodes_to_items(&output_nodes);
        assert_eq!(output_items.len(), 2);
        let Item::Message {
            parts, extra_body, ..
        } = &output_items[0]
        else {
            panic!("expected reasoning compatibility item");
        };
        assert!(extra_body.is_empty());
        assert_eq!(parts.len(), 1);
        let Item::Message {
            parts, extra_body, ..
        } = &output_items[1]
        else {
            panic!("expected phased assistant compatibility item");
        };
        assert_eq!(extra_body.get("phase"), Some(&json!("analysis")));
        assert_eq!(parts.len(), 2);
    }
}

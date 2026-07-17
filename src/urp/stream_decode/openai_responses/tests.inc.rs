#[cfg(test)]
mod image_generation_alias_tests {
    use super::*;

    #[test]
    fn response_image_generation_completed_alias_maps_to_image_node() {
        let mut state = ResponsesStreamIndexState::default();
        let events = map_responses_event_to_urp_events_with_state(
            "response.image_generation.completed",
            json!({
                "id": "ig_1",
                "result": "QUJD",
                "output_format": "jpeg"
            }),
            &HashMap::new(),
            &mut state,
        );

        assert!(matches!(events.as_slice(), [
            UrpStreamEvent::NodeStart { .. },
            UrpStreamEvent::NodeDone {
                node: Node::Image { source: crate::urp::ImageSource::Base64 { media_type, data }, .. },
                ..
            }
        ] if media_type == "image/jpeg" && data == "QUJD"));
    }

    #[test]
    fn response_image_generation_partial_alias_is_ignored() {
        let mut state = ResponsesStreamIndexState::default();
        let events = map_responses_event_to_urp_events_with_state(
            "response.image_generation.partial_image",
            json!({ "partial_image_index": 0, "b64_json": "AAAA" }),
            &HashMap::new(),
            &mut state,
        );

        assert!(matches!(events.as_slice(), [
            UrpStreamEvent::NodeDelta {
                delta: NodeDelta::ProviderItem { data },
                extra_body,
                ..
            }
        ] if data.is_null()
            && extra_body.get("partial_image_index") == Some(&json!(0))
            && extra_body.get("b64_json") == Some(&json!("AAAA"))));
    }

    #[test]
    fn response_image_generation_call_partial_maps_to_provider_delta() {
        let mut state = ResponsesStreamIndexState::default();
        let events = map_responses_event_to_urp_events_with_state(
            "response.image_generation_call.partial_image",
            json!({
                "type": "response.image_generation_call.partial_image",
                "item_id": "ig_1",
                "output_index": 2,
                "partial_image_index": 1,
                "partial_image_b64": "BBBB",
                "sequence_number": 99
            }),
            &HashMap::new(),
            &mut state,
        );

        assert!(matches!(events.as_slice(), [
            UrpStreamEvent::NodeDelta {
                node_index,
                delta: NodeDelta::Image {
                    source: crate::urp::ImageSource::Base64 { media_type, data },
                },
                extra_body,
                ..
            }
        ] if *node_index == 0
            && media_type == "image/png"
            && data == "BBBB"
            && extra_body.get("item_id") == Some(&json!("ig_1"))
            && extra_body.get("output_index") == Some(&json!(2))
            && extra_body.get("partial_image_index") == Some(&json!(1))
            && extra_body.get("provider_event_type") == Some(&json!("response.image_generation_call.partial_image"))
            && !extra_body.contains_key("sequence_number")));
    }

    #[test]
    fn response_completed_snapshot_image_generation_call_maps_to_image_node() {
        let mut state = ResponsesStreamIndexState::default();
        let events = map_responses_event_to_urp_events_with_state(
            "response.completed",
            json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_1",
                    "object": "response",
                    "created_at": 0,
                    "model": "gpt-5.4-mini",
                    "status": "completed",
                    "output": [{
                        "type": "image_generation_call",
                        "id": "ig_1",
                        "status": "completed",
                        "result": "QUJD",
                        "output_format": "webp",
                        "future_field": 7
                    }]
                }
            }),
            &HashMap::new(),
            &mut state,
        );

        let [UrpStreamEvent::ResponseDone { output, .. }] = events.as_slice() else {
            panic!("expected one response done event");
        };
        let [Node::Image {
            id: Some(id),
            source: crate::urp::ImageSource::Base64 { media_type, data },
            extra_body,
            ..
        }] = output.as_slice()
        else {
            panic!("expected one image node");
        };
        assert_eq!(id, "ig_1");
        assert_eq!(media_type, "image/webp");
        assert_eq!(data, "QUJD");
        assert_eq!(
            extra_body.get(RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY),
            Some(&json!({
                "type": "image_generation_call",
                "id": "ig_1",
                "status": "completed",
                "result": "QUJD",
                "output_format": "webp",
                "future_field": 7
            }))
        );
    }
}

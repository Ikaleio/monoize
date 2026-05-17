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

        assert!(events.is_empty());
    }
}

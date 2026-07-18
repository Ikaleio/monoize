use super::*;
use serde_json::json;

fn last_captured_body(ctx: &TestContext, endpoint: &str) -> Value {
    ctx.captured_bodies
        .lock()
        .expect("captured bodies lock")
        .iter()
        .rev()
        .find(|(name, _)| name == endpoint)
        .map(|(_, body)| body.clone())
        .unwrap_or_else(|| panic!("missing captured upstream body for {endpoint}"))
}

mod assistant_blocks {
    use super::*;
    include!("streaming_responses/assistant_blocks.rs");
}

mod basic_errors {
    use super::*;
    include!("streaming_responses/basic_errors.rs");
}

mod images_tools {
    use super::*;
    include!("streaming_responses/images_tools.rs");
}

mod tool_lifecycle {
    use super::*;
    include!("streaming_responses/tool_lifecycle.rs");
}

mod reasoning {
    use super::*;
    include!("streaming_responses/reasoning.rs");
}

mod completed_state {
    use super::*;
    include!("streaming_responses/completed_state.rs");
}

#[tokio::test]
async fn responses_stream_success_binds_native_response_id_for_stateful_continuation() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini",
            "input": "bind streamed response id",
            "stream": true
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("event: response.completed"), "{body}");

    let mut bound = false;
    for _ in 0..50 {
        bound = ctx
            .state
            .channel_affinity
            .lock()
            .await
            .keys()
            .any(|key| key.ends_with("previous_response_id:resp_mock"));
        if bound {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(bound, "successful Responses stream id was not bound");
}

use super::*;
use std::fs;

fn dumps_dir(ctx: &TestContext) -> std::path::PathBuf {
    let db_path = ctx._temp_dir.path().join("monoize.db");
    db_path.parent().expect("db parent exists").join("dumps")
}

/// RCD-Z3: dump writes are asynchronous, so tests poll for the renamed final
/// file (temporary `.tmp.` files are excluded) instead of asserting
/// immediately after the HTTP response.
async fn wait_for_dump_files(dump_dir: &std::path::Path, min_count: usize) -> Vec<String> {
    for _ in 0..400 {
        let mut names: Vec<String> = fs::read_dir(dump_dir)
            .ok()
            .map(|dir| {
                dir.filter_map(|entry| entry.ok())
                    .filter_map(|entry| entry.file_name().into_string().ok())
                    .filter(|name| !name.contains(".tmp."))
                    .collect()
            })
            .unwrap_or_default();
        if names.len() >= min_count {
            names.sort();
            return names;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("capture dump did not appear within timeout");
}

/// Reads through the store so the RCD-Z6 format detection path is exercised.
async fn read_dump(ctx: &TestContext, file_name: &str) -> Value {
    let bytes = ctx
        .state
        .request_capture
        .read_dump_file(file_name)
        .await
        .expect("dump readable")
        .expect("dump exists");
    serde_json::from_slice(&bytes).expect("dump json")
}

async fn enable_request_capture(ctx: &TestContext) {
    let settings = ctx
        .state
        .settings_store
        .get_all()
        .await
        .expect("settings load");
    let updated_settings = monoize::settings::SystemSettings {
        monoize_request_capture_enabled: true,
        ..settings
    };
    ctx.state
        .settings_store
        .update_all(&updated_settings)
        .await
        .expect("settings update");
    {
        let mut runtime = ctx.state.monoize_runtime.write().await;
        runtime.request_capture_enabled = updated_settings.monoize_request_capture_enabled;
        runtime.request_capture_max_total_bytes =
            updated_settings.monoize_request_capture_max_total_bytes;
    }

    let token = ctx
        .auth_header
        .strip_prefix("Bearer ")
        .expect("bearer token present");
    let key = ctx
        .state
        .user_store
        .get_api_key_by_prefix(&token[..12])
        .await
        .expect("lookup succeeds")
        .expect("api key exists");
    ctx.state
        .user_store
        .update_api_key(
            &key.id,
            monoize::users::UpdateApiKeyInput {
                name: None,
                enabled: None,
                sub_account_enabled: None,
                sub_account_balance_nano_usd: None,
                model_limits_enabled: None,
                model_limits: None,
                ip_whitelist: None,
                use_user_group: None,
                group_ids: None,
                max_multiplier: None,
                transforms: None,
                model_redirects: None,
                reasoning_envelope_enabled: None,
                request_capture_mode: Some(monoize::users::RequestCaptureMode::CaptureAll),
                request_capture_retention: None,
                expires_at: None,
            },
            false,
        )
        .await
        .expect("api key update");
}

#[tokio::test]
async fn nonstream_request_capture_writes_dump_with_sanitized_prefix() {
    let ctx = setup().await;
    enable_request_capture(&ctx).await;

    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .header("x-request-id", "../evil42")
        .body(Body::from(
            json!({
                "model": "gpt-5-mini",
                "input": "capture me"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let dump_dir = dumps_dir(&ctx);
    let names = wait_for_dump_files(&dump_dir, 1).await;
    assert_eq!(names.len(), 1);
    let filename = &names[0];
    assert!(filename.starts_with("___evil4_"));
    // RCD-Z1: new dumps carry the compressed extension and a zstd frame.
    assert!(filename.ends_with(".json.zst"));
    let raw = fs::read(dump_dir.join(filename)).expect("disk read");
    assert_eq!(raw[..4], [0x28, 0xB5, 0x2F, 0xFD]);
    let dump = read_dump(&ctx, filename).await;
    assert_eq!(dump["request_id"].as_str(), Some("../evil42"));
    assert_eq!(
        dump["attempts"][0]["raw_input"]["input"].as_str(),
        Some("capture me")
    );
    // RCD-D10b: non-streaming attempts have no URP reconstruction.
    assert!(dump["attempts"][0]["reconstructed_urp_response"].is_null());
}

#[tokio::test]
async fn streaming_request_capture_records_downstream_sse_frames() {
    let ctx = setup().await;
    enable_request_capture(&ctx).await;

    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .header("x-request-id", "stream123")
        .body(Body::from(
            json!({
                "model": "gpt-5-mini",
                "input": "stream capture",
                "stream": true,
                "emit_usage": true
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let _bytes = resp.into_body().collect().await.unwrap().to_bytes();

    let dump_dir = dumps_dir(&ctx);
    let names = wait_for_dump_files(&dump_dir, 1).await;
    let dump = read_dump(&ctx, names.last().expect("dump name")).await;
    let frames = dump["attempts"][0]["downstream_sse_frames"]
        .as_array()
        .expect("frames array");
    assert!(!frames.is_empty());
    assert!(frames.iter().any(|frame| {
        frame
            .as_str()
            .is_some_and(|s| s.contains("response.output_text.delta"))
    }));
    assert!(
        frames
            .iter()
            .any(|frame| frame.as_str().is_some_and(|s| s.contains("[DONE]")))
    );
    // RCD-D10a: the post-transform terminal response_done event is retained
    // as the non-stream URP reconstruction.
    let reconstructed = &dump["attempts"][0]["reconstructed_urp_response"];
    assert!(
        reconstructed.is_object(),
        "reconstructed response: {reconstructed:?}"
    );
    assert!(reconstructed["output"].is_array());
}

#[tokio::test]
async fn streaming_request_capture_records_downstream_error_sse_frames() {
    let ctx = setup().await;
    enable_request_capture(&ctx).await;

    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .header("x-request-id", "streamerr")
        .body(Body::from(
            json!({
                "model": "gpt-5-mini",
                "input": "stream capture error",
                "stream": true,
                "stream_mode": "error_event"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("event: response.failed"),
        "downstream stream: {text}"
    );

    let dump_dir = dumps_dir(&ctx);
    let names = wait_for_dump_files(&dump_dir, 1).await;
    let dump = read_dump(&ctx, names.last().expect("dump name")).await;
    let frames = dump["attempts"][0]["downstream_sse_frames"]
        .as_array()
        .expect("frames array");
    assert!(
        frames.iter().any(|frame| {
            frame.as_str().is_some_and(|s| {
                s.contains("event: response.failed") && s.contains("mock_stream_error")
            })
        }),
        "captured frames: {frames:?}"
    );
}

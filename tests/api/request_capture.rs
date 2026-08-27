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

/// RCD-C9 first condition: flip only the global capture switch.
async fn enable_global_request_capture(ctx: &TestContext) {
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
}

/// RCD-C9 second condition: set the per-key capture mode.
async fn set_api_key_capture_mode(ctx: &TestContext, mode: monoize::users::RequestCaptureMode) {
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
                request_capture_mode: Some(mode),
                request_capture_retention: None,
                expires_at: None,
            },
            false,
        )
        .await
        .expect("api key update");
}

async fn enable_request_capture(ctx: &TestContext) {
    enable_global_request_capture(ctx).await;
    set_api_key_capture_mode(ctx, monoize::users::RequestCaptureMode::CaptureAll).await;
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

const TEST_PNG_B64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9p4N2VwAAAAASUVORK5CYII=";

/// RCD-D17: an `input_image` data URL in a Responses request survives into
/// `raw_input`, `transformed_urp_request`, and `upstream_request` byte-exact.
#[tokio::test]
async fn nonstream_capture_retains_multimodal_image_input_unredacted() {
    let ctx = setup().await;
    enable_request_capture(&ctx).await;
    let data_url = format!("data:image/png;base64,{TEST_PNG_B64}");

    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .header("x-request-id", "mmcap001")
        .body(Body::from(
            json!({
                "model": "gpt-5-mini",
                "input": [{
                    "type": "message",
                    "role": "user",
                    "content": [
                        { "type": "input_text", "text": "describe this image" },
                        { "type": "input_image", "image_url": data_url }
                    ]
                }]
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let dump_dir = dumps_dir(&ctx);
    let names = wait_for_dump_files(&dump_dir, 1).await;
    let dump = read_dump(&ctx, &names[0]).await;
    let attempt = &dump["attempts"][0];
    assert_eq!(
        attempt["raw_input"]["input"][0]["content"][1]["image_url"].as_str(),
        Some(data_url.as_str())
    );
    // The URP image node keeps the full data URL payload, not a placeholder.
    let urp_input = attempt["transformed_urp_request"]["input"]
        .as_array()
        .expect("urp input array");
    let image_node = urp_input
        .iter()
        .find(|node| node["type"].as_str() == Some("image"))
        .expect("urp image node");
    assert_eq!(
        image_node["source"]["url"].as_str(),
        Some(data_url.as_str()),
        "urp image node: {image_node}"
    );
    assert!(
        serde_json::to_string(&attempt["upstream_request"])
            .expect("upstream json")
            .contains(TEST_PNG_B64),
        "upstream_request lost the image payload"
    );
}

/// RCD-C16/RCD-D2b/RCD-D2c/RCD-D10b for the JSON generations endpoint routed
/// to an `openai_image` upstream.
#[tokio::test]
async fn image_generations_capture_writes_dump_for_openai_image_upstream() {
    let ctx = setup().await;
    let (upstream_addr, _, _) = start_upstream().await;
    create_test_provider(
        &ctx.state,
        "img-capture-gen",
        monoize::monoize_routing::MonoizeProviderType::OpenaiImage,
        "gpt-image-capture-gen",
        &format!("http://{upstream_addr}"),
        "upstream-key",
    )
    .await;
    seed_test_model_pricing(&ctx.state, &["gpt-image-capture-gen"]).await;
    enable_request_capture(&ctx).await;

    let body = json!({
        "model": "gpt-image-capture-gen",
        "prompt": "draw a cat",
        "size": "1024x1024"
    });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/images/generations")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .header("x-request-id", "imgen001")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let dump_dir = dumps_dir(&ctx);
    let names = wait_for_dump_files(&dump_dir, 1).await;
    let dump = read_dump(&ctx, &names[0]).await;
    // RCD-C16: n == 1 keeps the downstream request id unsuffixed.
    assert_eq!(dump["request_id"].as_str(), Some("imgen001"));
    assert_eq!(
        dump["downstream_protocol"].as_str(),
        Some("image_generations")
    );
    assert_eq!(dump["is_stream"].as_bool(), Some(false));
    let attempt = &dump["attempts"][0];
    assert_eq!(attempt["raw_input"], body);
    assert_eq!(
        attempt["upstream_path"].as_str(),
        Some("/v1/images/generations")
    );
    assert_eq!(
        attempt["upstream_request"]["model"].as_str(),
        Some("gpt-image-capture-gen")
    );
    assert_eq!(
        attempt["upstream_request"]["prompt"].as_str(),
        Some("draw a cat")
    );
    assert_eq!(
        attempt["upstream_request"]["size"].as_str(),
        Some("1024x1024")
    );
    assert_eq!(
        attempt["downstream_response"]["data"][0]["b64_json"].as_str(),
        Some(TEST_PNG_B64)
    );
    // RCD-D10b: an attempt with a provider JSON body has no reconstruction.
    assert!(attempt["reconstructed_urp_response"].is_null());
}

/// RCD-D4a + RCD-D6a + RCD-D16: the edits ingress multipart body and the
/// sent upstream multipart form are both captured as multipart objects.
#[tokio::test]
async fn image_edits_capture_records_multipart_raw_input_and_upstream_request() {
    let ctx = setup().await;
    let (upstream_addr, _, _) = start_upstream().await;
    create_test_provider(
        &ctx.state,
        "img-capture-edit",
        monoize::monoize_routing::MonoizeProviderType::OpenaiImage,
        "gpt-image-capture-edit",
        &format!("http://{upstream_addr}"),
        "upstream-key",
    )
    .await;
    seed_test_model_pricing(&ctx.state, &["gpt-image-capture-edit"]).await;
    enable_request_capture(&ctx).await;

    let png = base64::engine::general_purpose::STANDARD
        .decode(TEST_PNG_B64)
        .unwrap();
    let boundary = "----monoize-edit-capture-test";
    let mut req_body = Vec::new();
    req_body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\ngpt-image-capture-edit\r\n").as_bytes());
    req_body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"prompt\"\r\n\r\nedit this image\r\n").as_bytes());
    req_body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"size\"\r\n\r\n1024x1024\r\n"
        )
        .as_bytes(),
    );
    req_body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"image\"; filename=\"one.png\"\r\nContent-Type: image/png\r\n\r\n").as_bytes());
    req_body.extend_from_slice(&png);
    req_body.extend_from_slice(b"\r\n");
    req_body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"mask\"; filename=\"mask.png\"\r\nContent-Type: image/png\r\n\r\n").as_bytes());
    req_body.extend_from_slice(&png);
    req_body.extend_from_slice(b"\r\n");
    req_body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

    let req = Request::builder()
        .method("POST")
        .uri("/v1/images/edits")
        .header(
            CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .header("x-request-id", "imedit01")
        .body(Body::from(req_body))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let dump_dir = dumps_dir(&ctx);
    let names = wait_for_dump_files(&dump_dir, 1).await;
    let dump = read_dump(&ctx, &names[0]).await;
    assert_eq!(dump["downstream_protocol"].as_str(), Some("image_edits"));
    assert_eq!(dump["is_stream"].as_bool(), Some(false));
    let attempt = &dump["attempts"][0];

    // RCD-D4a: ingress parts in wire order with exact wire values.
    let raw_input = &attempt["raw_input"];
    assert_eq!(
        raw_input["content_type"].as_str(),
        Some("multipart/form-data")
    );
    let raw_parts = raw_input["parts"].as_array().expect("raw parts");
    assert_eq!(
        raw_parts[0],
        json!({ "name": "model", "text": "gpt-image-capture-edit" })
    );
    assert_eq!(
        raw_parts[1],
        json!({ "name": "prompt", "text": "edit this image" })
    );
    assert_eq!(raw_parts[2], json!({ "name": "size", "text": "1024x1024" }));
    assert_eq!(
        raw_parts[3],
        json!({
            "name": "image",
            "filename": "one.png",
            "part_content_type": "image/png",
            "byte_length": png.len(),
            "data_base64": TEST_PNG_B64
        })
    );
    assert_eq!(
        raw_parts[4],
        json!({
            "name": "mask",
            "filename": "mask.png",
            "part_content_type": "image/png",
            "byte_length": png.len(),
            "data_base64": TEST_PNG_B64
        })
    );

    // RCD-D6a: the sent upstream form mirrored part-for-part.
    let upstream = &attempt["upstream_request"];
    assert_eq!(
        upstream["content_type"].as_str(),
        Some("multipart/form-data")
    );
    let upstream_parts = upstream["parts"].as_array().expect("upstream parts");
    let text_part = |name: &str| {
        upstream_parts
            .iter()
            .find(|part| part["name"].as_str() == Some(name) && part.get("text").is_some())
            .unwrap_or_else(|| panic!("missing upstream text part {name}: {upstream_parts:?}"))
    };
    assert_eq!(
        text_part("model")["text"].as_str(),
        Some("gpt-image-capture-edit")
    );
    assert_eq!(
        text_part("prompt")["text"].as_str(),
        Some("edit this image")
    );
    assert_eq!(text_part("size")["text"].as_str(), Some("1024x1024"));
    let file_part = |name: &str| {
        upstream_parts
            .iter()
            .find(|part| part["name"].as_str() == Some(name) && part.get("data_base64").is_some())
            .unwrap_or_else(|| panic!("missing upstream file part {name}: {upstream_parts:?}"))
    };
    let image_part = file_part("image");
    assert_eq!(image_part["part_content_type"].as_str(), Some("image/png"));
    assert_eq!(image_part["data_base64"].as_str(), Some(TEST_PNG_B64));
    assert_eq!(image_part["byte_length"].as_u64(), Some(png.len() as u64));
    let mask_part = file_part("mask");
    assert_eq!(mask_part["part_content_type"].as_str(), Some("image/png"));
    assert_eq!(mask_part["data_base64"].as_str(), Some(TEST_PNG_B64));

    assert_eq!(
        attempt["downstream_response"]["data"][0]["b64_json"].as_str(),
        Some(TEST_PNG_B64)
    );
}

/// RCD-C16 + RCD-D4b + RCD-S6a: `n == 2` produces two dump files (no
/// same-millisecond filename collision) with per-sub-request ids and a shared
/// downstream `raw_input`.
#[tokio::test]
async fn image_generations_fan_out_writes_one_dump_per_subrequest() {
    let ctx = setup().await;
    let (upstream_addr, _, _) = start_upstream().await;
    create_test_provider(
        &ctx.state,
        "img-capture-fan",
        monoize::monoize_routing::MonoizeProviderType::OpenaiImage,
        "gpt-image-capture-fan",
        &format!("http://{upstream_addr}"),
        "upstream-key",
    )
    .await;
    seed_test_model_pricing(&ctx.state, &["gpt-image-capture-fan"]).await;
    enable_request_capture(&ctx).await;

    let body = json!({
        "model": "gpt-image-capture-fan",
        "prompt": "draw two cats",
        "n": 2
    });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/images/generations")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .header("x-request-id", "imgfan01")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let dump_dir = dumps_dir(&ctx);
    let names = wait_for_dump_files(&dump_dir, 2).await;
    assert_eq!(names.len(), 2, "dump files: {names:?}");
    let mut request_ids = Vec::new();
    for name in &names {
        let dump = read_dump(&ctx, name).await;
        request_ids.push(dump["request_id"].as_str().expect("request id").to_string());
        // RCD-D4b: every sub-request dump shares the one downstream body.
        assert_eq!(dump["attempts"][0]["raw_input"], body);
        assert_eq!(dump["is_stream"].as_bool(), Some(false));
    }
    request_ids.sort();
    assert_eq!(request_ids, vec!["imgfan01:img:0", "imgfan01:img:1"]);
}

/// RCD-D10c: an image sub-request served by a Responses provider through the
/// internal stream-collected path records a reconstruction and null
/// `downstream_response` / `downstream_sse_frames`.
#[tokio::test]
async fn stream_collected_image_generation_capture_reconstructs_urp_response() {
    let ctx = setup().await;
    enable_request_capture(&ctx).await;

    let req = Request::builder()
        .method("POST")
        .uri("/v1/images/generations")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .header("x-request-id", "imgstrm1")
        .body(Body::from(
            json!({
                "model": "gpt-5-mini",
                "prompt": "draw a cat",
                "stream_mode": "image_generation_completed"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let dump_dir = dumps_dir(&ctx);
    let names = wait_for_dump_files(&dump_dir, 1).await;
    let dump = read_dump(&ctx, &names[0]).await;
    assert_eq!(
        dump["downstream_protocol"].as_str(),
        Some("image_generations")
    );
    // RCD-D2c: Image API dumps are never is_stream even though the upstream
    // leg streamed.
    assert_eq!(dump["is_stream"].as_bool(), Some(false));
    let attempt = &dump["attempts"][0];
    assert_eq!(attempt["upstream_request"]["stream"].as_bool(), Some(true));
    assert!(attempt["downstream_response"].is_null());
    assert!(attempt["downstream_sse_frames"].is_null());
    let reconstructed = &attempt["reconstructed_urp_response"];
    assert!(
        reconstructed.is_object(),
        "reconstructed response: {reconstructed:?}"
    );
    let output = reconstructed["output"].as_array().expect("output array");
    assert!(!output.is_empty());
    assert!(
        serde_json::to_string(reconstructed)
            .expect("reconstruction json")
            .contains(TEST_PNG_B64),
        "reconstruction lost the generated image payload"
    );
}

/// RCD-C9 second condition on the Image API path: the global switch alone
/// does not capture when the key's mode is `off`.
#[tokio::test]
async fn image_generations_capture_respects_per_key_mode_off() {
    let ctx = setup().await;
    let (upstream_addr, _, _) = start_upstream().await;
    create_test_provider(
        &ctx.state,
        "img-capture-off",
        monoize::monoize_routing::MonoizeProviderType::OpenaiImage,
        "gpt-image-capture-off",
        &format!("http://{upstream_addr}"),
        "upstream-key",
    )
    .await;
    seed_test_model_pricing(&ctx.state, &["gpt-image-capture-off"]).await;
    enable_global_request_capture(&ctx).await;

    let (status, body) = json_post(
        &ctx,
        "/v1/images/generations",
        json!({
            "model": "gpt-image-capture-off",
            "prompt": "draw a cat"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // RCD-Z3 writes are asynchronous, so absence is checked after a grace
    // period instead of immediately.
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let dump_dir = dumps_dir(&ctx);
    let names: Vec<String> = fs::read_dir(&dump_dir)
        .ok()
        .map(|dir| {
            dir.filter_map(|entry| entry.ok())
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|name| !name.contains(".tmp."))
                .collect()
        })
        .unwrap_or_default();
    assert!(names.is_empty(), "unexpected dumps: {names:?}");
}

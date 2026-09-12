use super::*;
use serde_json::{Value, json};

fn response(parts: Vec<Value>) -> Value {
    json!({"responseId":"gemini-features", "modelVersion":"gemini-test",
        "candidates":[{"content":{"role":"model", "parts":parts}, "finishReason":"STOP"}]})
}

fn media_request(nodes: Vec<Node>, stream: bool) -> UrpRequest {
    let mut req = decode::gemini::decode_request(&json!({"contents":[]})).unwrap();
    req.input = nodes;
    req.stream = Some(stream);
    req
}

fn media_node(source: FileSource) -> Node {
    Node::File {
        id: None,
        role: OrdinaryRole::User,
        source,
        metadata: Default::default(),
        extra_body: Default::default(),
    }
}

#[tokio::test]
async fn gemini_compatible_part_shapes_preserve_order_and_repeated_media() {
    let image = json!({"inlineData":{"mimeType":"image/png","data":"aW1hZ2U="}});
    let audio = json!({"inlineData":{"mimeType":"audio/wav","data":"YXVkaW8="}});
    let video = json!({"inlineData":{"mimeType":"video/mp4","data":"dmlkZW8="}});
    let pdf = json!({"inlineData":{"mimeType":"application/pdf","data":"JVBERi0xLjc="}});
    let url = json!({"fileData":{"fileUri":"https://example.com/download?id=42"}});
    for (shape, expected) in [
        (json!("single"), vec![json!({"text":"single"})]),
        (image.clone(), vec![image.clone()]),
        (
            json!([image, "between", image, audio, video, pdf, url]),
            vec![
                image.clone(),
                json!({"text":"between"}),
                image.clone(),
                audio,
                video,
                pdf,
                url,
            ],
        ),
    ] {
        for stream in [false, true] {
            let req = decode::gemini::decode_request(&json!({
                "stream":stream,"contents":[{"role":"user","parts":shape}]
            }))
            .unwrap();
            let wire = encode::gemini::encode_request_checked(&req, "gemini-test").unwrap();
            assert_eq!(wire["contents"][0]["parts"], json!(expected));
            assert_eq!(req.input.len(), expected.len());
        }
        let mut native = response(vec![]);
        native["candidates"][0]["content"]["parts"] = shape;
        let events = stream_decode(&[native.clone()]).await.unwrap();
        for canonical in [
            decode::gemini::decode_response(&native).unwrap(),
            terminal(&events),
        ] {
            let wire = encode::gemini::encode_response_checked(&canonical, "gemini-test").unwrap();
            assert_eq!(wire["candidates"][0]["content"]["parts"], json!(expected));
            assert_eq!(canonical.output.len(), expected.len());
        }
        let mut encoder = stream_encode::gemini::GeminiStreamEncoder::new("gemini-test");
        let frames = events
            .into_iter()
            .flat_map(|event| encoder.push_event(event).unwrap())
            .collect::<Vec<_>>();
        let canonical = terminal(&stream_decode(&frames).await.unwrap());
        assert_eq!(
            encode::gemini::encode_response_checked(&canonical, "gemini-test").unwrap()["candidates"]
                [0]["content"]["parts"],
            json!(expected)
        );
    }
}

#[tokio::test]
async fn gemini_compatible_audio_aliases_use_typed_nodes_and_nested_files() {
    for (part, mime, source) in [
        (
            json!({"type":"input_audio","input_audio":{"format":"wav","data":"YXVkaW8="}}),
            "audio/wav",
            AudioSource::Base64 {
                media_type: "audio/wav".into(),
                data: "YXVkaW8=".into(),
            },
        ),
        (
            json!({"type":"audio","source":{"type":"url","url":"https://example.com/audio.wav"},"media_type":"audio/wav"}),
            "audio/wav",
            AudioSource::Url {
                url: "https://example.com/audio.wav".into(),
            },
        ),
    ] {
        let native = response(vec![part.clone()]);
        let streamed = terminal(&stream_decode(&[native.clone()]).await.unwrap());
        for canonical in [decode::gemini::decode_response(&native).unwrap(), streamed] {
            let Node::Audio { source: actual, .. } = &canonical.output[0] else {
                panic!("audio must be typed")
            };
            assert_eq!(actual, &source);
            let wire = encode::gemini::encode_response_checked(&canonical, "gemini-test").unwrap();
            assert_eq!(
                wire["candidates"][0]["content"]["parts"][0]
                    .get(if matches!(source, AudioSource::Url { .. }) {
                        "fileData"
                    } else {
                        "inlineData"
                    })
                    .unwrap()["mimeType"],
                mime
            );
        }
        for stream in [false, true] {
            let req = decode::gemini::decode_request(&json!({"stream":stream,"contents":[{"parts":{"functionResponse":{"id":"call-1","name":"audio","response":{},"parts":part}}}]})).unwrap();
            let Node::ToolResult { content, .. } = &req.input[0] else {
                panic!()
            };
            assert_eq!(content.len(), 2);
            let ToolResultContent::File {
                source: actual,
                metadata,
                ..
            } = &content[1]
            else {
                panic!("nested audio must be File")
            };
            match &source {
                AudioSource::Base64 { media_type, data } => assert_eq!(
                    actual,
                    &FileSource::Base64 {
                        media_type: media_type.clone(),
                        data: data.clone()
                    }
                ),
                AudioSource::Url { url } => {
                    assert_eq!(actual, &FileSource::Url { url: url.clone() });
                    assert_eq!(metadata.media_type.as_deref(), Some(mime));
                }
            }
            let wire = encode::gemini::encode_request_checked(&req, "gemini-test");
            if matches!(source, AudioSource::Url { .. }) {
                assert!(wire.is_err());
            } else {
                assert_eq!(
                    wire.unwrap()["contents"][0]["parts"][0]["functionResponse"]["parts"][0]["inlineData"]
                        ["data"],
                    "YXVkaW8="
                );
            }
        }
    }
}

#[tokio::test]
async fn gemini_nested_function_parts_preserve_media_and_compatible_text() {
    let parts = json!([
        {"inlineData":{"mimeType":"image/png","data":"aW1hZ2U="}}, "between",
        {"inlineData":{"mimeType":"audio/wav","data":"YXVkaW8="}},
        {"inlineData":{"mimeType":"video/mp4","data":"dmlkZW8="}},
        {"inlineData":{"mimeType":"application/pdf","data":"JVBERi0xLjc="}}
    ]);
    let part = json!({"functionResponse":{"id":"call-1","name":"inspect","response":{"ok":true},"parts":parts}});
    let native = response(vec![part.clone()]);
    for canonical in [
        decode::gemini::decode_response(&native).unwrap(),
        terminal(&stream_decode(&[native]).await.unwrap()),
    ] {
        let Node::ToolResult { content, .. } = &canonical.output[0] else {
            panic!()
        };
        assert_eq!(content.len(), 6);
        assert!(matches!(&content[2],ToolResultContent::Text{text,..} if text=="between"));
        let wire = encode::gemini::encode_response_checked(&canonical, "gemini-test").unwrap();
        let result = &wire["candidates"][0]["content"]["parts"][0]["functionResponse"];
        assert!(
            result["response"]["result"]
                .as_str()
                .unwrap()
                .ends_with("between")
        );
        assert_eq!(
            result["parts"],
            json!([parts[0], parts[2], parts[3], parts[4]])
        );
    }
    for stream in [false, true] {
        let req =
            decode::gemini::decode_request(&json!({"stream":stream,"contents":[{"parts":part}]}))
                .unwrap();
        assert_eq!(
            encode::gemini::encode_request_checked(&req, "gemini-test").unwrap()["contents"][0]["parts"]
                [0]["functionResponse"]["parts"],
            json!([parts[0], parts[2], parts[3], parts[4]])
        );
    }
}

#[tokio::test]
async fn gemini_nested_audio_urls_preserve_private_provenance_before_target_rejection() {
    for (url, private) in [
        ("https://example.com/audio.wav", false),
        (
            "https://generativelanguage.googleapis.com/v1beta/files/audio-1",
            true,
        ),
        ("gs://bucket/audio.wav", true),
    ] {
        let audio = json!({"fileData":{"mimeType":"audio/wav","fileUri":url},"thoughtSignature":"media-signature"});
        let nested = json!({"functionResponse":{"id":"call-1","name":"audio","response":{},"parts":[audio]}});
        let native = response(vec![nested.clone()]);
        for canonical in [
            decode::gemini::decode_response(&native).unwrap(),
            terminal(&stream_decode(&[native]).await.unwrap()),
        ] {
            let Node::ToolResult { content, .. } = &canonical.output[0] else {
                panic!()
            };
            assert_eq!(content.len(), 2);
            let ToolResultContent::File {
                source, metadata, ..
            } = &content[1]
            else {
                panic!()
            };
            assert_eq!(source, &FileSource::Url { url: url.into() });
            assert_eq!(metadata.media_type.as_deref(), Some("audio/wav"));
            assert_eq!(metadata.signature, Some(json!("media-signature")));
            assert_eq!(
                metadata.resource.as_ref().map(|r| r.protocol),
                private.then_some(ProviderProtocol::Gemini)
            );
            assert!(encode::gemini::encode_response_checked(&canonical, "gemini-test").is_err());
        }
        for stream in [false, true] {
            let mut req = decode::gemini::decode_request(
                &json!({"stream":stream,"contents":[{"parts":[audio,nested]}]}),
            )
            .unwrap();
            assert_eq!(
                media::resources(&req.input).unwrap().len(),
                if private { 2 } else { 0 }
            );
            assert!(encode::gemini::encode_request_checked(&req, "gemini-test").is_err());
            if private {
                let scope = MediaResource {
                    protocol: ProviderProtocol::Gemini,
                    provider_id: Some("provider-a".into()),
                    channel_id: Some("channel-a".into()),
                    credential_scope: Some("credential-a".into()),
                };
                media::bind_resources(&mut req.input, &scope);
                assert!(
                    media::resources(&req.input)
                        .unwrap()
                        .iter()
                        .all(|resource| resource == &scope)
                );
                if let Node::Audio { metadata, .. } = &mut req.input[0] {
                    metadata.resource = None;
                }
                assert!(media::resources(&req.input).is_err());
            }
        }
    }
}

#[test]
fn gemini_system_media_is_typed_before_target_capability_check() {
    for stream in [false, true] {
        let req=decode::gemini::decode_request(&json!({"stream":stream,"systemInstruction":{"parts":["before",{"inlineData":{"mimeType":"image/png","data":"aW1hZ2U="}},"after"]},"contents":[]})).unwrap();
        assert_eq!(req.input.len(), 3);
        assert!(
            matches!(&req.input[0],Node::Text{role:OrdinaryRole::System,content,..} if content=="before")
        );
        assert!(matches!(
            &req.input[1],
            Node::Image {
                role: OrdinaryRole::System,
                ..
            }
        ));
        assert!(
            matches!(&req.input[2],Node::Text{role:OrdinaryRole::System,content,..} if content=="after")
        );
        assert!(
            encode::gemini::encode_request_checked(&req, "gemini-test")
                .unwrap_err()
                .contains("system")
        );
    }
}

#[tokio::test]
async fn gemini_malformed_media_fails_at_content_boundaries_only() {
    for malformed in [
        json!({"inlineData":false}),
        json!({"inlineData":{"mimeType":"image/png"}}),
        json!({"inlineData":{"data":"aW1hZ2U="}}),
        json!({"fileData":{"mimeType":"audio/wav"}}),
        json!({"fileData":{"fileUri":"https://example.com/file","mimeType":42}}),
        json!({"type":"input_audio","input_audio":{"format":"wav"}}),
    ] {
        for part in [
            malformed.clone(),
            json!({"functionResponse":{"name":"f","response":{},"parts":[malformed]}}),
        ] {
            for stream in [false, true] {
                assert!(
                    decode::gemini::decode_request(
                        &json!({"stream":stream,"contents":[{"parts":[{"text":"before"},part]}]})
                    )
                    .is_err()
                );
            }
            let native = response(vec![part.clone()]);
            assert!(decode::gemini::decode_response(&native).is_err());
            assert!(stream_decode(&[native]).await.is_err());
        }
        let arbitrary = json!({"contents":[{"role":"model","parts":[{"functionCall":{"id":"call-1","name":"f","args":malformed}}]},{"role":"user","parts":[{"functionResponse":{"id":"call-1","name":"f","response":malformed}}]}]});
        let req = decode::gemini::decode_request(&arbitrary).unwrap();
        let wire = encode::gemini::encode_request_checked(&req, "gemini-test").unwrap();
        assert_eq!(
            wire["contents"][0]["parts"][0]["functionCall"]["args"],
            malformed
        );
        assert_eq!(
            wire["contents"][1]["parts"][0]["functionResponse"]["response"],
            malformed
        );
    }
}

#[tokio::test]
async fn gemini_nested_tool_and_reasoning_parts_fail_explicitly() {
    for part in [
        json!({"functionCall":{"name":"nested","args":{}}}),
        json!({"text":"nested thought","thought":true}),
    ] {
        let nested =
            json!({"functionResponse":{"id":"call-1","name":"f","response":{},"parts":[part]}});
        for stream in [false, true] {
            let error = decode::gemini::decode_request(
                &json!({"stream":stream,"contents":[{"parts":[nested]}]}),
            )
            .unwrap_err();
            assert!(error.contains("unsupported nested"));
        }
        let native = response(vec![nested]);
        assert!(
            decode::gemini::decode_response(&native)
                .unwrap_err()
                .contains("unsupported nested")
        );
        assert!(stream_decode(&[native]).await.is_err());
    }
}

#[tokio::test]
async fn gemini_media_only_terminal_keeps_prior_text_citations() {
    use axum::response::IntoResponse;
    use futures_util::StreamExt;

    let mut initial = response(vec![json!({"text":"answer"})]);
    initial["candidates"][0]
        .as_object_mut()
        .unwrap()
        .remove("finishReason");
    let image = json!({"inlineData":{"mimeType":"image/png","data":"aW1hZ2U="}});
    let mut last = response(vec![image.clone(), image.clone()]);
    let citation = json!({"uri":"https://example.com/source","startIndex":0,"endIndex":6});
    last["candidates"][0]["citationMetadata"] = json!({"citationSources":[citation]});
    let events = stream_decode(&[initial, last]).await.unwrap();
    let canonical = terminal(&events);
    assert_eq!(canonical.output.len(), 3);
    assert!(
        matches!(&canonical.output[0],Node::Text{citations,..} if citations==&vec![citation.clone()])
    );
    assert!(matches!(&canonical.output[1], Node::Image { .. }));
    assert!(matches!(&canonical.output[2], Node::Image { .. }));
    assert_eq!(events.iter().filter(|event| matches!(event,UrpStreamEvent::NodeDelta{node_index:0,delta:NodeDelta::Text{content,citations,..},..} if content.is_empty() && citations==&vec![citation.clone()])).count(),1);
    let (tx, rx) = tokio::sync::mpsc::channel(32);
    for event in events.iter().cloned() {
        tx.send(event).await.unwrap();
    }
    drop(tx);
    let (wire_tx, wire_rx) = tokio::sync::mpsc::channel(32);
    // Messages emits the citation before rejecting the following unsupported response image.
    assert!(
        stream_encode::anthropic::encode_urp_stream_as_messages(
            rx,
            wire_tx,
            "gemini-test",
            None,
            false
        )
        .await
        .is_err()
    );
    let body =
        tokio_stream::wrappers::ReceiverStream::new(wire_rx).map(Ok::<_, std::convert::Infallible>);
    let bytes = axum::body::to_bytes(
        axum::response::Sse::new(body).into_response().into_body(),
        usize::MAX,
    )
    .await
    .unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert_eq!(text.matches("\"citations_delta\"").count(), 1, "{text}");
    assert_eq!(
        text.matches("https://example.com/source").count(),
        1,
        "{text}"
    );
    assert!(text.contains("\"error\""), "{text}");
    assert!(!text.contains("\"end_turn\""), "{text}");
    let mut encoder = stream_encode::gemini::GeminiStreamEncoder::new("gemini-test");
    let frames = events
        .into_iter()
        .flat_map(|event| encoder.push_event(event).unwrap())
        .collect::<Vec<_>>();
    let wire = encode::gemini::encode_response_checked(
        &terminal(&stream_decode(&frames).await.unwrap()),
        "gemini-test",
    )
    .unwrap();
    assert_eq!(
        wire["candidates"][0]["content"]["parts"],
        json!([{"text":"answer"},image,image])
    );
    assert_eq!(
        wire["candidates"][0]["citationMetadata"]["citationSources"],
        json!([citation])
    );
}

#[tokio::test]
async fn gemini_mime_classification_accepts_case_and_parameters_without_losing_metadata() {
    for (mime, kind) in [
        ("IMAGE/PNG;profile=example", "image"),
        ("AUDIO/WAV;rate=16000", "audio"),
        ("VIDEO/AUDIO/WAV;rate=16000", "audio"),
        ("APPLICATION/PDF", "file"),
        ("VIDEO/MP4;codecs=avc1", "file"),
    ] {
        let video = mime.starts_with("VIDEO/MP4");
        let mut part = json!({"inlineData":{"mimeType":mime,"data":"AAECAw=="}});
        if video {
            part["videoMetadata"] = json!({"startOffset":"1s"});
        }
        let native = response(vec![part.clone()]);
        for canonical in [
            decode::gemini::decode_response(&native).unwrap(),
            terminal(&stream_decode(&[native]).await.unwrap()),
        ] {
            assert!(
                matches!((&canonical.output[0],kind), (Node::Image{source:ImageSource::Base64{media_type,..},..},"image") | (Node::Audio{source:AudioSource::Base64{media_type,..},..},"audio") | (Node::File{source:FileSource::Base64{media_type,..},..},"file") if media_type==mime)
            );
            let wire = encode::gemini::encode_response_checked(&canonical, "gemini-test").unwrap();
            let encoded = &wire["candidates"][0]["content"]["parts"][0];
            assert_eq!(encoded["inlineData"]["data"], "AAECAw==");
            assert_eq!(
                encoded["inlineData"]["mimeType"],
                if kind == "image" { "image/png" } else { mime }
            );
            if video {
                assert_eq!(encoded["videoMetadata"], part["videoMetadata"]);
            }
        }
        for stream in [false, true] {
            let req = decode::gemini::decode_request(
                &json!({"stream":stream,"contents":[{"parts":[part]}]}),
            )
            .unwrap();
            let wire = encode::gemini::encode_request_checked(&req, "gemini-test").unwrap();
            assert_eq!(
                wire["contents"][0]["parts"][0]["inlineData"]["mimeType"],
                if kind == "image" { "image/png" } else { mime }
            );
            if video {
                assert_eq!(
                    wire["contents"][0]["parts"][0]["videoMetadata"],
                    part["videoMetadata"]
                );
            }
        }
    }
}

#[tokio::test]
async fn gemini_signed_compatible_media_keeps_the_media_and_one_typed_signature() {
    for part in [
        json!({"type":"input_image","image_url":"data:image/png;base64,aW1hZ2U=","thoughtSignature":"signature"}),
        json!({"type":"input_audio","input_audio":{"format":"wav","data":"YXVkaW8="},"thoughtSignature":"signature"}),
        json!({"type":"input_file","file_data":"data:application/pdf;base64,JVBERi0xLjc=","thoughtSignature":"signature"}),
    ] {
        for stream in [false, true] {
            let mut req = decode::gemini::decode_request(
                &json!({"stream":stream,"contents":[{"parts":[part]}]}),
            )
            .unwrap();
            assert_eq!(req.input.len(), 1);
            assert!(!matches!(req.input[0], Node::Reasoning { .. }));
            assert_eq!(
                encode::gemini::encode_request_checked(&req, "gemini-test").unwrap()["contents"][0]
                    ["parts"][0]["thoughtSignature"],
                "signature"
            );
            match &mut req.input[0] {
                Node::Image {
                    metadata,
                    extra_body,
                    ..
                }
                | Node::Audio {
                    metadata,
                    extra_body,
                    ..
                }
                | Node::File {
                    metadata,
                    extra_body,
                    ..
                } => {
                    assert_eq!(metadata.signature, Some(json!("signature")));
                    assert!(!extra_body.contains_key("thoughtSignature"));
                    metadata.signature = None;
                }
                _ => panic!(),
            }
            assert!(
                encode::gemini::encode_request_checked(&req, "gemini-test").unwrap()["contents"][0]
                    ["parts"][0]
                    .get("thoughtSignature")
                    .is_none()
            );
        }
        let native = response(vec![part]);
        for canonical in [
            decode::gemini::decode_response(&native).unwrap(),
            terminal(&stream_decode(&[native]).await.unwrap()),
        ] {
            assert_eq!(canonical.output.len(), 1);
            assert!(!matches!(canonical.output[0], Node::Reasoning { .. }));
            let wire = encode::gemini::encode_response_checked(&canonical, "gemini-test").unwrap();
            assert!(
                wire["candidates"][0]["content"]["parts"][0]
                    .get("inlineData")
                    .is_some()
            );
            assert_eq!(
                wire["candidates"][0]["content"]["parts"][0]["thoughtSignature"],
                "signature"
            );
        }
    }
}

#[tokio::test]
async fn gemini_custom_calls_and_results_fail_without_discarding_content() {
    use axum::response::IntoResponse;
    use futures_util::StreamExt;

    let call = Node::ToolCall {
        id: Some("custom-1".into()),
        call_id: "call-1".into(),
        namespace: None,
        signature: None,
        name: "custom".into(),
        tool_type: ToolCallType::Custom,
        arguments: "original custom input".into(),
        extra_body: Default::default(),
    };
    let result = Node::ToolResult {
        id: None,
        call_id: "call-1".into(),
        namespace: None,
        signature: None,
        name: Some("custom".into()),
        tool_type: ToolCallType::Custom,
        is_error: false,
        content: vec![ToolResultContent::Text {
            text: "original output".into(),
            extra_body: Default::default(),
        }],
        extra_body: Default::default(),
    };
    let mut media_result = result.clone();
    if let Node::ToolResult { content, .. } = &mut media_result {
        content.push(ToolResultContent::Image {
            source: ImageSource::Base64 {
                media_type: "image/png".into(),
                data: "aW1hZ2U=".into(),
            },
            metadata: Default::default(),
            extra_body: Default::default(),
        });
    }
    for node in [call, result, media_result] {
        for stream in [false, true] {
            let req = media_request(vec![node.clone()], stream);
            assert!(
                encode::gemini::encode_request_checked(&req, "gemini-test")
                    .unwrap_err()
                    .contains("custom tool")
            );
            assert!(
                encode::gemini::encode_request(&req, "gemini-test")
                    .get("error")
                    .is_some()
            );
        }
        let mut canonical = decode::gemini::decode_response(&response(vec![])).unwrap();
        canonical.output = vec![node.clone()];
        assert!(
            encode::gemini::encode_response_checked(&canonical, "gemini-test")
                .unwrap_err()
                .contains("custom tool")
        );
        assert!(
            encode::gemini::encode_response(&canonical, "gemini-test")
                .get("error")
                .is_some()
        );
        for terminal_only in [false, true] {
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            if !terminal_only {
                tx.send(UrpStreamEvent::NodeDone {
                    node_index: 0,
                    node: node.clone(),
                    usage: None,
                    extra_body: Default::default(),
                })
                .await
                .unwrap();
            }
            tx.send(UrpStreamEvent::ResponseDone {
                output: vec![node.clone()],
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                extra_body: Default::default(),
            })
            .await
            .unwrap();
            drop(tx);
            let (wire_tx, wire_rx) = tokio::sync::mpsc::channel(8);
            let error =
                stream_encode::gemini::encode_urp_stream_as_gemini(rx, wire_tx, "gemini-test")
                    .await
                    .unwrap_err();
            assert!(error.downstream_stream_terminal_sent);
            let body = tokio_stream::wrappers::ReceiverStream::new(wire_rx)
                .map(Ok::<_, std::convert::Infallible>);
            let bytes = axum::body::to_bytes(
                axum::response::Sse::new(body).into_response().into_body(),
                usize::MAX,
            )
            .await
            .unwrap();
            let text = String::from_utf8(bytes.to_vec()).unwrap();
            assert_eq!(text.matches("\"error\"").count(), 1, "{text}");
            assert!(!text.contains("finishReason"), "{text}");
            assert!(
                !text.contains("functionCall")
                    && !text.contains("functionResponse")
                    && !text.contains("inlineData"),
                "{text}"
            );
        }
    }
}

#[test]
fn gemini_media_requests_normalize_data_urls_and_expand_documents_in_order() {
    for stream in [false, true] {
        let image = Node::Image {
            id: None,
            role: OrdinaryRole::User,
            source: ImageSource::Url {
                url: "data:image/png;base64,aW1hZ2U=".into(),
                detail: None,
            },
            metadata: Default::default(),
            extra_body: Default::default(),
        };
        let mut document = media_node(FileSource::Content {
            content: vec![
                json!({"type":"text","text":"first"}),
                json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"aW1hZ2U="}}),
                json!({"type":"text","text":"last"}),
            ],
        });
        if let Node::File { metadata, .. } = &mut document {
            metadata.document_title = Some("Report title".into());
            metadata.document_context = Some("Report context".into());
        }
        let req = media_request(
            vec![
                image,
                document,
                media_node(FileSource::Text {
                    text: "plain".into(),
                }),
            ],
            stream,
        );
        let original = serde_json::to_value(&req).unwrap();
        let wire = encode::gemini::encode_request_checked(&req, "gemini-test").unwrap();
        let parts = wire["contents"][0]["parts"].as_array().unwrap();
        assert_eq!(
            parts[0],
            json!({"inlineData":{"mimeType":"image/png","data":"aW1hZ2U="}})
        );
        let text: String = parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect();
        for value in ["Report title", "Report context", "first", "last", "plain"] {
            assert!(text.contains(value), "missing {value}: {wire}");
        }
        assert!(text.find("first") < text.find("last"));
        assert_eq!(
            parts
                .iter()
                .filter(|part| part.get("inlineData").is_some())
                .count(),
            2
        );
        assert!(!wire.to_string().contains("data:image"));
        assert_eq!(serde_json::to_value(&req).unwrap(), original);
        let decoded = decode::gemini::decode_request(&wire).unwrap();
        assert_eq!(
            encode::gemini::encode_request_checked(&decoded, "gemini-test").unwrap(),
            wire
        );
    }
}

#[test]
fn gemini_url_requests_omit_unknown_mime_and_honor_typed_mime_changes() {
    for stream in [false, true] {
        let url = "https://example.com/download?token=opaque";
        let mut req = media_request(
            vec![media_node(FileSource::Url { url: url.into() })],
            stream,
        );
        let wire = encode::gemini::encode_request_checked(&req, "gemini-test").unwrap();
        assert_eq!(
            wire["contents"][0]["parts"][0],
            json!({"fileData":{"fileUri":url}})
        );
        let decoded = decode::gemini::decode_request(&wire).unwrap();
        assert!(
            matches!(&decoded.input[0], Node::File { metadata, source: FileSource::Url { url: decoded_url }, .. }
            if metadata.media_type.is_none() && decoded_url == url)
        );
        assert_eq!(
            encode::gemini::encode_request_checked(&decoded, "gemini-test").unwrap(),
            wire
        );

        if let Node::File {
            metadata,
            extra_body,
            ..
        } = &mut req.input[0]
        {
            metadata.media_type = Some("application/pdf".into());
            extra_body.insert(
                decode::gemini::GEMINI_PART_EXTRA_KEY.into(),
                json!({"fileData":{"mimeType":"video/mp4"}}),
            );
        }
        let wire = encode::gemini::encode_request_checked(&req, "gemini-test").unwrap();
        assert_eq!(
            wire["contents"][0]["parts"][0],
            json!({"fileData":{"mimeType":"application/pdf","fileUri":url}})
        );
        let decoded = decode::gemini::decode_request(&wire).unwrap();
        assert!(matches!(&decoded.input[0], Node::File { metadata, .. }
            if metadata.media_type.as_deref() == Some("application/pdf")));

        if let Node::File { metadata, .. } = &mut req.input[0] {
            metadata.media_type = None;
        }
        let wire = encode::gemini::encode_request_checked(&req, "gemini-test").unwrap();
        assert_eq!(
            wire["contents"][0]["parts"][0],
            json!({"fileData":{"fileUri":url}})
        );
        assert!(!wire.to_string().contains("mimeType"));
    }
}

#[test]
fn gemini_nested_function_media_rejects_url_and_file_id_without_partial_output() {
    for stream in [false, true] {
        for source in [
            json!({"type":"url","url":"https://example.com/image.png"}),
            json!({"type":"file_id","file_id":"file-123"}),
        ] {
            let node: Node = serde_json::from_value(
                json!({"type":"tool_result","call_id":"call-1","name":"screen",
                "tool_type":"function","is_error":false,"content":[{"type":"text","text":"keep"},
                {"type":"image","source":source,"metadata":{"media_type":"image/png"}}]}),
            )
            .unwrap();
            let req = media_request(vec![node], stream);
            assert!(encode::gemini::encode_request_checked(&req, "gemini-test").is_err());
            let wire = encode::gemini::encode_request(&req, "gemini-test");
            assert!(wire.get("error").is_some());
            assert!(wire.get("contents").is_none());
        }
    }
}

#[test]
fn gemini_pdf_tool_result_metadata_is_typed_and_context_remains_visible() {
    for stream in [false, true] {
        let node: Node = serde_json::from_value(json!({"type":"tool_result","call_id":"call-1","name":"document",
            "tool_type":"function","is_error":false,"content":[{"type":"file",
            "source":{"type":"base64","media_type":"application/octet-stream","data":"JVBERi0="},
            "metadata":{"filename":"report.pdf","document_title":"Title","document_context":"Context"}}]})).unwrap();
        let req = media_request(vec![node], stream);
        let wire = encode::gemini::encode_request_checked(&req, "gemini-test").unwrap();
        let result = &wire["contents"][0]["parts"][0]["functionResponse"];
        assert_eq!(
            result["parts"][0],
            json!({"inlineData":{"mimeType":"application/pdf","data":"JVBERi0="}})
        );
        let text = result["response"]["result"].as_str().unwrap();
        assert!(text.contains("Title") && text.contains("Context"), "{wire}");
        assert!(!wire.to_string().contains("displayName"));
        assert!(!wire.to_string().contains("filename"));
        let Node::ToolResult { content, .. } = &req.input[0] else {
            panic!()
        };
        let ToolResultContent::File { metadata, .. } = &content[0] else {
            panic!()
        };
        assert_eq!(metadata.filename.as_deref(), Some("report.pdf"));
    }
}

#[tokio::test]
async fn gemini_function_parent_signature_has_typed_ownership_and_deletion_wins() {
    let native = response(vec![
        json!({"functionResponse":{"id":"call-1","name":"screen","response":{"ok":true},
        "parts":[{"inlineData":{"mimeType":"image/png","data":"aW1hZ2U="}}]},
        "thoughtSignature":"cGFyZW50", "partMetadata":{"vendor":"value"}}),
    ]);
    let events = stream_decode(&[native.clone()]).await.unwrap();
    assert!(events.iter().any(|event| matches!(event,
        UrpStreamEvent::NodeStart { header: NodeHeader::ToolResult { signature: Some(value), .. }, .. }
        if value == &json!("cGFyZW50"))));
    for mut canonical in [
        decode::gemini::decode_response(&native).unwrap(),
        terminal(&events),
    ] {
        let Node::ToolResult {
            signature,
            content,
            extra_body,
            ..
        } = &mut canonical.output[0]
        else {
            panic!()
        };
        assert_eq!(signature, &Some(json!("cGFyZW50")));
        assert!(
            extra_body
                .get(decode::gemini::GEMINI_PART_EXTRA_KEY)
                .unwrap()
                .get("thoughtSignature")
                .is_none()
        );
        let ToolResultContent::Image { metadata, .. } = &content[1] else {
            panic!()
        };
        assert!(metadata.signature.is_none());
        *signature = None;
        let wire = encode::gemini::encode_response_checked(&canonical, "gemini-test").unwrap();
        let part = &wire["candidates"][0]["content"]["parts"][0];
        assert!(part.get("thoughtSignature").is_none());
        assert_eq!(part["partMetadata"], json!({"vendor":"value"}));
        assert!(part["functionResponse"].get("partMetadata").is_none());
        assert_eq!(
            part["functionResponse"]["parts"][0],
            json!({"inlineData":{"mimeType":"image/png","data":"aW1hZ2U="}})
        );
    }
}

#[tokio::test]
async fn gemini_google_audio_mimes_remain_typed_audio_in_both_directions() {
    for mime in [
        "video/audio/s16le",
        "video/audio/wav",
        "audio/pcm;rate=24000",
    ] {
        for payload in [
            json!({"inlineData":{"mimeType":mime,"data":"YXVkaW8="}}),
            json!({"fileData":{"mimeType":mime,"fileUri":"https://example.com/audio"}}),
        ] {
            let native = response(vec![payload.clone()]);
            let canonical = decode::gemini::decode_response(&native).unwrap();
            assert!(matches!(&canonical.output[0], Node::Audio { .. }));
            assert_nonstream_part(payload.clone());
            let events = stream_decode(&[native]).await.unwrap();
            assert!(events.iter().any(|event| matches!(
                event,
                UrpStreamEvent::NodeStart {
                    header: NodeHeader::Audio { .. },
                    ..
                }
            )));
            assert_stream_part(payload).await;
        }
    }
}

#[tokio::test]
async fn gemini_documented_mimes_and_youtube_wildcard_preserve_wire_values() {
    for mime in [
        "audio/aiff",
        "audio/m4a",
        "audio/l16",
        "audio/opus",
        "audio/alaw",
        "audio/mulaw",
        "audio/webm",
        "video/quicktime",
        "video/3gpp",
        "video/text/timestamp",
        "application/rtf",
    ] {
        let part = json!({"inlineData":{"mimeType":mime,"data":"AAECAw=="}});
        assert_nonstream_part(part.clone());
        assert_stream_part(part).await;
    }
    let part = json!({"fileData":{"mimeType":"video/*","fileUri":"https://www.youtube.com/watch?v=example"},
        "videoMetadata":{"startOffset":"1s"}});
    assert_nonstream_part(part.clone());
    assert_stream_part(part).await;
    for stream in [false, true] {
        for mime in ["audio/banana", "video/banana", "image/banana", "video/*"] {
            let req = media_request(
                vec![media_node(FileSource::Base64 {
                    media_type: mime.into(),
                    data: "AAECAw==".into(),
                })],
                stream,
            );
            assert!(
                encode::gemini::encode_request_checked(&req, "gemini-test").is_err(),
                "{mime}"
            );
        }
        let req = media_request(
            vec![media_node(FileSource::Base64 {
                media_type: "image/heic".into(),
                data: "AAECAw==".into(),
            })],
            stream,
        );
        let wire = encode::gemini::encode_request_checked(&req, "gemini-test").unwrap();
        assert_eq!(
            wire["contents"][0]["parts"][0]["inlineData"]["mimeType"],
            "image/heic"
        );
    }
}

#[tokio::test]
async fn gemini_media_mutation_discards_video_metadata_in_nonstream_and_stream() {
    let native = response(vec![
        json!({"fileData":{"mimeType":"video/mp4","fileUri":"https://example.com/video.mp4"},
        "videoMetadata":{"startOffset":"1s","endOffset":"3s"},"mediaProcessing":"STATIC","partMetadata":{"vendor":7}}),
    ]);
    for mut canonical in [
        decode::gemini::decode_response(&native).unwrap(),
        terminal(&stream_decode(&[native]).await.unwrap()),
    ] {
        let Node::File {
            source, metadata, ..
        } = &mut canonical.output[0]
        else {
            panic!()
        };
        *source = FileSource::Base64 {
            media_type: "application/pdf".into(),
            data: "JVBERg==".into(),
        };
        metadata.media_type = Some("application/pdf".into());
        let wire = encode::gemini::encode_response_checked(&canonical, "gemini-test").unwrap();
        let part = &wire["candidates"][0]["content"]["parts"][0];
        assert!(part.get("videoMetadata").is_none());
        assert!(part.get("mediaProcessing").is_none());
        assert_eq!(part["partMetadata"], json!({"vendor":7}));
        let mut encoder = stream_encode::gemini::GeminiStreamEncoder::new("gemini-test");
        let frames = encoder
            .push_event(UrpStreamEvent::ResponseDone {
                output: canonical.output,
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                extra_body: Default::default(),
            })
            .unwrap();
        assert!(
            frames[0]["candidates"][0]["content"]["parts"][0]
                .get("videoMetadata")
                .is_none()
        );
    }
}

#[tokio::test]
async fn gemini_unsupported_media_errors_before_success_in_nonstream_and_live() {
    use axum::response::IntoResponse;
    use futures_util::StreamExt;
    let invalid_nested = decode::gemini::decode_response(&response(vec![json!({"functionResponse":{"id":"call-1","name":"document",
        "response":{"text":"keep"},"parts":[{"fileData":{"mimeType":"application/pdf","fileUri":"https://example.com/private.pdf"}}]}})])).unwrap().output.remove(0);
    for mut node in [
        media_node(FileSource::Base64 {
            media_type: "application/octet-stream".into(),
            data: "AAECAw==".into(),
        }),
        media_node(FileSource::FileId {
            file_id: "file-private".into(),
        }),
        invalid_nested,
    ] {
        for stream in [false, true] {
            let req = media_request(vec![node.clone()], stream);
            assert!(encode::gemini::encode_request_checked(&req, "gemini-test").is_err());
        }
        if let Node::File { role, .. } = &mut node {
            *role = OrdinaryRole::Assistant;
        }
        let mut canonical = decode::gemini::decode_response(&response(vec![])).unwrap();
        canonical.output = vec![node.clone()];
        assert!(encode::gemini::encode_response_checked(&canonical, "gemini-test").is_err());
        assert!(
            encode::gemini::encode_response(&canonical, "gemini-test")
                .get("error")
                .is_some()
        );
        for terminal_only in [false, true] {
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            if !terminal_only {
                tx.send(UrpStreamEvent::NodeDone {
                    node_index: 0,
                    node: node.clone(),
                    usage: None,
                    extra_body: Default::default(),
                })
                .await
                .unwrap();
            }
            tx.send(UrpStreamEvent::ResponseDone {
                output: vec![node.clone()],
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                extra_body: Default::default(),
            })
            .await
            .unwrap();
            drop(tx);
            let (wire_tx, wire_rx) = tokio::sync::mpsc::channel(8);
            assert!(
                stream_encode::gemini::encode_urp_stream_as_gemini(rx, wire_tx, "gemini-test")
                    .await
                    .is_err()
            );
            let stream = tokio_stream::wrappers::ReceiverStream::new(wire_rx)
                .map(Ok::<_, std::convert::Infallible>);
            let bytes = axum::body::to_bytes(
                axum::response::Sse::new(stream).into_response().into_body(),
                usize::MAX,
            )
            .await
            .unwrap();
            let text = String::from_utf8(bytes.to_vec()).unwrap();
            assert!(text.contains("\"error\""), "{text}");
            assert!(!text.contains("finishReason"), "{text}");
            assert!(!text.contains("inlineData"), "{text}");
            assert!(!text.contains("fileData"), "{text}");
        }
    }
}

#[tokio::test]
async fn gemini_compound_document_response_expands_consistently_in_live_frames() {
    let mut document = media_node(FileSource::Content {
        content: vec![
            json!({"type":"text","text":"first"}),
            json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"aW1hZ2U="}}),
            json!({"type":"text","text":"last"}),
        ],
    });
    if let Node::File { role, .. } = &mut document {
        *role = OrdinaryRole::Assistant;
    }
    let mut canonical = decode::gemini::decode_response(&response(vec![])).unwrap();
    canonical.output = vec![document.clone(), Node::assistant_text("after")];
    let wire = encode::gemini::encode_response_checked(&canonical, "gemini-test").unwrap();
    let mut encoder = stream_encode::gemini::GeminiStreamEncoder::new("gemini-test");
    let mut frames = encoder
        .push_event(UrpStreamEvent::NodeDone {
            node_index: 0,
            node: document,
            usage: None,
            extra_body: Default::default(),
        })
        .unwrap();
    frames.extend(
        encoder
            .push_event(UrpStreamEvent::ResponseDone {
                output: canonical.output,
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                extra_body: Default::default(),
            })
            .unwrap(),
    );
    let parts: Vec<_> = frames
        .iter()
        .filter_map(|frame| frame["candidates"][0]["content"]["parts"].as_array())
        .flatten()
        .cloned()
        .collect();
    assert_eq!(json!(parts), wire["candidates"][0]["content"]["parts"]);
    assert_eq!(parts[0]["text"], "first");
    assert_eq!(parts[1]["inlineData"]["mimeType"], "image/png");
    assert_eq!(parts[2]["text"], "last");
    assert_eq!(parts[3]["text"], "after");
    stream_decode(&frames).await.unwrap();
}

fn handler_request() -> crate::handlers::UrpRequest {
    crate::handlers::UrpRequest {
        audio_output_format: None,
        model: "gemini-test".into(),
        max_multiplier: None,
        server_tool_usage_classes: Vec::new(),
        messages_custom_tool_names: Default::default(),
        affinity_explicit: None,
        affinity_prefix_hash: String::new(),
    }
}

async fn stream_decode(frames: &[Value]) -> Result<Vec<UrpStreamEvent>, crate::error::AppError> {
    let body = frames
        .iter()
        .map(|frame| format!("data: {frame}\n\n"))
        .collect::<String>();
    let upstream = reqwest::Response::from(axum::http::Response::new(body));
    let (tx, mut rx) = tokio::sync::mpsc::channel(128);
    super::stream_decode::gemini::stream_gemini_to_urp_events(
        &handler_request(),
        upstream,
        tx,
        None,
        None,
        1000,
    )
    .await?;
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    Ok(events)
}

fn terminal(events: &[UrpStreamEvent]) -> UrpResponse {
    let (id, model) = events
        .iter()
        .find_map(|event| match event {
            UrpStreamEvent::ResponseStart { id, model, .. } => Some((id.clone(), model.clone())),
            _ => None,
        })
        .unwrap();
    events
        .iter()
        .find_map(|event| match event {
            UrpStreamEvent::ResponseDone {
                finish_reason,
                usage,
                output,
                extra_body,
            } => Some(UrpResponse {
                id: id.clone(),
                model: model.clone(),
                created_at: None,
                output: output.clone(),
                finish_reason: *finish_reason,
                usage: usage.clone(),
                extra_body: extra_body.clone(),
            }),
            _ => None,
        })
        .unwrap()
}

#[test]
fn gemini_usage_roundtrip_does_not_double_count_thoughts_or_tool_prompt() {
    let mut wire = response(vec![json!({"text":"answer"})]);
    wire["usageMetadata"] = json!({"promptTokenCount":10,"candidatesTokenCount":7,
        "toolUsePromptTokenCount":3,"thoughtsTokenCount":5,"cachedContentTokenCount":4});
    let canonical = decode::gemini::decode_response(&wire).unwrap();
    let usage = canonical.usage.as_ref().unwrap();
    assert_eq!((usage.input_tokens, usage.output_tokens), (13, 12));
    let encoded = encode::gemini::encode_response(&canonical, "gemini-test");
    assert_eq!(encoded["responseId"], "gemini-features");
    assert_eq!(encoded["usageMetadata"]["promptTokenCount"], 10);
    assert_eq!(encoded["usageMetadata"]["candidatesTokenCount"], 7);
    let decoded = decode::gemini::decode_response(&encoded).unwrap();
    assert_eq!(
        serde_json::to_value(decoded.usage).unwrap(),
        serde_json::to_value(canonical.usage).unwrap()
    );
}

#[tokio::test]
async fn gemini_stream_usage_survives_without_runtime_metrics_and_after_finish() {
    let mut first = response(vec![json!({"text":"answer"})]);
    first["usageMetadata"] = json!({"promptTokenCount":10,"candidatesTokenCount":1});
    let events = stream_decode(&[
        first,
        json!({"usageMetadata":{
        "promptTokenCount":10,"candidatesTokenCount":7,"thoughtsTokenCount":5,
        "toolUsePromptTokenCount":3}}),
    ])
    .await
    .unwrap();
    let response = terminal(&events);
    assert_eq!(response.usage.as_ref().unwrap().input_tokens, 13);
    assert_eq!(response.usage.as_ref().unwrap().output_tokens, 12);
    assert!(matches!(
        events[0],
        UrpStreamEvent::ResponseStart { usage: Some(_), .. }
    ));
}

#[test]
fn gemini_response_format_and_controls_obey_typed_deletion() {
    let mut request = decode::gemini::decode_request(&json!({"contents":[],"generationConfig":{
        "temperature":0.5,"topP":0.8,"maxOutputTokens":512,"stopSequences":["END"],
        "responseMimeType":"application/json","responseJsonSchema":{"type":"object"},
        "thinkingConfig":{"thinkingBudget":1024,"includeThoughts":true,"future":9},
        "topK":40}}))
    .unwrap();
    request.temperature = None;
    request.top_p = None;
    request.max_output_tokens = None;
    request.stop = None;
    request.reasoning = None;
    request.response_format = None;
    let wire = encode::gemini::encode_request(&request, "gemini-test");
    let cfg = wire["generationConfig"].as_object().unwrap();
    for key in [
        "temperature",
        "topP",
        "maxOutputTokens",
        "stopSequences",
        "responseMimeType",
        "responseJsonSchema",
        "responseSchema",
    ] {
        assert!(!cfg.contains_key(key), "restored {key}");
    }
    assert_eq!(cfg["topK"], 40);
    assert_eq!(cfg["thinkingConfig"], json!({"future":9}));
}

#[tokio::test]
async fn gemini_stream_rejects_missing_terminal_and_malformed_payload() {
    let wire = json!({"candidates":[{"content":{"parts":[{"text":"partial"}]}}]});
    assert!(stream_decode(&[wire]).await.is_err());
    assert!(
        stream_decode(&[json!({"error":{"message":"failed"}})])
            .await
            .is_err()
    );
}

fn assert_nonstream_part(part: Value) {
    let native = response(vec![part.clone()]);
    let canonical = decode::gemini::decode_response(&native).unwrap();
    let encoded = encode::gemini::encode_response(&canonical, "gemini-test");
    assert_eq!(encoded["candidates"][0]["content"]["parts"], json!([part]));
    let decoded = decode::gemini::decode_response(&encoded).unwrap();
    let replay = encode::gemini::encode_response(&decoded, "gemini-test");
    assert_eq!(replay, encoded);
}

async fn assert_stream_part(part: Value) {
    let events = stream_decode(&[response(vec![part.clone()])])
        .await
        .unwrap();
    let mut encoder = stream_encode::gemini::GeminiStreamEncoder::new("gemini-test");
    let frames: Vec<_> = events
        .into_iter()
        .flat_map(|event| encoder.push_event(event).unwrap())
        .collect();
    let parts: Vec<_> = frames
        .iter()
        .flat_map(|frame| {
            frame["candidates"][0]["content"]["parts"]
                .as_array()
                .into_iter()
                .flatten()
        })
        .cloned()
        .collect();
    assert_eq!(parts, vec![part.clone()]);
    let decoded = terminal(&stream_decode(&frames).await.unwrap());
    assert_eq!(
        encode::gemini::encode_response(&decoded, "gemini-test")["candidates"][0]["content"]["parts"],
        json!([part])
    );
}

macro_rules! part_feature {
    ($nonstream:ident, $stream:ident, $fixture:expr) => {
        #[test]
        fn $nonstream() {
            assert_nonstream_part($fixture);
        }
        #[tokio::test]
        async fn $stream() {
            assert_stream_part($fixture).await;
        }
    };
}

part_feature!(
    gemini_function_parent_metadata_nonstream,
    gemini_function_parent_metadata_stream,
    json!({"functionResponse":{"id":"call-1","name":"screen","response":{"ok":true},
        "parts":[{"inlineData":{"mimeType":"image/png","data":"aW1hZ2U="}}]},
        "thoughtSignature":"cGFyZW50", "partMetadata":{"vendor":"value"}})
);

part_feature!(
    gemini_thought_nonstream,
    gemini_thought_stream,
    json!({"text":"reason","thought":true,"thoughtSignature":"c2ln"})
);
part_feature!(
    gemini_signed_text_nonstream,
    gemini_signed_text_stream,
    json!({"text":"answer","thoughtSignature":"dGV4dA=="})
);
part_feature!(
    gemini_signature_only_nonstream,
    gemini_signature_only_stream,
    json!({"thoughtSignature":"b3BhcXVl"})
);
part_feature!(
    gemini_function_call_nonstream,
    gemini_function_call_stream,
    json!({"functionCall":{"id":"call-1","name":"weather","args":{"city":"Tokyo"},"future":7},"thoughtSignature":"ZnVuYw=="})
);
part_feature!(
    gemini_computer_action_nonstream,
    gemini_computer_action_stream,
    json!({"functionCall":{"id":"click-1","name":"click_at","args":{"x":140,"y":200}},"thoughtSignature":"Y29tcHV0ZXI="})
);
part_feature!(
    gemini_image_nonstream,
    gemini_image_stream,
    json!({"inlineData":{"mimeType":"image/png","data":"aW1hZ2U="},"thoughtSignature":"aW1nc2ln"})
);
part_feature!(
    gemini_audio_nonstream,
    gemini_audio_stream,
    json!({"inlineData":{"mimeType":"audio/wav","data":"YXVkaW8="},"thoughtSignature":"YXVkaW9zaWc="})
);
part_feature!(
    gemini_inline_file_nonstream,
    gemini_inline_file_stream,
    json!({"inlineData":{"mimeType":"application/pdf","data":"cGRm","displayName":"report"}})
);
part_feature!(
    gemini_file_uri_nonstream,
    gemini_file_uri_stream,
    json!({"fileData":{"mimeType":"application/pdf","fileUri":"https://example.com/report.pdf"},"thoughtSignature":"cGRmc2ln"})
);
part_feature!(
    gemini_audio_uri_nonstream,
    gemini_audio_uri_stream,
    json!({"fileData":{"mimeType":"audio/mp3","fileUri":"gs://bucket/audio.mp3"}})
);
part_feature!(
    gemini_image_uri_nonstream,
    gemini_image_uri_stream,
    json!({"fileData":{"mimeType":"image/webp","fileUri":"gs://bucket/image.webp"}})
);
part_feature!(
    gemini_video_nonstream,
    gemini_video_stream,
    json!({"fileData":{"mimeType":"video/mp4","fileUri":"gs://bucket/video.mp4"},"videoMetadata":{"startOffset":"1s","endOffset":"3s","fps":1}})
);
part_feature!(
    gemini_code_execution_nonstream,
    gemini_code_execution_stream,
    json!({"executableCode":{"id":"exec-1","language":"PYTHON","code":"print(2+2)"}})
);
part_feature!(
    gemini_code_result_nonstream,
    gemini_code_result_stream,
    json!({"codeExecutionResult":{"id":"exec-1","outcome":"OUTCOME_OK","output":"4\n"}})
);
part_feature!(
    gemini_server_tool_call_nonstream,
    gemini_server_tool_call_stream,
    json!({"toolCall":{"id":"search-1","toolType":"GOOGLE_SEARCH_WEB","toolName":"search","args":{"query":"weather"}}})
);
part_feature!(
    gemini_server_tool_result_nonstream,
    gemini_server_tool_result_stream,
    json!({"toolResponse":{"id":"search-1","toolType":"GOOGLE_SEARCH_WEB","response":{"result":"sunny"}}})
);
part_feature!(
    gemini_multimodal_function_result_nonstream,
    gemini_multimodal_function_result_stream,
    json!({"functionResponse":{"id":"call-1","name":"screen","response":{"output":{"$ref":"screenshot"}},
        "parts":[{"inlineData":{"mimeType":"image/png","data":"aW1hZ2U=","displayName":"screenshot"}}],"willContinue":false,"scheduling":"WHEN_IDLE"}})
);

#[test]
fn gemini_built_in_definitions_roundtrip_and_respect_config_deletion() {
    for stream in [false, true] {
        for (kind, config) in [
            (
                "computerUse",
                json!({"environment":"ENVIRONMENT_BROWSER","excludedPredefinedFunctions":["drag_and_drop"]}),
            ),
            (
                "googleSearch",
                json!({"timeRangeFilter":{"startTime":"2026-01-01T00:00:00Z"}}),
            ),
            (
                "googleSearchRetrieval",
                json!({"dynamicRetrievalConfig":{"mode":"MODE_DYNAMIC","dynamicThreshold":0.7}}),
            ),
            ("codeExecution", json!({})),
            ("googleMaps", json!({"enableWidget":true})),
            ("urlContext", json!({})),
            (
                "fileSearch",
                json!({"fileSearchStoreNames":["fileSearchStores/test"]}),
            ),
            (
                "mcpServers",
                json!([{"name":"docs","streamableHttpTransport":{"url":"https://example.com/mcp"}}]),
            ),
        ] {
            let native = json!({"contents":[],"stream":stream,"tools":[{kind:config}]});
            let mut canonical = decode::gemini::decode_request(&native).unwrap();
            let tool = &canonical.tools.as_ref().unwrap()[0];
            assert_eq!(tool.origin_protocol, Some(ProviderProtocol::Gemini));
            assert_eq!(tool.config.as_ref(), Some(&config));
            assert!(tool.extra_body.is_empty());
            assert_eq!(
                encode::gemini::encode_request(&canonical, "gemini-test")["tools"],
                native["tools"]
            );
            canonical.tools.as_mut().unwrap()[0].config = None;
            assert!(
                encode::gemini::encode_request(&canonical, "gemini-test")
                    .get("tools")
                    .is_none()
            );
        }
    }
}

#[test]
fn gemini_function_schema_choice_and_unknown_config_roundtrip() {
    for stream in [false, true] {
        for mode in ["ANY", "VALIDATED", "AUTO", "NONE"] {
            let native = json!({"contents":[],"stream":stream,"tools":[{"functionDeclarations":[
                {"name":"first","parametersJsonSchema":{"type":"object","properties":{"x":{"type":"number"}}},"responseJsonSchema":{"type":"number"},"behavior":"NON_BLOCKING"},
                {"name":"second","parameters":{"type":"OBJECT"}}]}],
                "toolConfig":{"functionCallingConfig":{"mode":mode,"allowedFunctionNames":["first","second"],"future":true},"retrievalConfig":{"latLng":{"latitude":1,"longitude":2}}}});
            let mut canonical = decode::gemini::decode_request(&native).unwrap();
            let encoded = encode::gemini::encode_request(&canonical, "gemini-test");
            assert_eq!(encoded["tools"], native["tools"]);
            assert_eq!(encoded["toolConfig"], native["toolConfig"]);
            canonical.tools.as_mut().unwrap()[0]
                .function
                .as_mut()
                .unwrap()
                .parameters = None;
            canonical.tool_choice = None;
            let encoded = encode::gemini::encode_request(&canonical, "gemini-test");
            assert!(
                encoded["tools"][0]["functionDeclarations"][0]
                    .get("parametersJsonSchema")
                    .is_none()
            );
            assert_eq!(
                encoded["toolConfig"]["functionCallingConfig"],
                json!({"future":true})
            );
        }
    }
}

#[test]
fn gemini_signed_nodes_have_no_shadow_payload_and_obey_mutation() {
    let mut canonical = decode::gemini::decode_response(&response(vec![
        json!({"functionCall":{"id":"call-1","name":"click_at","args":{"x":1}},"thoughtSignature":"old"}),
        json!({"inlineData":{"mimeType":"image/png","data":"b2xk"},"thoughtSignature":"old"}),
        json!({"text":"old","thoughtSignature":"old"}),
    ])).unwrap();
    assert_eq!(canonical.output.len(), 3);
    let Node::ToolCall {
        signature,
        arguments,
        extra_body,
        ..
    } = &mut canonical.output[0]
    else {
        panic!()
    };
    assert!(extra_body.is_empty());
    *signature = None;
    *arguments = "{\"x\":2}".into();
    let Node::Image {
        metadata,
        source,
        extra_body,
        ..
    } = &mut canonical.output[1]
    else {
        panic!()
    };
    assert!(extra_body.is_empty());
    metadata.signature = None;
    *source = ImageSource::Base64 {
        media_type: "image/jpeg".into(),
        data: "bmV3".into(),
    };
    let Node::Text {
        signature,
        content,
        extra_body,
        ..
    } = &mut canonical.output[2]
    else {
        panic!()
    };
    assert!(extra_body.is_empty());
    *signature = None;
    *content = "new".into();
    let encoded = encode::gemini::encode_response(&canonical, "gemini-test");
    let parts = encoded["candidates"][0]["content"]["parts"]
        .as_array()
        .unwrap();
    assert!(
        parts
            .iter()
            .all(|part| part.get("thoughtSignature").is_none())
    );
    assert_eq!(parts[0]["functionCall"]["args"], json!({"x":2}));
    assert_eq!(
        parts[1]["inlineData"],
        json!({"mimeType":"image/jpeg","data":"bmV3"})
    );
    assert_eq!(parts[2]["text"], "new");
}

#[test]
fn gemini_synthetic_call_result_matching_preserves_parallel_name_correlation() {
    let native = json!({"contents":[{"role":"model","parts":[
        {"functionCall":{"name":"weather","args":{"city":"Tokyo"}}},
        {"functionCall":{"name":"weather","args":{"city":"Osaka"}}}]},
        {"role":"user","parts":[{"functionResponse":{"name":"weather","response":{"temp":25}}},
        {"functionResponse":{"name":"weather","response":{"temp":28}}}]}]});
    let canonical = decode::gemini::decode_request(&native).unwrap();
    let calls: Vec<_> = canonical
        .input
        .iter()
        .filter_map(|node| match node {
            Node::ToolCall { call_id, .. } => Some(call_id),
            _ => None,
        })
        .collect();
    let results: Vec<_> = canonical
        .input
        .iter()
        .filter_map(|node| match node {
            Node::ToolResult { call_id, .. } => Some(call_id),
            _ => None,
        })
        .collect();
    assert_eq!(calls, results);
    assert_ne!(calls[0], calls[1]);
    assert_eq!(
        encode::gemini::encode_request(&canonical, "gemini-test")["contents"],
        native["contents"]
    );
}

#[tokio::test]
async fn gemini_grounding_citations_safety_metadata_nonstream_and_stream() {
    let mut native = response(vec![json!({"text":"source"})]);
    native["candidates"][0]["citationMetadata"] = json!({"citationSources":[{"startIndex":0,"endIndex":6,"uri":"https://example.com","license":"CC-BY"}],"future":true});
    native["candidates"][0]["groundingMetadata"] = json!({"groundingChunks":[{"web":{"uri":"https://example.com","title":"Source"}}],"webSearchQueries":["query"]});
    native["candidates"][0]["urlContextMetadata"] = json!({"urlMetadata":[{"retrievedUrl":"https://example.com","urlRetrievalStatus":"URL_RETRIEVAL_STATUS_SUCCESS"}]});
    native["candidates"][0]["safetyRatings"] =
        json!([{"category":"HARM_CATEGORY_HATE_SPEECH","probability":"NEGLIGIBLE"}]);
    for mut canonical in [
        decode::gemini::decode_response(&native).unwrap(),
        terminal(&stream_decode(&[native.clone()]).await.unwrap()),
    ] {
        let Node::Text { citations, .. } = &mut canonical.output[0] else {
            panic!()
        };
        assert_eq!(citations.len(), 1);
        let encoded = encode::gemini::encode_response(&canonical, "gemini-test");
        for key in [
            "citationMetadata",
            "groundingMetadata",
            "urlContextMetadata",
            "safetyRatings",
        ] {
            assert_eq!(encoded["candidates"][0][key], native["candidates"][0][key]);
        }
        let Node::Text { citations, .. } = &mut canonical.output[0] else {
            panic!()
        };
        citations.clear();
        assert!(encode::gemini::encode_response(&canonical,"gemini-test")["candidates"][0]["citationMetadata"].get("citationSources").is_none());
    }
}

#[tokio::test]
async fn gemini_prompt_block_and_finish_without_content_roundtrip() {
    let blocked = json!({"responseId":"blocked","promptFeedback":{"blockReason":"SAFETY","safetyRatings":[]}});
    for canonical in [
        decode::gemini::decode_response(&blocked).unwrap(),
        terminal(&stream_decode(&[blocked]).await.unwrap()),
    ] {
        assert_eq!(canonical.finish_reason, Some(FinishReason::ContentFilter));
        assert!(matches!(canonical.output[0], Node::Refusal { .. }));
        assert_eq!(
            encode::gemini::encode_response(&canonical, "gemini-test")["promptFeedback"]["blockReason"],
            "SAFETY"
        );
    }
    for reason in [
        "MAX_TOKENS",
        "SAFETY",
        "RECITATION",
        "BLOCKLIST",
        "SPII",
        "IMAGE_SAFETY",
        "STOP",
    ] {
        let native = json!({"candidates":[{"finishReason":reason}]});
        let nonstream = decode::gemini::decode_response(&native).unwrap();
        let streamed = terminal(&stream_decode(&[native]).await.unwrap());
        assert_eq!(nonstream.finish_reason, streamed.finish_reason);
    }
}

#[tokio::test]
async fn gemini_stream_terminal_reconciliation_emits_missing_parts_once() {
    let mut encoder = stream_encode::gemini::GeminiStreamEncoder::new("gemini-test");
    let first = Node::assistant_text("a");
    let mut frames = encoder
        .push_event(UrpStreamEvent::NodeDone {
            node_index: 0,
            node: first,
            usage: None,
            extra_body: HashMap::new(),
        })
        .unwrap();
    frames.extend(
        encoder
            .push_event(UrpStreamEvent::ResponseDone {
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                output: vec![Node::assistant_text("ab"), Node::assistant_text("c")],
                extra_body: HashMap::new(),
            })
            .unwrap(),
    );
    let decoded = terminal(&stream_decode(&frames).await.unwrap());
    let text: String = decoded
        .output
        .iter()
        .filter_map(|node| match node {
            Node::Text { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "abc");
    assert_eq!(
        frames
            .iter()
            .filter(|frame| frame["candidates"][0].get("finishReason").is_some())
            .count(),
        1
    );
}

#[tokio::test]
async fn gemini_stream_orders_out_of_order_completions_and_terminal_repairs() {
    for missing_first_completion in [false, true] {
        let mut encoder = stream_encode::gemini::GeminiStreamEncoder::new("gemini-test");
        let first = Node::assistant_text("first");
        let second = Node::assistant_text("second");
        for node_index in 0..2 {
            assert!(
                encoder
                    .push_event(UrpStreamEvent::NodeStart {
                        node_index,
                        header: NodeHeader::Text {
                            id: None,
                            role: OrdinaryRole::Assistant,
                            phase: None,
                            signature: None,
                            citations: Vec::new()
                        },
                        extra_body: HashMap::new(),
                    })
                    .unwrap()
                    .is_empty()
            );
        }
        assert!(
            encoder
                .push_event(UrpStreamEvent::NodeDone {
                    node_index: 1,
                    node: second.clone(),
                    usage: None,
                    extra_body: HashMap::new(),
                })
                .unwrap()
                .is_empty()
        );
        let mut frames = Vec::new();
        if !missing_first_completion {
            frames.extend(
                encoder
                    .push_event(UrpStreamEvent::NodeDone {
                        node_index: 0,
                        node: first.clone(),
                        usage: None,
                        extra_body: HashMap::new(),
                    })
                    .unwrap(),
            );
        }
        frames.extend(
            encoder
                .push_event(UrpStreamEvent::ResponseDone {
                    output: vec![first, second],
                    finish_reason: Some(FinishReason::Stop),
                    usage: None,
                    extra_body: HashMap::new(),
                })
                .unwrap(),
        );
        let parts: Vec<_> = frames
            .iter()
            .filter_map(|frame| frame["candidates"][0]["content"]["parts"].as_array())
            .flatten()
            .cloned()
            .collect();
        assert_eq!(
            parts,
            vec![json!({"text":"first"}), json!({"text":"second"})]
        );
        let decoded = terminal(&stream_decode(&frames).await.unwrap());
        let text: String = decoded
            .output
            .iter()
            .filter_map(|node| match node {
                Node::Text { content, .. } => Some(content.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "firstsecond");
    }
}

#[test]
fn gemini_stream_does_not_mark_unrepresentable_nodes_as_emitted() {
    for output in [vec![], vec![Node::assistant_text("replacement")]] {
        let mut encoder = stream_encode::gemini::GeminiStreamEncoder::new("gemini-test");
        assert!(
            encoder
                .push_event(UrpStreamEvent::NodeDone {
                    node_index: 0,
                    node: Node::NextDownstreamEnvelopeExtra {
                        extra_body: HashMap::new()
                    },
                    usage: None,
                    extra_body: HashMap::new(),
                })
                .unwrap()
                .is_empty()
        );
        let expected_parts = output.len();
        let frames = encoder
            .push_event(UrpStreamEvent::ResponseDone {
                output,
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                extra_body: HashMap::new(),
            })
            .unwrap();
        assert_eq!(frames.len(), expected_parts + 1);
    }
    let mut encoder = stream_encode::gemini::GeminiStreamEncoder::new("gemini-test");
    for (node_index, node) in [
        (
            0,
            Node::NextDownstreamEnvelopeExtra {
                extra_body: HashMap::new(),
            },
        ),
        (1, Node::assistant_text("later")),
    ] {
        encoder
            .push_event(UrpStreamEvent::NodeDone {
                node_index,
                node,
                usage: None,
                extra_body: HashMap::new(),
            })
            .unwrap();
    }
    assert!(
        encoder
            .push_event(UrpStreamEvent::ResponseDone {
                output: vec![
                    Node::assistant_text("inserted"),
                    Node::assistant_text("later")
                ],
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                extra_body: HashMap::new(),
            })
            .is_err()
    );
}

#[test]
fn gemini_stream_rejects_prefix_extension_after_later_part_emission() {
    let mut encoder = stream_encode::gemini::GeminiStreamEncoder::new("gemini-test");
    for (node_index, text) in [(0, "a"), (1, "c")] {
        encoder
            .push_event(UrpStreamEvent::NodeDone {
                node_index,
                node: Node::assistant_text(text),
                usage: None,
                extra_body: HashMap::new(),
            })
            .unwrap();
    }
    assert!(
        encoder
            .push_event(UrpStreamEvent::ResponseDone {
                output: vec![Node::assistant_text("ab"), Node::assistant_text("c")],
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                extra_body: HashMap::new(),
            })
            .is_err()
    );
}

fn mutate_provider_identity(response: &mut UrpResponse, delete: bool) {
    for node in &mut response.output {
        if let Node::ProviderItem { id, item_type, .. } = node {
            *id = (!delete).then(|| "new".into());
            *item_type = if delete {
                String::new()
            } else {
                "updated".into()
            };
        } else {
            panic!("expected opaque part")
        }
    }
}

fn opaque_parts() -> Vec<Value> {
    vec![
        json!({"id":"old","type":"vendor","payload":{"value":7}}),
        json!({"executableCode":{"language":"PYTHON","code":"print(1)"}}),
        json!({"toolCall":{"id":"nested","toolType":"GOOGLE_SEARCH","args":{"q":"test"}}}),
    ]
}

fn assert_provider_identity(parts: &Value, delete: bool) {
    let expected = if delete {
        json!({"payload":{"value":7}})
    } else {
        json!({"id":"new","type":"updated","payload":{"value":7}})
    };
    assert_eq!(parts[0], expected);
    assert_eq!(parts[1], opaque_parts()[1]);
    assert_eq!(parts[2], opaque_parts()[2]);
}

#[test]
fn gemini_opaque_identity_obeys_typed_mutation_and_deletion_nonstream() {
    for delete in [false, true] {
        let mut canonical = decode::gemini::decode_response(&response(opaque_parts())).unwrap();
        mutate_provider_identity(&mut canonical, delete);
        let wire = encode::gemini::encode_response(&canonical, "gemini-test");
        assert_provider_identity(&wire["candidates"][0]["content"]["parts"], delete);
        let decoded = decode::gemini::decode_response(&wire).unwrap();
        assert_eq!(
            encode::gemini::encode_response(&decoded, "gemini-test"),
            wire
        );
    }
}

#[tokio::test]
async fn gemini_opaque_identity_obeys_typed_mutation_and_deletion_stream() {
    for delete in [false, true] {
        let mut canonical = terminal(&stream_decode(&[response(opaque_parts())]).await.unwrap());
        mutate_provider_identity(&mut canonical, delete);
        let mut encoder = stream_encode::gemini::GeminiStreamEncoder::new("gemini-test");
        let mut frames = Vec::new();
        for (index, node) in canonical.output.iter().cloned().enumerate() {
            frames.extend(
                encoder
                    .push_event(UrpStreamEvent::NodeDone {
                        node_index: index as u32,
                        node,
                        usage: None,
                        extra_body: HashMap::new(),
                    })
                    .unwrap(),
            );
        }
        frames.extend(
            encoder
                .push_event(UrpStreamEvent::ResponseDone {
                    output: canonical.output,
                    finish_reason: Some(FinishReason::Stop),
                    usage: None,
                    extra_body: HashMap::new(),
                })
                .unwrap(),
        );
        let decoded = terminal(&stream_decode(&frames).await.unwrap());
        let wire = encode::gemini::encode_response(&decoded, "gemini-test");
        assert_provider_identity(&wire["candidates"][0]["content"]["parts"], delete);
    }
}

#[test]
fn gemini_native_schema_types_map_to_canonical_json_schema() {
    let canonical = decode::gemini::decode_request(&json!({"contents":[],
        "tools":[{"functionDeclarations":[{"name":"run","parameters":{"type":"OBJECT","properties":{"value":{"type":"INTEGER"}}}}]}],
        "generationConfig":{"responseMimeType":"application/json","responseSchema":{"type":"ARRAY","items":{"type":"STRING"}}}})).unwrap();
    assert_eq!(
        canonical.tools.as_ref().unwrap()[0]
            .function
            .as_ref()
            .unwrap()
            .parameters
            .as_ref()
            .unwrap()["type"],
        "object"
    );
    let ResponseFormat::JsonSchema { json_schema } = canonical.response_format.as_ref().unwrap()
    else {
        panic!()
    };
    assert_eq!(
        json_schema.schema,
        json!({"type":"array","items":{"type":"string"}})
    );
    let encoded = encode::gemini::encode_request(&canonical, "gemini-test");
    assert_eq!(
        encoded["generationConfig"]["responseJsonSchema"],
        json_schema.schema
    );
    assert_eq!(
        encoded["tools"][0]["functionDeclarations"][0]["parameters"]["type"],
        "OBJECT"
    );
    let enum_request = json!({"contents":[],"generationConfig":{"responseMimeType":"text/x.enum","responseSchema":{"type":"STRING","enum":["a","b"]}}});
    let canonical = decode::gemini::decode_request(&enum_request).unwrap();
    assert_eq!(
        encode::gemini::encode_request(&canonical, "gemini-test")["generationConfig"],
        enum_request["generationConfig"]
    );
}

#[tokio::test]
async fn gemini_prompt_refusal_stream_roundtrip_does_not_duplicate_text() {
    let blocked = json!({"responseId":"blocked","promptFeedback":{"blockReason":"SAFETY"}});
    let events = stream_decode(&[blocked.clone()]).await.unwrap();
    let canonical = terminal(&events);
    let encoded = encode::gemini::encode_response(&canonical, "gemini-test");
    assert_eq!(
        decode::gemini::decode_response(&encoded)
            .unwrap()
            .output
            .len(),
        1
    );
    let mut encoder = stream_encode::gemini::GeminiStreamEncoder::new("gemini-test");
    let frames: Vec<_> = events
        .into_iter()
        .flat_map(|event| encoder.push_event(event).unwrap())
        .collect();
    assert_eq!(
        terminal(&stream_decode(&frames).await.unwrap())
            .output
            .len(),
        1
    );
}

#[tokio::test]
async fn gemini_usage_modality_details_and_overflow_both_paths() {
    let mut native = response(vec![]);
    native["usageMetadata"] = json!({"promptTokenCount":10,"candidatesTokenCount":8,
        "promptTokensDetails":[{"modality":"TEXT","tokenCount":3},{"modality":"AUDIO","tokenCount":7}],
        "cacheTokensDetails":[{"modality":"IMAGE","tokenCount":2}],
        "candidatesTokensDetails":[{"modality":"TEXT","tokenCount":8}]});
    for canonical in [
        decode::gemini::decode_response(&native).unwrap(),
        terminal(&stream_decode(&[native.clone()]).await.unwrap()),
    ] {
        let usage = canonical.usage.as_ref().unwrap();
        assert_eq!(
            usage
                .input_details
                .as_ref()
                .unwrap()
                .modality_breakdown
                .as_ref()
                .unwrap()
                .audio_tokens,
            Some(7)
        );
        assert!(!usage.extra_body.contains_key("promptTokensDetails"));
        let encoded = encode::gemini::encode_response(&canonical, "gemini-test");
        for key in [
            "promptTokensDetails",
            "cacheTokensDetails",
            "candidatesTokensDetails",
        ] {
            assert_eq!(encoded["usageMetadata"][key], native["usageMetadata"][key]);
        }
    }
    native["usageMetadata"] = json!({"promptTokenCount":u64::MAX,"toolUsePromptTokenCount":1});
    assert!(decode::gemini::decode_response(&native).is_err());
    assert!(stream_decode(&[native]).await.is_err());
}

#[tokio::test]
async fn gemini_late_citations_reach_terminal_stream_output() {
    let mut initial = response(vec![json!({"text":"answer"})]);
    initial["candidates"][0]
        .as_object_mut()
        .unwrap()
        .remove("finishReason");
    let terminal_frame = json!({"candidates":[{"finishReason":"STOP","citationMetadata":{"citationSources":[{"uri":"https://example.com"}]}}]});
    let events = stream_decode(&[initial, terminal_frame]).await.unwrap();
    let Node::Text { citations, .. } = &terminal(&events).output[0] else {
        panic!()
    };
    assert_eq!(citations.len(), 1);
    let mut encoder = stream_encode::gemini::GeminiStreamEncoder::new("gemini-test");
    let frames: Vec<_> = events
        .into_iter()
        .flat_map(|event| encoder.push_event(event).unwrap())
        .collect();
    let decoded = terminal(&stream_decode(&frames).await.unwrap());
    let Node::Text { citations, .. } = &decoded.output[0] else {
        panic!()
    };
    assert_eq!(citations.len(), 1);
}

#[tokio::test]
async fn gemini_async_sse_encoder_writes_native_frames_and_requires_terminal() {
    use axum::response::IntoResponse;
    use futures_util::StreamExt;
    let events = stream_decode(&[response(vec![
        json!({"functionCall":{"id":"c","name":"run","args":{}}}),
    ])])
    .await
    .unwrap();
    let (tx, rx) = tokio::sync::mpsc::channel(128);
    for event in events {
        tx.send(event).await.unwrap();
    }
    drop(tx);
    let (wire_tx, wire_rx) = tokio::sync::mpsc::channel(128);
    stream_encode::gemini::encode_urp_stream_as_gemini(rx, wire_tx, "gemini-test")
        .await
        .unwrap();
    let response = axum::response::sse::Sse::new(
        tokio_stream::wrappers::ReceiverStream::new(wire_rx).map(Ok::<_, std::convert::Infallible>),
    )
    .into_response();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = std::str::from_utf8(&body).unwrap();
    assert!(!text.contains("[DONE]"));
    let frames: Vec<Value> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|data| serde_json::from_str(data).unwrap())
        .collect();
    assert_eq!(
        terminal(&stream_decode(&frames).await.unwrap()).finish_reason,
        Some(FinishReason::ToolCalls)
    );
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    drop(tx);
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    assert!(
        stream_encode::gemini::encode_urp_stream_as_gemini(rx, tx, "gemini-test")
            .await
            .is_err()
    );
}

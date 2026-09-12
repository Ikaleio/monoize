use super::{
    AudioSource, FileSource, ImageSource, MediaResource, Node, OrdinaryRole, ProviderProtocol,
    ToolResultContent, UrpRequest, decode, encode, media,
};
use serde_json::{Value, json};

const PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aT54AAAAASUVORK5CYII=";
const PDF: &str = "JVBERi0xLjcK";
const TARGETS: [ProviderProtocol; 4] = [
    ProviderProtocol::ChatCompletion,
    ProviderProtocol::Responses,
    ProviderProtocol::Messages,
    ProviderProtocol::Gemini,
];

fn decode_request(protocol: ProviderProtocol, native: &Value) -> UrpRequest {
    match protocol {
        ProviderProtocol::ChatCompletion => decode::openai_chat::decode_request(native),
        ProviderProtocol::Responses => decode::openai_responses::decode_request(native),
        ProviderProtocol::Messages => decode::anthropic::decode_request(native),
        ProviderProtocol::Gemini => decode::gemini::decode_request(native),
        _ => unreachable!(),
    }
    .unwrap()
}

fn encode_request(protocol: ProviderProtocol, request: &UrpRequest) -> Result<Value, String> {
    match protocol {
        ProviderProtocol::ChatCompletion => {
            encode::openai_chat::encode_request_checked(request, "media-test")
        }
        ProviderProtocol::Responses => {
            encode::openai_responses::encode_request_checked(request, "media-test")
        }
        ProviderProtocol::Messages => {
            encode::anthropic::encode_request_checked(request, "media-test")
        }
        ProviderProtocol::Gemini => encode::gemini::encode_request_checked(request, "media-test"),
        _ => unreachable!(),
    }
}

fn chat_request(blocks: Vec<Value>, stream: bool) -> UrpRequest {
    decode_request(
        ProviderProtocol::ChatCompletion,
        &json!({"model":"media-test","stream":stream,
        "messages":[{"role":"user","content":blocks}]}),
    )
}

fn document_request(source: Value, stream: bool) -> UrpRequest {
    decode_request(
        ProviderProtocol::Messages,
        &json!({"model":"media-test","max_tokens":128,"stream":stream,
        "messages":[{"role":"user","content":[{"type":"document","title":"Document title",
            "context":"Document context","citations":{"enabled":true},"source":source}]}]}),
    )
}

fn media_block(protocol: ProviderProtocol, wire: &Value, index: usize) -> &Value {
    match protocol {
        ProviderProtocol::ChatCompletion | ProviderProtocol::Messages => {
            &wire["messages"][0]["content"][index]
        }
        ProviderProtocol::Responses => &wire["input"][0]["content"][index],
        ProviderProtocol::Gemini => &wire["contents"][0]["parts"][index],
        _ => unreachable!(),
    }
}

fn visible_text(nodes: &[Node]) -> String {
    nodes
        .iter()
        .flat_map(|node| match node {
            Node::Text { content, .. } => vec![content.clone()],
            Node::File {
                source, metadata, ..
            } => {
                let mut text = [
                    metadata.document_title.clone(),
                    metadata.document_context.clone(),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
                match source {
                    FileSource::Text { text: body } => text.push(body.clone()),
                    FileSource::Content { content } => {
                        text.extend(content.iter().filter_map(|part| {
                            part.get("text").and_then(Value::as_str).map(str::to_string)
                        }))
                    }
                    _ => {}
                }
                text
            }
            _ => vec![],
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn media_data_url_owns_mime_and_image_detail_across_openai_schemas() {
    for stream in [false, true] {
        for source_protocol in [
            ProviderProtocol::ChatCompletion,
            ProviderProtocol::Responses,
        ] {
            let url = format!("data:image/png;base64,{PNG}");
            let mut request = match source_protocol {
                ProviderProtocol::ChatCompletion => chat_request(
                    vec![json!({"type":"image_url","image_url":{"url":url,"detail":"high"}})],
                    stream,
                ),
                ProviderProtocol::Responses => decode_request(
                    source_protocol,
                    &json!({"model":"media-test","stream":stream,
                    "input":[{"role":"user","content":[{"type":"input_image","image_url":url,"detail":"high"}]}]}),
                ),
                _ => unreachable!(),
            };
            let Node::Image {
                source,
                metadata,
                extra_body,
                ..
            } = &request.input[0]
            else {
                panic!("typed image required")
            };
            assert_eq!(
                source,
                &ImageSource::Base64 {
                    media_type: "image/png".into(),
                    data: PNG.into()
                }
            );
            assert_eq!(metadata.detail.as_deref(), Some("high"));
            assert!(!extra_body.contains_key("detail"));
            let canonical = serde_json::to_value(&request).unwrap();
            for target in TARGETS {
                let wire = encode_request(target, &request).unwrap();
                let block = media_block(target, &wire, 0);
                match target {
                    ProviderProtocol::ChatCompletion => {
                        assert_eq!(block["image_url"]["url"], url);
                        assert_eq!(block["image_url"]["detail"], "high");
                    }
                    ProviderProtocol::Responses => {
                        assert_eq!(block["image_url"], url);
                        assert_eq!(block["detail"], "high");
                    }
                    ProviderProtocol::Messages => assert_eq!(
                        block["source"],
                        json!({"type":"base64","media_type":"image/png","data":PNG})
                    ),
                    ProviderProtocol::Gemini => assert_eq!(
                        block["inlineData"],
                        json!({"mimeType":"image/png","data":PNG})
                    ),
                    _ => unreachable!(),
                }
            }
            assert_eq!(serde_json::to_value(&request).unwrap(), canonical);
            let Node::Image {
                metadata,
                extra_body,
                ..
            } = &mut request.input[0]
            else {
                unreachable!()
            };
            metadata.detail = None;
            extra_body.insert("detail".into(), json!("stale"));
            for target in [
                ProviderProtocol::ChatCompletion,
                ProviderProtocol::Responses,
            ] {
                let wire = encode_request(target, &request).unwrap();
                let block = media_block(target, &wire, 0);
                assert!(block.get("detail").is_none());
                assert!(block["image_url"].get("detail").is_none());
            }
        }
    }
}

#[test]
fn media_image_mime_parameters_do_not_enter_fixed_native_mime_fields() {
    for stream in [false, true] {
        let request = chat_request(
            vec![json!({"type":"image_url", "image_url":{
                "url":format!("data:image/png;charset=utf-8;base64,{PNG}")
            }})],
            stream,
        );
        let before = serde_json::to_value(&request).unwrap();
        for target in TARGETS {
            let wire = encode_request(target, &request).unwrap();
            assert!(!wire.to_string().contains("charset"));
            let restored = decode_request(target, &wire);
            assert!(matches!(&restored.input[0], Node::Image {
                source:ImageSource::Base64 { media_type, data }, ..
            } if media_type == "image/png" && data == PNG));
        }
        assert_eq!(serde_json::to_value(&request).unwrap(), before);
    }
}

#[test]
fn media_file_data_url_has_single_filename_owner_and_raw_bytes() {
    for stream in [false, true] {
        let mut request = chat_request(
            vec![json!({"type":"file","file":{
            "filename":"document.pdf","file_data":format!("data:application/pdf;base64,{PDF}")}})],
            stream,
        );
        let Node::File {
            source,
            metadata,
            extra_body,
            ..
        } = &request.input[0]
        else {
            panic!("typed file required")
        };
        assert_eq!(
            source,
            &FileSource::Base64 {
                media_type: "application/pdf".into(),
                data: PDF.into()
            }
        );
        assert_eq!(metadata.filename.as_deref(), Some("document.pdf"));
        assert!(
            !serde_json::to_value(source)
                .unwrap()
                .as_object()
                .unwrap()
                .contains_key("filename")
        );
        assert!(!extra_body.contains_key("filename"));
        for target in [
            ProviderProtocol::ChatCompletion,
            ProviderProtocol::Responses,
        ] {
            let wire = encode_request(target, &request).unwrap();
            let block = media_block(target, &wire, 0);
            let file = if target == ProviderProtocol::ChatCompletion {
                &block["file"]
            } else {
                block
            };
            assert_eq!(
                file["file_data"],
                format!("data:application/pdf;base64,{PDF}")
            );
            assert_eq!(file["filename"], "document.pdf");
        }
        let Node::File {
            metadata,
            extra_body,
            ..
        } = &mut request.input[0]
        else {
            unreachable!()
        };
        metadata.filename = None;
        extra_body.insert("filename".into(), json!("stale.pdf"));
        for target in [
            ProviderProtocol::ChatCompletion,
            ProviderProtocol::Responses,
        ] {
            let wire = encode_request(target, &request).unwrap();
            assert!(!wire.to_string().contains("stale.pdf"));
            assert!(!wire.to_string().contains("document.pdf"));
        }
    }
}

#[test]
fn media_raw_file_mime_uses_magic_then_filename_and_rejects_unknown() {
    for stream in [false, true] {
        for (data, filename, mime) in [
            (PDF, Some("misleading.txt"), "application/pdf"),
            (PDF, None, "application/pdf"),
            ("aGVsbG8=", Some("notes.txt"), "text/plain"),
        ] {
            let mut file = json!({"file_data":data});
            if let Some(filename) = filename {
                file["filename"] = json!(filename);
            }
            let request = chat_request(vec![json!({"type":"file","file":file})], stream);
            assert!(
                matches!(&request.input[0], Node::File { source:FileSource::Base64 { media_type, data:actual }, .. } if media_type == mime && actual == data)
            );
            for target in TARGETS {
                let wire = encode_request(target, &request).unwrap();
                let restored = decode_request(target, &wire);
                if target == ProviderProtocol::Messages && mime == "text/plain" {
                    assert!(
                        matches!(&restored.input[0], Node::File { source:FileSource::Text { text }, .. } if text == "hello")
                    );
                } else {
                    assert!(
                        matches!(&restored.input[0], Node::File { source:FileSource::Base64 { media_type, data:actual }, .. } if media_type == mime && actual == data),
                        "{target:?}: {restored:?}"
                    );
                }
            }
        }
        let request = chat_request(
            vec![
                json!({"type":"text","text":"Do not partially succeed"}),
                json!({"type":"file","file":{"file_data":"AAECAw=="}}),
            ],
            stream,
        );
        for target in TARGETS {
            assert!(encode_request(target, &request).is_err(), "{target:?}");
        }
    }
}

#[test]
fn media_text_and_compound_documents_keep_visible_metadata_and_prepare_is_idempotent() {
    let sources = [
        json!({"type":"text","media_type":"text/plain","data":"Document body"}),
        json!({"type":"content","content":"Document body"}),
        json!({"type":"content","content":[{"type":"text","text":"Document body"},
            {"type":"image","source":{"type":"url","url":format!("data:image/png;base64,{PNG}")}},
            {"type":"text","text":"After image"}]}),
    ];
    for stream in [false, true] {
        for source in &sources {
            let request = document_request(source.clone(), stream);
            let canonical = serde_json::to_value(&request).unwrap();
            let Node::File { metadata, .. } = &request.input[0] else {
                unreachable!()
            };
            assert_eq!(metadata.document_citations, Some(json!({"enabled":true})));
            for target in TARGETS {
                let prepared = media::prepare_request(&request, target).unwrap();
                let twice = media::prepare_request(&prepared, target).unwrap();
                assert_eq!(
                    serde_json::to_value(&prepared).unwrap(),
                    serde_json::to_value(&twice).unwrap(),
                    "{target:?}"
                );
                let wire = encode_request(target, &prepared).unwrap();
                let restored = decode_request(target, &wire);
                let text = visible_text(&restored.input);
                for expected in ["Document title", "Document context", "Document body"] {
                    assert_eq!(text.matches(expected).count(), 1, "{target:?}: {wire}");
                }
                assert!(
                    text.find("Document title").unwrap() < text.find("Document context").unwrap()
                );
                assert!(
                    text.find("Document context").unwrap() < text.find("Document body").unwrap()
                );
                if source["content"].is_array() {
                    assert!(
                        text.find("Document body").unwrap() < text.find("After image").unwrap()
                    );
                    assert!(wire.to_string().contains(PNG));
                }
            }
            assert_eq!(serde_json::to_value(&request).unwrap(), canonical);
        }
    }
}

#[test]
fn media_pdf_metadata_prefix_is_visible_once_across_preparation_and_encoding() {
    for stream in [false, true] {
        let request = document_request(
            json!({"type":"base64","media_type":"application/pdf","data":PDF}),
            stream,
        );
        for target in TARGETS {
            let prepared = media::prepare_request(&request, target).unwrap();
            assert_eq!(
                serde_json::to_value(&prepared).unwrap(),
                serde_json::to_value(media::prepare_request(&prepared, target).unwrap()).unwrap()
            );
            let wire = encode_request(target, &prepared).unwrap();
            let restored = decode_request(target, &wire);
            let text = visible_text(&restored.input);
            assert_eq!(
                text.matches("Document title").count(),
                1,
                "{target:?}: {wire}"
            );
            assert_eq!(
                text.matches("Document context").count(),
                1,
                "{target:?}: {wire}"
            );
            assert!(restored.input.iter().any(|node| matches!(node, Node::File { source:FileSource::Base64 { data, media_type }, .. } if data == PDF && media_type == "application/pdf")));
        }
    }
}

#[test]
fn media_file_id_provenance_is_typed_and_deletion_prevents_replay() {
    for stream in [false, true] {
        let mut request = chat_request(
            vec![json!({"type":"file","file":{"file_id":"file_openai"}})],
            stream,
        );
        let Node::File {
            metadata,
            extra_body,
            ..
        } = &request.input[0]
        else {
            unreachable!()
        };
        assert_eq!(
            metadata.resource.as_ref().unwrap().protocol,
            ProviderProtocol::Responses
        );
        assert!(extra_body.is_empty());
        for target in TARGETS {
            let encoded = encode_request(target, &request);
            if matches!(
                target,
                ProviderProtocol::ChatCompletion | ProviderProtocol::Responses
            ) {
                let wire = encoded.unwrap();
                assert!(wire.to_string().contains("file_openai"));
                assert!(!wire.to_string().contains("credential_scope"));
                assert!(!wire.to_string().contains("resource"));
            } else {
                assert!(encoded.is_err());
            }
        }
        let Node::File {
            metadata,
            extra_body,
            ..
        } = &mut request.input[0]
        else {
            unreachable!()
        };
        metadata.resource = None;
        extra_body.insert("_monoize_file_id_origin".into(), json!("openai"));
        assert!(media::resources(&request.input).is_err());
        for target in TARGETS {
            assert!(encode_request(target, &request).is_err());
        }
    }
}

#[test]
fn media_bound_resource_requires_matching_provider_channel_and_credential_scope() {
    let mut request = chat_request(
        vec![json!({"type":"file","file":{"file_id":"file_private"}})],
        false,
    );
    let scope = MediaResource {
        protocol: ProviderProtocol::Responses,
        provider_id: Some("provider-a".into()),
        channel_id: Some("channel-a".into()),
        credential_scope: Some("credential-a".into()),
    };
    let unbound = media::resources(&request.input).unwrap().remove(0);
    assert!(media::resource_matches_scope(&unbound, &scope));
    media::bind_resources(&mut request.input, &scope);
    let bound = media::resources(&request.input).unwrap().remove(0);
    assert_eq!(bound, scope);
    let mut sibling_protocol = scope.clone();
    sibling_protocol.protocol = ProviderProtocol::ChatCompletion;
    assert!(media::resource_matches_scope(&bound, &sibling_protocol));
    for mismatch in [
        MediaResource {
            protocol: ProviderProtocol::Messages,
            ..scope.clone()
        },
        MediaResource {
            provider_id: Some("provider-b".into()),
            ..scope.clone()
        },
        MediaResource {
            channel_id: Some("channel-b".into()),
            ..scope.clone()
        },
        MediaResource {
            credential_scope: Some("credential-b".into()),
            ..scope.clone()
        },
        MediaResource {
            provider_id: None,
            ..scope.clone()
        },
        MediaResource {
            channel_id: None,
            ..scope.clone()
        },
        MediaResource {
            credential_scope: None,
            ..scope.clone()
        },
    ] {
        assert!(
            !media::resource_matches_scope(&bound, &mismatch),
            "{mismatch:?}"
        );
    }
    assert_eq!(media::resources(&request.input).unwrap(), vec![scope]);

    let compound = document_request(
        json!({"type":"content","content":[
            {"type":"image","source":{"type":"file","file_id":"file_in_document"}}
        ]}),
        false,
    );
    let private_url = decode_request(
        ProviderProtocol::Gemini,
        &json!({"contents":[{"role":"user","parts":[
            {"fileData":{"mimeType":"application/pdf","fileUri":"https://generativelanguage.googleapis.com/v1beta/files/unbound"}}
        ]}]}),
    );
    for mut request in [compound, private_url] {
        let unbound = media::resources(&request.input).unwrap().remove(0);
        let scope_a = MediaResource {
            protocol: unbound.protocol,
            provider_id: Some("provider-a".into()),
            channel_id: Some("channel-a".into()),
            credential_scope: Some("credential-a".into()),
        };
        media::bind_resources(&mut request.input, &scope_a);
        let collected = media::resources(&request.input).unwrap();
        assert_eq!(collected, vec![scope_a.clone()]);
        let scope_b = MediaResource {
            provider_id: Some("provider-b".into()),
            ..scope_a
        };
        assert!(!media::resource_matches_scope(&collected[0], &scope_b));
    }
}

#[test]
fn media_replacing_private_uri_with_inline_bytes_removes_obsolete_resource_constraints() {
    for (mime, data) in [
        ("application/pdf", PDF),
        ("image/png", PNG),
        ("audio/wav", "UklGRg=="),
    ] {
        for stream in [false, true] {
            let mut request = decode_request(
                ProviderProtocol::Gemini,
                &json!({"contents":[{"role":"user","parts":[
                    {"fileData":{"mimeType":mime,"fileUri":"https://generativelanguage.googleapis.com/v1beta/files/private-file"}}
                ]}]}),
            );
            request.stream = Some(stream);
            assert_eq!(media::resources(&request.input).unwrap().len(), 1);
            for target in [
                ProviderProtocol::ChatCompletion,
                ProviderProtocol::Responses,
                ProviderProtocol::Messages,
            ] {
                assert!(encode_request(target, &request).is_err());
            }
            match &mut request.input[0] {
                Node::File { source, .. } => {
                    *source = FileSource::Base64 {
                        media_type: mime.into(),
                        data: data.into(),
                    }
                }
                Node::Image { source, .. } => {
                    *source = ImageSource::Base64 {
                        media_type: mime.into(),
                        data: data.into(),
                    }
                }
                Node::Audio { source, .. } => {
                    *source = AudioSource::Base64 {
                        media_type: mime.into(),
                        data: data.into(),
                    }
                }
                _ => unreachable!(),
            }
            let canonical = serde_json::to_value(&request).unwrap();
            for target in TARGETS {
                if mime.starts_with("audio/")
                    && matches!(
                        target,
                        ProviderProtocol::Messages | ProviderProtocol::Responses
                    )
                {
                    continue;
                }
                let prepared = media::prepare_request(&request, target).unwrap();
                assert!(media::resources(&prepared.input).unwrap().is_empty());
                let metadata = match &prepared.input[0] {
                    Node::File { metadata, .. }
                    | Node::Image { metadata, .. }
                    | Node::Audio { metadata, .. } => metadata,
                    _ => unreachable!(),
                };
                assert!(metadata.resource.is_none());
                let wire = encode_request(target, &prepared).unwrap();
                assert!(!wire.to_string().contains("private-file"));
                assert!(wire.to_string().contains(data));
            }
            assert_eq!(serde_json::to_value(&request).unwrap(), canonical);
        }
    }
}

#[test]
fn media_nested_tool_result_metadata_has_one_canonical_owner_and_survives_preparation() {
    for stream in [false, true] {
        let request = decode_request(
            ProviderProtocol::Messages,
            &json!({"model":"media-test","stream":stream,"messages":[
                {"role":"assistant","content":[{"type":"tool_use","id":"toolu_media","name":"read","input":{}}]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_media","content":[
                    {"type":"image","source":{"type":"base64","media_type":"image/png","data":PNG},"cache_control":{"type":"ephemeral"}},
                    {"type":"document","title":"Nested title","context":"Nested context","citations":{"enabled":true},
                        "source":{"type":"base64","media_type":"application/pdf","data":PDF}}
                ]}]}
            ]}),
        );
        let Node::ToolResult { content, .. } = &request.input[1] else {
            unreachable!()
        };
        let ToolResultContent::File {
            metadata,
            extra_body,
            ..
        } = &content[1]
        else {
            unreachable!()
        };
        assert_eq!(metadata.document_title.as_deref(), Some("Nested title"));
        assert_eq!(metadata.document_context.as_deref(), Some("Nested context"));
        assert_eq!(metadata.document_citations, Some(json!({"enabled":true})));
        for key in ["title", "context", "citations"] {
            assert!(!extra_body.contains_key(key));
        }
        let serialized = serde_json::to_value(&request.input[1]).unwrap();
        assert_eq!(
            serialized["content"][1]["metadata"]["document_title"],
            "Nested title"
        );
        assert!(serialized["content"][1].get("title").is_none());
        assert_eq!(serialized["content"][0]["metadata"], json!({}));
        let before = serde_json::to_value(content).unwrap();
        for target in TARGETS {
            if target == ProviderProtocol::ChatCompletion {
                assert!(media::prepare_tool_result_content(content, target).is_err());
                assert!(encode_request(target, &request).is_err());
                continue;
            }
            let prepared = media::prepare_tool_result_content(content, target).unwrap();
            assert_eq!(
                serde_json::to_value(&prepared).unwrap(),
                serde_json::to_value(
                    media::prepare_tool_result_content(&prepared, target).unwrap()
                )
                .unwrap()
            );
            let file = prepared
                .iter()
                .find_map(|part| match part {
                    ToolResultContent::File { metadata, .. } => Some(metadata),
                    _ => None,
                })
                .unwrap();
            assert_eq!(file.document_citations, Some(json!({"enabled":true})));
            let wire = encode_request(target, &request).unwrap();
            assert!(wire.to_string().contains("Nested title"));
            assert!(wire.to_string().contains("Nested context"));
            assert!(!wire.to_string().contains("document_citations"));
        }
        assert_eq!(serde_json::to_value(content).unwrap(), before);
    }
}

#[test]
fn media_messages_system_and_developer_media_are_explicit_errors() {
    for stream in [false, true] {
        for role in [OrdinaryRole::System, OrdinaryRole::Developer] {
            for mut request in [
                chat_request(
                    vec![
                        json!({"type":"image_url","image_url":{"url":format!("data:image/png;base64,{PNG}")}}),
                    ],
                    stream,
                ),
                chat_request(
                    vec![
                        json!({"type":"file","file":{"file_data":format!("data:application/pdf;base64,{PDF}")}}),
                    ],
                    stream,
                ),
                document_request(
                    json!({"type":"text","media_type":"text/plain","data":"Document"}),
                    stream,
                ),
            ] {
                match &mut request.input[0] {
                    Node::Image {
                        role: node_role, ..
                    }
                    | Node::File {
                        role: node_role, ..
                    } => *node_role = role,
                    _ => unreachable!(),
                }
                assert!(media::prepare_request(&request, ProviderProtocol::Messages).is_err());
                assert!(encode_request(ProviderProtocol::Messages, &request).is_err());
            }
        }
    }
}

#[test]
fn media_messages_url_documents_require_pdf_evidence_without_relabeling() {
    for stream in [false, true] {
        for (url, mime, accepted) in [
            ("https://example.com/document.pdf?token=opaque", None, true),
            (
                "https://example.com/download",
                Some("application/pdf"),
                true,
            ),
            ("https://example.com/download", None, false),
            ("https://example.com/document.docx", None, false),
            (
                "https://example.com/document.pdf",
                Some("application/json"),
                false,
            ),
        ] {
            let mut request = decode_request(
                ProviderProtocol::Responses,
                &json!({
                    "model":"media-test", "stream":stream, "input":[{"role":"user", "content":[
                        {"type":"input_file", "file_url":url}
                    ]}]
                }),
            );
            let Node::File { metadata, .. } = &mut request.input[0] else {
                unreachable!()
            };
            metadata.media_type = mime.map(str::to_owned);
            let before = serde_json::to_value(&request).unwrap();
            let result = encode_request(ProviderProtocol::Messages, &request);
            if accepted {
                let wire = result.unwrap();
                assert_eq!(
                    media_block(ProviderProtocol::Messages, &wire, 0)["source"],
                    json!({"type":"url", "url":url})
                );
            } else {
                assert!(result.unwrap_err().contains("PDF MIME"));
            }
            assert_eq!(serde_json::to_value(&request).unwrap(), before);
        }
        let request = document_request(
            json!({"type":"url", "url":"https://example.com/opaque"}),
            stream,
        );
        let wire = encode_request(ProviderProtocol::Messages, &request).unwrap();
        assert_eq!(
            media_block(ProviderProtocol::Messages, &wire, 0)["source"]["url"],
            "https://example.com/opaque"
        );
    }
}

#[test]
fn compound_document_text_fields_survive_preparation_and_citations_become_typed() {
    let citation = json!({"type":"char_location", "cited_text":"alpha", "document_index":0,
        "document_title":"Source", "start_char_index":0, "end_char_index":5});
    let content = json!([{"type":"text", "text":"alpha", "cache_control":{"type":"ephemeral"},
        "citations":[citation], "custom_text_field":{"present":true}}]);
    for stream in [false, true] {
        let single = document_request(
            json!({"type":"content", "content":{"type":"text", "text":"single"}}),
            stream,
        );
        let single_wire = encode_request(ProviderProtocol::Messages, &single).unwrap();
        assert_eq!(
            single_wire["messages"][0]["content"][0]["source"]["content"],
            json!([{"type":"text", "text":"single"}])
        );
        let request = document_request(json!({"type":"content", "content":content}), stream);
        let original = serde_json::to_value(&request).unwrap();
        let wire = encode_request(ProviderProtocol::Messages, &request).unwrap();
        assert_eq!(
            wire["messages"][0]["content"][0]["source"]["content"],
            content
        );
        for target in [
            ProviderProtocol::Responses,
            ProviderProtocol::ChatCompletion,
            ProviderProtocol::Gemini,
        ] {
            let prepared = media::prepare_request(&request, target).unwrap();
            let (citations, extra) = prepared
                .input
                .iter()
                .find_map(|node| match node {
                    Node::Text {
                        content,
                        citations,
                        extra_body,
                        ..
                    } if content == "alpha" => Some((citations, extra_body)),
                    _ => None,
                })
                .unwrap();
            assert_eq!(citations, &vec![citation.clone()]);
            assert!(!extra.contains_key("citations"));
        }
        assert_eq!(serde_json::to_value(&request).unwrap(), original);
    }
}

#[test]
fn compatible_media_aliases_keep_actual_bytes_types_and_private_audio_provenance() {
    for native in [
        json!({"type":"audio", "source":{"type":"base64", "url":"https://example.com/audio.wav"}}),
        json!({"type":"audio", "source":{"type":"url", "data":"ZkxhQw==", "media_type":"audio/flac"}}),
    ] {
        assert!(decode::parse_compatible_media_part(native.as_object().unwrap()).is_err());
    }
    for stream in [false, true] {
        let request = chat_request(
            vec![json!({"type":"image", "image_base64":"/9j/2Q=="})],
            stream,
        );
        assert!(
            matches!(&request.input[0], Node::Image { source:ImageSource::Base64 {media_type, ..}, .. } if media_type == "image/jpeg")
        );
        for target in TARGETS {
            let wire = encode_request(target, &request).unwrap();
            assert!(!wire.to_string().contains("image/png"));
            assert!(wire.to_string().contains("image/jpeg"));
        }
    }
    for native in [
        json!({"type":"input_audio", "input_audio":{"format":"flac", "data":"ZkxhQw=="}}),
        json!({"type":"audio", "source":{"type":"base64", "media_type":"audio/flac", "data":"ZkxhQw=="}}),
        json!({"type":"output_audio", "audio_url":"data:audio/flac;base64,ZkxhQw=="}),
    ] {
        let node =
            decode::parse_audio_node_from_obj(native.as_object().unwrap(), OrdinaryRole::User)
                .unwrap();
        assert!(
            matches!(node, Node::Audio {source:AudioSource::Base64 {media_type, data}, ..} if media_type == "audio/flac" && data == "ZkxhQw==")
        );
    }
    let native = json!({"type":"audio", "source":{"type":"url", "media_type":"audio/wav",
        "url":"https://generativelanguage.googleapis.com/v1beta/files/audio"}});
    let node =
        decode::parse_audio_node_from_obj(native.as_object().unwrap(), OrdinaryRole::User).unwrap();
    let Node::Audio { metadata, .. } = &node else {
        unreachable!()
    };
    assert_eq!(metadata.media_type.as_deref(), Some("audio/wav"));
    assert_eq!(
        media::resources(&[node]).unwrap()[0].protocol,
        ProviderProtocol::Gemini
    );
    assert!(media::is_audio_mime("VIDEO/AUDIO/WAV;rate=16000"));
    assert!(media::is_text_mime("TEXT/PLAIN; charset=utf-8"));
    for stream in [false, true] {
        let mut request = decode_request(
            ProviderProtocol::Gemini,
            &json!({"contents":[{"role":"user", "parts":[{"inlineData":{"mimeType":"VIDEO/AUDIO/WAV;rate=16000", "data":"UklGRgAAAABXQVZF"}}]}]}),
        );
        request.stream = Some(stream);
        let original = serde_json::to_value(&request).unwrap();
        let wire = encode_request(ProviderProtocol::ChatCompletion, &request).unwrap();
        assert_eq!(
            wire["messages"][0]["content"][0]["input_audio"]["format"],
            "wav"
        );
        assert_eq!(serde_json::to_value(&request).unwrap(), original);
    }
    let mut request = decode_request(
        ProviderProtocol::Gemini,
        &json!({
            "contents":[{"role":"user", "parts":[{"inlineData":{"mimeType":"APPLICATION/PDF", "data":PDF}}]}]
        }),
    );
    for stream in [false, true] {
        request.stream = Some(stream);
        let before = serde_json::to_value(&request).unwrap();
        let messages = encode_request(ProviderProtocol::Messages, &request).unwrap();
        assert_eq!(
            messages["messages"][0]["content"][0]["source"]["media_type"],
            "application/pdf"
        );
        let gemini = encode_request(ProviderProtocol::Gemini, &request).unwrap();
        assert_eq!(gemini["contents"][0]["parts"][0]["inlineData"]["data"], PDF);
        assert_eq!(serde_json::to_value(&request).unwrap(), before);
    }
}

#[test]
fn compatible_url_mime_and_inline_detail_are_typed_before_conversion() {
    for stream in [false, true] {
        for block in [
            json!({"type":"input_image", "url":format!("data:image/png;base64,{PNG}"), "detail":"high"}),
            json!({"type":"image", "source":{"type":"url", "url":format!("data:image/png;base64,{PNG}")}, "detail":"high"}),
            json!({"type":"image", "image_base64":PNG, "media_type":"image/png", "detail":"high"}),
        ] {
            let mut request = chat_request(vec![block], stream);
            let Node::Image {
                metadata,
                source,
                extra_body,
                ..
            } = &mut request.input[0]
            else {
                panic!("expected typed image");
            };
            assert!(matches!(source, ImageSource::Base64 { .. }));
            assert_eq!(metadata.detail.as_deref(), Some("high"));
            assert!(!extra_body.contains_key("detail"));
            metadata.detail = Some("low".into());
            let wire = encode_request(ProviderProtocol::Responses, &request).unwrap();
            assert_eq!(
                media_block(ProviderProtocol::Responses, &wire, 0)["detail"],
                "low"
            );
        }
        for block in [
            json!({"type":"input_file", "file_url":"https://example.com/opaque", "media_type":"application/pdf"}),
            json!({"type":"document", "source":{"type":"url", "url":"https://example.com/opaque", "media_type":"application/pdf"}}),
        ] {
            let request = chat_request(vec![block], stream);
            let Node::File {
                metadata,
                extra_body,
                ..
            } = &request.input[0]
            else {
                panic!("expected typed file");
            };
            assert_eq!(metadata.media_type.as_deref(), Some("application/pdf"));
            assert!(!extra_body.contains_key("media_type"));
            let wire = encode_request(ProviderProtocol::Gemini, &request).unwrap();
            assert_eq!(
                wire["contents"][0]["parts"][0]["fileData"]["mimeType"],
                "application/pdf"
            );
            encode_request(ProviderProtocol::Messages, &request).unwrap();
        }
        let request = document_request(
            json!({"type":"url", "url":"https://example.com/opaque", "media_type":"text/plain"}),
            stream,
        );
        let Node::File { metadata, .. } = &request.input[0] else {
            unreachable!()
        };
        assert_eq!(metadata.media_type.as_deref(), Some("text/plain"));
        assert!(encode_request(ProviderProtocol::Messages, &request).is_err());

        let request = chat_request(
            vec![
                json!({"type":"image_url", "image_url":{"url":"https://example.com/image", "media_type":"image/jpeg"}}),
            ],
            stream,
        );
        let wire = encode_request(ProviderProtocol::Gemini, &request).unwrap();
        assert_eq!(
            wire["contents"][0]["parts"][0]["fileData"]["mimeType"],
            "image/jpeg"
        );
        let request = chat_request(
            vec![
                json!({"type":"audio", "audio_url":{"url":"https://example.com/opaque", "media_type":"audio/wav"}}),
            ],
            stream,
        );
        let wire = encode_request(ProviderProtocol::Gemini, &request).unwrap();
        assert_eq!(
            wire["contents"][0]["parts"][0]["fileData"]["mimeType"],
            "audio/wav"
        );
        let request = chat_request(
            vec![
                json!({"type":"audio", "source":{"type":"base64", "media_type":"AUDIO/WAV;rate=16000", "data":"UklGRgAAAABXQVZF"}}),
            ],
            stream,
        );
        let before = serde_json::to_value(&request).unwrap();
        let wire = encode_request(ProviderProtocol::ChatCompletion, &request).unwrap();
        assert_eq!(
            wire["messages"][0]["content"][0]["input_audio"]["format"],
            "wav"
        );
        assert_eq!(serde_json::to_value(&request).unwrap(), before);
    }
}

#[test]
fn media_malformed_bytes_and_non_utf8_messages_text_fail_without_partial_output() {
    for stream in [false, true] {
        for malformed in [
            "data:application/pdf;base64,%%%",
            "data:application/pdf;base64,a",
            "data:;base64,JVBERi0xLjcK",
            "data:not-a-mime;base64,JVBERi0xLjcK",
        ] {
            let request = chat_request(
                vec![
                    json!({"type":"text","text":"Must not hide the malformed file"}),
                    json!({"type":"file","file":{"file_data":malformed}}),
                ],
                stream,
            );
            for target in TARGETS {
                assert!(encode_request(target, &request).is_err(), "{target:?}");
            }
        }
        let mut request = decode_request(
            ProviderProtocol::Gemini,
            &json!({"contents":[{"role":"user","parts":[
                {"text":"Must not drop non-UTF-8 text"},{"inlineData":{"mimeType":"text/plain","data":"/w=="}}
            ]}]}),
        );
        request.stream = Some(stream);
        let before = serde_json::to_value(&request).unwrap();
        let error = encode_request(ProviderProtocol::Messages, &request).unwrap_err();
        assert!(error.contains("UTF-8"));
        assert_eq!(serde_json::to_value(&request).unwrap(), before);
    }
}

#[test]
fn media_deleted_private_provenance_cannot_be_restored_from_native_shapes() {
    fn metadata(node: &mut Node) -> &mut super::MediaMetadata {
        match node {
            Node::Image { metadata, .. } | Node::File { metadata, .. } => metadata,
            Node::ToolResult { content, .. } => match &mut content[0] {
                ToolResultContent::File { metadata, .. } => metadata,
                _ => panic!("expected a typed compound tool-result file"),
            },
            _ => panic!("expected a typed private media reference"),
        }
    }

    for stream in [false, true] {
        let compound_source = json!({"type":"content","content":[
            {"type":"image","source":{"type":"file","file_id":"file_private_child"}}
        ]});
        let top_level = document_request(compound_source.clone(), stream);
        let nested = decode_request(
            ProviderProtocol::Messages,
            &json!({"model":"media-test","stream":stream,"messages":[{"role":"user","content":[
                {"type":"tool_result","tool_use_id":"toolu_private","content":[
                    {"type":"document","source":compound_source}
                ]}
            ]}]}),
        );
        let private_file = decode_request(
            ProviderProtocol::Gemini,
            &json!({"contents":[{"role":"user","parts":[
                {"fileData":{"mimeType":"application/pdf","fileUri":"https://generativelanguage.googleapis.com/v1beta/files/private-document"}}
            ]}]}),
        );
        let private_image = chat_request(
            vec![json!({"type":"image_url","image_url":{
                "url":"https://generativelanguage.googleapis.com/v1beta/files/private-image",
                "detail":"high"
            }})],
            stream,
        );
        for (fixture, source_protocol, mut request) in [
            ("compound document", ProviderProtocol::Messages, top_level),
            (
                "compound tool-result file",
                ProviderProtocol::Messages,
                nested,
            ),
            (
                "Gemini private file URI",
                ProviderProtocol::Gemini,
                private_file,
            ),
            (
                "Chat object image URL",
                ProviderProtocol::Gemini,
                private_image,
            ),
        ] {
            request.stream = Some(stream);
            assert_eq!(
                metadata(&mut request.input[0])
                    .resource
                    .as_ref()
                    .map(|resource| resource.protocol),
                Some(source_protocol),
                "{fixture} must receive typed provenance during native decoding",
            );
            let scope_a = MediaResource {
                protocol: source_protocol,
                provider_id: Some("provider-a".into()),
                channel_id: Some("channel-a".into()),
                credential_scope: Some("credential-a".into()),
            };
            media::bind_resources(&mut request.input, &scope_a);
            assert_eq!(media::resources(&request.input).unwrap(), vec![scope_a]);

            metadata(&mut request.input[0]).resource = None;
            let deleted = serde_json::to_value(&request).unwrap();
            assert!(media::resources(&request.input).is_err(), "{fixture}");
            for target in TARGETS {
                assert!(
                    encode_request(target, &request).is_err(),
                    "{fixture} -> {target:?}"
                );
            }
            assert_eq!(serde_json::to_value(&request).unwrap(), deleted);

            let scope_b = MediaResource {
                protocol: source_protocol,
                provider_id: Some("provider-b".into()),
                channel_id: Some("channel-b".into()),
                credential_scope: Some("credential-b".into()),
            };
            media::bind_resources(&mut request.input, &scope_b);
            assert!(
                metadata(&mut request.input[0]).resource.is_none(),
                "{fixture}"
            );
            assert!(media::resources(&request.input).is_err(), "{fixture}");
            for target in TARGETS {
                assert!(
                    encode_request(target, &request).is_err(),
                    "{fixture} rebound -> {target:?}"
                );
            }
        }
    }
}

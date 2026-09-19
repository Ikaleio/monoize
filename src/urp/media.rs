use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::json;

pub fn error_body(message: &str) -> Value {
    json!({"error":{"type":"invalid_request_error","code":"unsupported_media","message":message}})
}

pub fn audio_mime_for_format(format: &str) -> Option<&'static str> {
    Some(match format {
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "opus" => "audio/opus",
        "aac" => "audio/aac",
        "pcm16" => "audio/pcm;rate=24000",
        _ => return None,
    })
}

pub fn mime_essence(mime: &str) -> String {
    mime.split(';')
        .next()
        .unwrap_or(mime)
        .trim()
        .to_ascii_lowercase()
}

pub fn is_audio_mime(mime: &str) -> bool {
    let mime = mime_essence(mime);
    mime.starts_with("audio/") || matches!(mime.as_str(), "video/audio/s16le" | "video/audio/wav")
}

pub fn is_text_mime(mime: &str) -> bool {
    let mime = mime_essence(mime);
    mime.starts_with("text/")
        || matches!(
            mime.as_str(),
            "application/json"
                | "application/xml"
                | "application/javascript"
                | "application/x-javascript"
                | "application/x-typescript"
                | "application/x-python-code"
                | "application/x-ipynb+json"
                | "application/sql"
                | "application/yaml"
        )
}

pub fn decode_text(data: &str) -> Result<String, String> {
    String::from_utf8(decode_bytes(data)?)
        .map_err(|_| "The document is not valid UTF-8 text.".into())
}

fn decode_bytes(data: &str) -> Result<Vec<u8>, String> {
    STANDARD
        .decode(data)
        .map_err(|_| "Media data must contain valid raw Base64 bytes.".into())
}

pub fn parse_data_url(url: &str) -> Option<(&str, &str)> {
    let (header, data) = url.strip_prefix("data:")?.split_once(',')?;
    let mime = header.strip_suffix(";base64")?;
    (!mime.is_empty()).then_some((mime, data))
}

pub fn infer_mime(data: &str, filename: Option<&str>) -> String {
    if let Ok(bytes) = decode_bytes(data) {
        let mime = if bytes.starts_with(b"%PDF-") {
            Some("application/pdf")
        } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some("image/png")
        } else if bytes.starts_with(b"\xff\xd8\xff") {
            Some("image/jpeg")
        } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            Some("image/gif")
        } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
            Some("image/webp")
        } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") {
            Some("audio/wav")
        } else if bytes.starts_with(b"fLaC") {
            Some("audio/flac")
        } else if bytes.starts_with(b"ID3") {
            Some("audio/mpeg")
        } else {
            None
        };
        if let Some(mime) = mime {
            return mime.into();
        }
    }
    filename
        .and_then(|name| mime_guess::from_path(name).first_raw())
        .unwrap_or("application/octet-stream")
        .into()
}

pub fn resource_for_url(url: &str) -> Option<MediaResource> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let private = parsed.scheme() == "gs"
        || (parsed.host_str() == Some("generativelanguage.googleapis.com")
            && parsed.path().contains("/files/"));
    private.then_some(MediaResource {
        protocol: ProviderProtocol::Gemini,
        provider_id: None,
        channel_id: None,
        credential_scope: None,
    })
}

pub fn resource_matches(metadata: &MediaMetadata, target: ProviderProtocol) -> bool {
    metadata
        .resource
        .as_ref()
        .is_some_and(|resource| protocol_matches(resource.protocol, target))
}

pub fn protocol_matches(source: ProviderProtocol, target: ProviderProtocol) -> bool {
    source == target
        || (matches!(
            source,
            ProviderProtocol::Responses | ProviderProtocol::ChatCompletion
        ) && matches!(
            target,
            ProviderProtocol::Responses | ProviderProtocol::ChatCompletion
        ))
}

fn check_resource(metadata: &MediaMetadata, target: ProviderProtocol) -> Result<(), String> {
    if let Some(resource) = &metadata.resource {
        if !protocol_matches(resource.protocol, target) {
            return Err("The file reference belongs to another provider protocol. Supply file bytes or a public URL.".into());
        }
    }
    Ok(())
}

fn check_url(url: &str, metadata: &MediaMetadata, target: ProviderProtocol) -> Result<(), String> {
    check_resource(metadata, target)?;
    if let Some(resource) = resource_for_url(url) {
        if metadata.resource.is_none() {
            return Err("Private media URL has no source provenance.".into());
        }
        if !protocol_matches(resource.protocol, target) {
            return Err("The private file URI cannot be sent to this provider. Supply file bytes or a public URL.".into());
        }
    }
    let parsed = reqwest::Url::parse(url).map_err(|_| "Media URL is invalid.".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https")
        && !(parsed.scheme() == "gs" && target == ProviderProtocol::Gemini)
    {
        return Err("The target does not support this media URL scheme.".into());
    }
    Ok(())
}

fn image(
    source: &mut ImageSource,
    metadata: &mut MediaMetadata,
    target: ProviderProtocol,
    nested: bool,
) -> Result<(), String> {
    if let ImageSource::Url { url, detail } = source {
        if url.starts_with("data:") {
            let (mime, data) = parse_data_url(url)
                .ok_or("Image data URL must contain a MIME type and Base64 bytes.")?;
            metadata.detail = detail.take().or(metadata.detail.take());
            *source = ImageSource::Base64 {
                media_type: mime.into(),
                data: data.into(),
            };
        }
    }
    if matches!(source, ImageSource::Base64 { .. }) {
        metadata.resource = None;
    }
    check_resource(metadata, target)?;
    match source {
        ImageSource::Base64 { media_type, data } => {
            decode_bytes(data)?;
            let parsed_mime = media_type
                .parse::<mime::Mime>()
                .map_err(|_| "Image MIME type is invalid.".to_string())?;
            let mime = parsed_mime.essence_str();
            let accepted = match target {
                ProviderProtocol::Messages
                | ProviderProtocol::ChatCompletion
                | ProviderProtocol::Responses => matches!(
                    mime,
                    "image/png" | "image/jpeg" | "image/gif" | "image/webp"
                ),
                ProviderProtocol::Gemini => matches!(
                    mime,
                    "image/png"
                        | "image/jpeg"
                        | "image/jpg"
                        | "image/webp"
                        | "image/heic"
                        | "image/heif"
                        | "image/gif"
                        | "image/avif"
                ),
                _ => true,
            };
            if !accepted {
                return Err(format!(
                    "The target does not support image MIME type {mime}. Convert the image bytes first."
                ));
            }
            *media_type = mime.to_owned();
        }
        ImageSource::Url { url, .. } => {
            check_url(url, metadata, target)?;
            if nested && target == ProviderProtocol::Gemini {
                return Err("Gemini function-result images require inline Base64 data; URL parts are not supported.".into());
            }
            if let Some(media_type) = &mut metadata.media_type {
                let parsed_mime = media_type
                    .parse::<mime::Mime>()
                    .map_err(|_| "Image MIME type is invalid.".to_string())?;
                let mime = parsed_mime.essence_str();
                if matches!(
                    target,
                    ProviderProtocol::Messages
                        | ProviderProtocol::ChatCompletion
                        | ProviderProtocol::Responses
                ) && !matches!(
                    mime,
                    "image/png" | "image/jpeg" | "image/gif" | "image/webp"
                ) {
                    return Err(format!(
                        "The target does not support image MIME type {mime}."
                    ));
                }
                *media_type = mime.to_owned();
            }
        }
        ImageSource::FileId { .. } => {
            if !resource_matches(metadata, target) {
                return Err("Image file_id has no compatible provider provenance.".into());
            }
            if matches!(
                target,
                ProviderProtocol::ChatCompletion | ProviderProtocol::Gemini
            ) {
                return Err(
                    "This target has no image file_id carrier. Supply bytes or a public image URL."
                        .into(),
                );
            }
        }
    }
    Ok(())
}

fn file(
    source: &mut FileSource,
    metadata: &mut MediaMetadata,
    target: ProviderProtocol,
    nested: bool,
) -> Result<(), String> {
    if matches!(source, FileSource::Text { .. } | FileSource::Base64 { .. }) {
        metadata.resource = None;
    }
    check_resource(metadata, target)?;
    match source {
        FileSource::Base64 { media_type, data } => {
            decode_bytes(data)?;
            if mime_essence(media_type) == "application/octet-stream" || media_type.is_empty() {
                *media_type = infer_mime(data, metadata.filename.as_deref());
            }
            let mime = mime_essence(media_type);
            if mime == "application/octet-stream" {
                return Err("File MIME type is unknown. Supply a MIME-bearing data URL or a filename with a supported extension.".into());
            }
            if target == ProviderProtocol::Messages && mime == "application/pdf" {
                *media_type = "application/pdf".into();
            } else if target == ProviderProtocol::Messages {
                if is_text_mime(media_type) {
                    *source = FileSource::Text {
                        text: decode_text(data)?,
                    };
                } else {
                    return Err(format!(
                        "Messages does not support {media_type} as a document. Supply PDF or UTF-8 text."
                    ));
                }
            } else if (is_audio_mime(media_type) || mime.starts_with("video/"))
                && target != ProviderProtocol::Gemini
            {
                return Err(format!(
                    "The target does not support {media_type} as a file input."
                ));
            } else if target == ProviderProtocol::Gemini
                && !(matches!(mime.as_str(), "application/pdf" | "application/rtf")
                    || is_text_mime(media_type)
                    || is_audio_mime(media_type)
                    || mime.starts_with("video/")
                    || mime.starts_with("image/"))
            {
                return Err(format!(
                    "Gemini does not support file MIME type {media_type}. Convert the document first."
                ));
            }
        }
        FileSource::Url { url } => {
            check_url(url, metadata, target)?;
            if target == ProviderProtocol::ChatCompletion
                || (nested && target == ProviderProtocol::Gemini)
            {
                return Err(
                    "This target requires file bytes for this file URL. Supply Base64 data.".into(),
                );
            }
            if target == ProviderProtocol::Messages {
                let mime = metadata.media_type.clone().or_else(|| {
                    reqwest::Url::parse(url).ok().and_then(|url| {
                        mime_guess::from_path(url.path())
                            .first_raw()
                            .map(str::to_owned)
                    })
                });
                if mime.as_deref().map(mime_essence).as_deref() != Some("application/pdf") {
                    return Err("Messages URL documents require a known PDF MIME type. Supply PDF bytes when the URL type is unknown.".into());
                }
            }
        }
        FileSource::FileId { .. } => {
            if !resource_matches(metadata, target) {
                return Err(
                    "File ID has no compatible provider provenance. Supply file bytes.".into(),
                );
            }
            if target == ProviderProtocol::Gemini {
                return Err("Gemini does not support this file_id carrier.".into());
            }
        }
        FileSource::Content { .. } if target == ProviderProtocol::Messages => {
            let parts =
                prepare_tool_result_content(&bound_document_parts(source, metadata)?, target)?;
            let content = parts
                .into_iter()
                .map(|part| match part {
                    ToolResultContent::Text { text, extra_body } => {
                        let mut obj: serde_json::Map<String, Value> = extra_body
                            .into_iter()
                            .filter(|(key, _)| !key.starts_with("_monoize_"))
                            .collect();
                        obj.insert("type".into(), json!("text"));
                        obj.insert("text".into(), json!(text));
                        Value::Object(obj)
                    }
                    ToolResultContent::Image {
                        source, extra_body, ..
                    } => {
                        let source = match source {
                            ImageSource::Url { url, .. } => json!({"type":"url", "url":url}),
                            ImageSource::Base64 { media_type, data } => {
                                json!({"type":"base64", "media_type":media_type, "data":data})
                            }
                            ImageSource::FileId { file_id, .. } => {
                                json!({"type":"file", "file_id":file_id})
                            }
                        };
                        let mut obj = serde_json::Map::new();
                        obj.extend(
                            extra_body
                                .into_iter()
                                .filter(|(key, _)| !key.starts_with("_monoize_")),
                        );
                        obj.insert("type".into(), json!("image"));
                        obj.insert("source".into(), source);
                        Value::Object(obj)
                    }
                    _ => unreachable!(),
                })
                .collect();
            *source = FileSource::Content { content };
        }
        FileSource::Content { .. } => {
            bound_document_parts(source, metadata)?;
        }
        FileSource::Text { .. } => {}
    }
    Ok(())
}

fn document_parts(source: &FileSource) -> Result<Vec<ToolResultContent>, String> {
    match source {
        FileSource::Text { text } => Ok(vec![ToolResultContent::Text {
            text: text.clone(),
            extra_body: Default::default(),
        }]),
        FileSource::Content { content } => content
            .iter()
            .map(|part| {
                let obj = part
                    .as_object()
                    .ok_or("Document content blocks must be objects.")?;
                match obj.get("type").and_then(Value::as_str) {
                    Some("text") => Ok(ToolResultContent::Text {
                        text: obj
                            .get("text")
                            .and_then(Value::as_str)
                            .ok_or("Document text is missing.")?
                            .into(),
                        extra_body: decode::split_extra(obj, &["type", "text"]),
                    }),
                    Some("image") | Some("image_url") | Some("input_image") => {
                        match decode::parse_image_node_from_obj(obj, OrdinaryRole::User)
                            .ok_or("Document image is invalid.")?
                        {
                            Node::Image {
                                source,
                                metadata,
                                extra_body,
                                ..
                            } => Ok(ToolResultContent::Image {
                                source,
                                metadata,
                                extra_body,
                            }),
                            _ => unreachable!(),
                        }
                    }
                    _ => Err("The compound document contains an unsupported content block.".into()),
                }
            })
            .collect(),
        _ => Err("Expected a text or compound document.".into()),
    }
}

pub(super) fn document_resource(source: &FileSource) -> Option<MediaResource> {
    document_parts(source)
        .ok()?
        .into_iter()
        .find_map(|part| match part {
            ToolResultContent::Image { metadata, .. } => metadata.resource,
            _ => None,
        })
}

fn bound_document_parts(
    source: &FileSource,
    metadata: &MediaMetadata,
) -> Result<Vec<ToolResultContent>, String> {
    let mut parts = document_parts(source)?;
    for part in &mut parts {
        if let ToolResultContent::Image {
            metadata: child, ..
        } = part
        {
            if let Some(resource) = &mut child.resource {
                let parent = metadata
                    .resource
                    .as_ref()
                    .ok_or("Compound document has no source provenance for its private media.")?;
                if !protocol_matches(resource.protocol, parent.protocol) {
                    return Err(
                        "Compound document media belongs to a different source protocol.".into(),
                    );
                }
                resource.provider_id = parent.provider_id.clone();
                resource.channel_id = parent.channel_id.clone();
                resource.credential_scope = parent.credential_scope.clone();
            }
        }
    }
    Ok(parts)
}

fn document_prefix(metadata: &MediaMetadata) -> Option<ToolResultContent> {
    let mut text = [
        metadata.document_title.as_deref(),
        metadata.document_context.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    (!text.is_empty()).then_some(ToolResultContent::Text {
        text,
        extra_body: Default::default(),
    })
}

pub fn prepare_tool_result_content(
    content: &[ToolResultContent],
    target: ProviderProtocol,
) -> Result<Vec<ToolResultContent>, String> {
    let mut result = Vec::new();
    for part in content {
        let mut part = part.clone();
        match &mut part {
            ToolResultContent::Image {
                source, metadata, ..
            } => {
                if target == ProviderProtocol::ChatCompletion {
                    return Err(
                        "Chat tool results support text only; image content cannot be discarded."
                            .into(),
                    );
                }
                image(source, metadata, target, true)?;
            }
            ToolResultContent::File {
                source, metadata, ..
            } => {
                file(source, metadata, target, true)?;
                if target != ProviderProtocol::Messages {
                    result.extend(document_prefix(metadata));
                    metadata.document_title = None;
                    metadata.document_context = None;
                }
                if matches!(source, FileSource::Text { .. } | FileSource::Content { .. })
                    && target != ProviderProtocol::Messages
                {
                    let mut parts = Vec::new();
                    parts.extend(bound_document_parts(source, metadata)?);
                    result.extend(prepare_tool_result_content(&parts, target)?);
                    continue;
                }
                if target == ProviderProtocol::ChatCompletion {
                    return Err(
                        "Chat tool results support text only; file content cannot be discarded."
                            .into(),
                    );
                }
            }
            _ => {}
        }
        result.push(part);
    }
    Ok(result)
}

pub fn prepare_nodes(nodes: &[Node], target: ProviderProtocol) -> Result<Vec<Node>, String> {
    let mut result = Vec::new();
    for node in nodes {
        let mut node = node.clone();
        match &mut node {
            Node::Image {
                source,
                metadata,
                role,
                ..
            } => {
                if target == ProviderProtocol::Messages
                    && matches!(role, OrdinaryRole::System | OrdinaryRole::Developer)
                {
                    return Err(
                        "Messages system content supports text only; images cannot be discarded."
                            .into(),
                    );
                }
                if target == ProviderProtocol::ChatCompletion && *role != OrdinaryRole::User {
                    return Err("Chat image content is supported only in user messages.".into());
                }
                image(source, metadata, target, false)?;
            }
            Node::File {
                source,
                metadata,
                role,
                id,
                ..
            } => {
                if target == ProviderProtocol::Messages
                    && matches!(role, OrdinaryRole::System | OrdinaryRole::Developer)
                {
                    return Err("Messages system content supports text only; documents cannot be discarded.".into());
                }
                file(source, metadata, target, false)?;
                if target != ProviderProtocol::Messages {
                    if let Some(ToolResultContent::Text { text, extra_body }) =
                        document_prefix(metadata)
                    {
                        result.push(Node::Text {
                            logprobs: None,
                            id: None,
                            role: *role,
                            content: text,
                            citations: vec![],
                            phase: None,
                            signature: None,
                            extra_body,
                        });
                    }
                    metadata.document_title = None;
                    metadata.document_context = None;
                }
                if matches!(source, FileSource::Text { .. } | FileSource::Content { .. })
                    && target != ProviderProtocol::Messages
                {
                    let mut parts = Vec::new();
                    parts.extend(bound_document_parts(source, metadata)?);
                    let expanded: Vec<Node> = parts
                        .into_iter()
                        .map(|part| match part {
                            ToolResultContent::Text {
                                text,
                                mut extra_body,
                            } => Node::Text {
                                logprobs: None,
                                id: id.clone(),
                                role: *role,
                                content: text,
                                citations: crate::urp::citations::decode(
                                    extra_body
                                        .remove("citations")
                                        .and_then(|value| value.as_array().cloned())
                                        .unwrap_or_default(),
                                    ProviderProtocol::Messages,
                                ),
                                phase: None,
                                signature: None,
                                extra_body,
                            },
                            ToolResultContent::Image {
                                source,
                                metadata,
                                extra_body,
                            } => Node::Image {
                                id: id.clone(),
                                role: *role,
                                source,
                                metadata,
                                extra_body,
                            },
                            _ => unreachable!(),
                        })
                        .collect();
                    result.extend(prepare_nodes(&expanded, target)?);
                    continue;
                }
                if target == ProviderProtocol::ChatCompletion && *role != OrdinaryRole::User {
                    return Err("Chat file content is supported only in user messages.".into());
                }
            }
            Node::Audio {
                source,
                metadata,
                role,
                extra_body,
                ..
            } => {
                if matches!(source, AudioSource::Base64 { .. }) {
                    metadata.resource = None;
                }
                check_resource(metadata, target)?;
                if target == ProviderProtocol::ChatCompletion && *role != OrdinaryRole::User {
                    if *role == OrdinaryRole::Assistant
                        && metadata.reference_id.is_some()
                        && extra_body.contains_key(CHAT_MESSAGE_AUDIO_EXTRA_KEY)
                    {
                        result.push(node);
                        continue;
                    }
                    return Err(
                        "Chat audio requires a user input or a native assistant audio reference."
                            .into(),
                    );
                }
                if matches!(
                    target,
                    ProviderProtocol::Messages | ProviderProtocol::Responses
                ) {
                    return Err("This target has no supported audio input carrier.".into());
                }
                match source {
                    AudioSource::Url { url } => {
                        check_url(url, metadata, target)?;
                        if target == ProviderProtocol::ChatCompletion {
                            return Err("Chat audio input requires inline WAV or MP3 bytes.".into());
                        }
                    }
                    AudioSource::Base64 { media_type, data } => {
                        decode_bytes(data)?;
                        if media_type == "audio/unknown" || !is_audio_mime(media_type) {
                            return Err("Audio format is unknown. Supply the actual format before conversion.".into());
                        }
                        if target == ProviderProtocol::ChatCompletion {
                            *media_type = match mime_essence(media_type).as_str() {
                                "audio/wav" | "audio/x-wav" | "video/audio/wav" => {
                                    "audio/wav".into()
                                }
                                "audio/mpeg" | "audio/mp3" => "audio/mpeg".into(),
                                _ => {
                                    return Err(
                                        "Chat audio input supports WAV or MP3 bytes.".into()
                                    );
                                }
                            };
                        }
                    }
                }
            }
            Node::ToolResult { content, .. } => {
                *content = prepare_tool_result_content(content, target)?
            }
            _ => {}
        }
        result.push(node);
    }
    Ok(result)
}

pub fn prepare_request(
    request: &UrpRequest,
    target: ProviderProtocol,
) -> Result<UrpRequest, String> {
    let mut request = request.clone();
    request.input = prepare_nodes(&request.input, target)?;
    Ok(request)
}

fn visit_file_resources(
    source: &FileSource,
    metadata: &MediaMetadata,
    resources: &mut Vec<MediaResource>,
) -> Result<(), String> {
    match source {
        FileSource::FileId { .. } => resources.push(
            metadata
                .resource
                .clone()
                .ok_or("File ID has no source provenance.")?,
        ),
        FileSource::Url { url } => {
            visit_url_resources(url, metadata, resources)?;
        }
        FileSource::Content { .. } => {
            for part in bound_document_parts(source, metadata)? {
                visit_part_resources(&part, resources)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn visit_url_resources(
    url: &str,
    metadata: &MediaMetadata,
    resources: &mut Vec<MediaResource>,
) -> Result<(), String> {
    if resource_for_url(url).is_some() && metadata.resource.is_none() {
        return Err("Private media URL has no source provenance.".into());
    }
    resources.extend(metadata.resource.clone());
    Ok(())
}

fn visit_image_resources(
    source: &ImageSource,
    metadata: &MediaMetadata,
    resources: &mut Vec<MediaResource>,
) -> Result<(), String> {
    match source {
        ImageSource::FileId { .. } => resources.push(
            metadata
                .resource
                .clone()
                .ok_or("Image file ID has no source provenance.")?,
        ),
        ImageSource::Url { url, .. } => {
            visit_url_resources(url, metadata, resources)?;
        }
        _ => {}
    }
    Ok(())
}

fn visit_part_resources(
    part: &ToolResultContent,
    resources: &mut Vec<MediaResource>,
) -> Result<(), String> {
    match part {
        ToolResultContent::Image {
            source, metadata, ..
        } => visit_image_resources(source, metadata, resources)?,
        ToolResultContent::File {
            source, metadata, ..
        } => visit_file_resources(source, metadata, resources)?,
        _ => {}
    }
    Ok(())
}

pub fn resources(nodes: &[Node]) -> Result<Vec<MediaResource>, String> {
    let mut resources = Vec::new();
    for node in nodes {
        match node {
            Node::Image {
                source, metadata, ..
            } => visit_image_resources(source, metadata, &mut resources)?,
            Node::File {
                source, metadata, ..
            } => visit_file_resources(source, metadata, &mut resources)?,
            Node::Audio {
                source: AudioSource::Url { url },
                metadata,
                ..
            } => visit_url_resources(url, metadata, &mut resources)?,
            Node::ToolResult { content, .. } => {
                for part in content {
                    visit_part_resources(part, &mut resources)?;
                }
            }
            _ => {}
        }
    }
    Ok(resources)
}

pub fn resource_matches_scope(resource: &MediaResource, target: &MediaResource) -> bool {
    protocol_matches(resource.protocol, target.protocol)
        && resource
            .provider_id
            .as_ref()
            .is_none_or(|id| target.provider_id.as_ref() == Some(id))
        && resource
            .channel_id
            .as_ref()
            .is_none_or(|id| target.channel_id.as_ref() == Some(id))
        && resource
            .credential_scope
            .as_ref()
            .is_none_or(|id| target.credential_scope.as_ref() == Some(id))
}

pub fn bind_resources(nodes: &mut [Node], target: &MediaResource) {
    fn bind(metadata: &mut MediaMetadata, target: &MediaResource) {
        if let Some(resource) = &mut metadata.resource {
            if !resource_matches_scope(resource, target) {
                return;
            }
            resource.provider_id = target.provider_id.clone();
            resource.channel_id = target.channel_id.clone();
            resource.credential_scope = target.credential_scope.clone();
        }
    }
    fn bind_image(source: &ImageSource, metadata: &mut MediaMetadata, target: &MediaResource) {
        match source {
            ImageSource::Url { .. } => bind(metadata, target),
            ImageSource::FileId { .. } => bind(metadata, target),
            ImageSource::Base64 { .. } => metadata.resource = None,
        }
    }
    fn bind_file(source: &FileSource, metadata: &mut MediaMetadata, target: &MediaResource) {
        match source {
            FileSource::Url { .. } => bind(metadata, target),
            FileSource::FileId { .. } => bind(metadata, target),
            FileSource::Content { .. } => {
                let mut children = Vec::new();
                if visit_file_resources(source, metadata, &mut children).is_ok() {
                    metadata.resource = children.into_iter().next();
                    bind(metadata, target);
                }
            }
            _ => metadata.resource = None,
        }
    }
    for node in nodes {
        match node {
            Node::Image {
                source, metadata, ..
            } => bind_image(source, metadata, target),
            Node::File {
                source, metadata, ..
            } => bind_file(source, metadata, target),
            Node::Audio {
                source: AudioSource::Url { .. },
                metadata,
                ..
            } => {
                bind(metadata, target);
            }
            Node::Audio { metadata, .. } => metadata.resource = None,
            Node::ToolResult { content, .. } => {
                for part in content {
                    match part {
                        ToolResultContent::Image {
                            source, metadata, ..
                        } => bind_image(source, metadata, target),
                        ToolResultContent::File {
                            source, metadata, ..
                        } => bind_file(source, metadata, target),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

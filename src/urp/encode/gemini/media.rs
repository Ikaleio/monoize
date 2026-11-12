//! Gemini media and call validation before encoding (GEM-3, media-transport.spec.md).
//! A node Gemini cannot carry fails the whole request or response; nothing is dropped.

use crate::urp::media::{is_text_mime, mime_essence};
use crate::urp::{
    AudioSource, FileSource, ImageSource, MediaMetadata, Node, ToolCallType, ToolResultContent,
};
use serde_json::Value;

pub(super) fn validate_media_nodes(nodes: &[Node]) -> Result<(), String> {
    for node in nodes {
        match node {
            Node::ToolCall {
                tool_type: ToolCallType::Custom,
                ..
            }
            | Node::ToolResult {
                tool_type: ToolCallType::Custom,
                ..
            } => {
                return Err("Gemini does not support custom tool calls or results without a function bridge.".into());
            }
            Node::ToolCall {
                name, arguments, ..
            } => {
                if name.is_empty() {
                    return Err("Gemini function calls require a non-empty name.".into());
                }
                let args: Value = serde_json::from_str(arguments).map_err(|error| {
                    format!("Gemini function arguments must be valid JSON: {error}")
                })?;
                if !args.is_object() {
                    return Err("Gemini function arguments must be a JSON object.".into());
                }
            }
            Node::Image {
                source, metadata, ..
            } => validate_image(source, metadata)?,
            Node::File {
                source, metadata, ..
            } => validate_file(source, metadata)?,
            Node::Audio {
                source, metadata, ..
            } => match source {
                AudioSource::Base64 { media_type, .. } => validate_media_mime(media_type, None)?,
                AudioSource::Url { url } => {
                    if let Some(mime) = &metadata.media_type {
                        validate_media_mime(mime, Some(url))?;
                    }
                }
            },
            Node::ToolResult { content, .. } => {
                for part in content {
                    match part {
                        ToolResultContent::Image {
                            source, metadata, ..
                        } => validate_image(source, metadata)?,
                        ToolResultContent::File {
                            source, metadata, ..
                        } => validate_file(source, metadata)?,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_image(source: &ImageSource, metadata: &MediaMetadata) -> Result<(), String> {
    match source {
        ImageSource::Base64 { media_type, .. } => validate_media_mime(media_type, None),
        ImageSource::Url { url, .. } => metadata
            .media_type
            .as_deref()
            .map_or(Ok(()), |mime| validate_media_mime(mime, Some(url))),
        _ => Ok(()),
    }
}

fn validate_file(source: &FileSource, metadata: &MediaMetadata) -> Result<(), String> {
    match source {
        FileSource::Base64 { media_type, .. } => validate_media_mime(media_type, None),
        FileSource::Url { url } => metadata
            .media_type
            .as_deref()
            .map_or(Ok(()), |mime| validate_media_mime(mime, Some(url))),
        _ => Ok(()),
    }
}

/// Documented Gemini media MIME types. `video/*` is accepted only for YouTube URLs.
fn validate_media_mime(mime: &str, url: Option<&str>) -> Result<(), String> {
    let base = mime_essence(mime);
    let supported = matches!(
        base.as_str(),
        "image/png"
            | "image/jpeg"
            | "image/jpg"
            | "image/webp"
            | "image/heic"
            | "image/heif"
            | "image/gif"
            | "image/avif"
            | "audio/wav"
            | "audio/mp3"
            | "audio/aiff"
            | "audio/aac"
            | "audio/ogg"
            | "audio/flac"
            | "audio/mpeg"
            | "audio/m4a"
            | "audio/l16"
            | "audio/opus"
            | "audio/alaw"
            | "audio/mulaw"
            | "audio/webm"
            | "audio/pcm"
            | "video/audio/s16le"
            | "video/audio/wav"
            | "video/mp4"
            | "video/mpeg"
            | "video/quicktime"
            | "video/avi"
            | "video/x-flv"
            | "video/mpg"
            | "video/webm"
            | "video/wmv"
            | "video/3gpp"
            | "video/text/timestamp"
            | "application/pdf"
            | "application/rtf"
    ) || is_text_mime(&base)
        || (base == "video/*"
            && url
                .and_then(|url| reqwest::Url::parse(url).ok())
                .is_some_and(|url| {
                    matches!(
                        url.host_str(),
                        Some("youtube.com" | "www.youtube.com" | "m.youtube.com" | "youtu.be")
                    )
                }));
    if supported {
        Ok(())
    } else {
        Err(format!(
            "Gemini does not support media MIME type {mime}. Supply a documented format without changing the actual bytes."
        ))
    }
}

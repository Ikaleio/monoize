use super::*;
use axum::extract::{FromRequest, Multipart, Request};
use base64::Engine as _;

pub(super) struct ParsedImageEdit {
    pub(super) fields: Map<String, Value>,
    pub(super) images: Vec<urp::Node>,
    pub(super) mask: Option<urp::Node>,
    pub(super) markup: Option<urp::Node>,
    pub(super) capture_raw_input: Arc<Value>,
}

pub(super) fn image_request_error(message: impl Into<String>) -> AppError {
    AppError::new(StatusCode::BAD_REQUEST, "invalid_request", message)
}

pub(super) fn required_image_text(fields: &Map<String, Value>, key: &str) -> AppResult<String> {
    let text = fields
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| image_request_error(format!("missing or empty {key}")).with_param(key))?;
    if key == "prompt" && text.chars().count() > 32_000 {
        return Err(
            image_request_error("prompt must contain at most 32000 characters")
                .with_param("prompt"),
        );
    }
    Ok(text.to_owned())
}

fn validate_edit_image_count(count: usize) -> AppResult<()> {
    if (1..=16).contains(&count) {
        Ok(())
    } else {
        Err(image_request_error(
            "image edits require 1 through 16 source images",
        ))
    }
}

pub(super) async fn parse_edit_request(
    state: &AppState,
    headers: &HeaderMap,
    request: Request,
    capture_eligible: bool,
) -> AppResult<ParsedImageEdit> {
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if content_type == "application/json"
        || content_type.starts_with("application/") && content_type.ends_with("+json")
    {
        let crate::payload::JsonBody(body) =
            crate::payload::JsonBody::from_request(request, state).await?;
        parse_json_image_edit(body, capture_eligible)
    } else if content_type == "multipart/form-data" {
        let multipart = Multipart::from_request(request, state)
            .await
            .map_err(|err| AppError::new(err.status(), "invalid_request", err.body_text()))?;
        parse_multipart_image_edit(multipart, capture_eligible).await
    } else {
        Err(AppError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "invalid_request",
            "image edits require application/json or multipart/form-data",
        ))
    }
}

pub(super) fn parse_edit_options(
    edit: &ParsedImageEdit,
    stream: bool,
) -> AppResult<ImageRequestOptions> {
    let options = ImageRequestOptions::parse(build_extra_body(
        &edit.fields,
        &[
            "prompt",
            "model",
            "n",
            "stream",
            "max_multiplier",
            "image",
            "image[]",
            "images",
            "mask",
            "markup",
        ],
    ))?;
    options.validate(true, stream)?;
    if edit.markup.is_some()
        && (edit.mask.is_some()
            || options
                .extra_body
                .get("prompt_rewriter")
                .is_some_and(|v| !v.is_null()))
    {
        return Err(
            image_request_error("markup cannot be combined with mask or prompt_rewriter")
                .with_param("markup"),
        );
    }
    Ok(options)
}

fn parse_json_image_edit(body: Value, capture_eligible: bool) -> AppResult<ParsedImageEdit> {
    let fields = body
        .as_object()
        .ok_or_else(|| image_request_error("body must be object"))?;
    let references = fields
        .get("images")
        .and_then(Value::as_array)
        .ok_or_else(|| image_request_error("images must be an array"))?;
    validate_edit_image_count(references.len())?;
    let images = references
        .iter()
        .map(|reference| parse_image_reference(reference, "image"))
        .collect::<AppResult<Vec<_>>>()?;
    let mask = fields
        .get("mask")
        .filter(|value| !value.is_null())
        .map(|reference| parse_image_reference(reference, "mask"))
        .transpose()?;
    let markup = fields
        .get("markup")
        .filter(|v| !v.is_null())
        .map(|reference| parse_image_reference(reference, "markup"))
        .transpose()?;
    Ok(ParsedImageEdit {
        fields: fields.clone(),
        images,
        mask,
        markup,
        capture_raw_input: Arc::new(if capture_eligible { body } else { Value::Null }),
    })
}

fn parse_image_reference(value: &Value, field: &str) -> AppResult<urp::Node> {
    let reference = value
        .as_object()
        .ok_or_else(|| image_request_error("each image reference must be an object"))?;
    let url = reference.get("image_url").filter(|value| !value.is_null());
    let file_id = reference.get("file_id").filter(|value| !value.is_null());
    let mut resource = None;
    let source = match (url, file_id) {
        (Some(Value::String(url)), None) if !url.trim().is_empty() => {
            if url.starts_with("data:") {
                let (media_type, data) = urp::media::parse_data_url(url).ok_or_else(|| {
                    image_request_error("image_url must contain a Base64 image data URL")
                })?;
                if field == "markup" && urp::media::mime_essence(media_type) != "image/png" {
                    return Err(image_request_error("markup must be PNG").with_param("markup"));
                }
                if !media_type.starts_with("image/")
                    || data.is_empty()
                    || urp::media::validate_base64(data).is_err()
                {
                    return Err(image_request_error(
                        "image_url must contain a Base64 image data URL",
                    ));
                }
                urp::ImageSource::Base64 {
                    media_type: media_type.to_owned(),
                    data: data.to_owned(),
                }
            } else {
                let parsed = reqwest::Url::parse(url).map_err(|_| {
                    image_request_error("image_url must be an HTTP(S) URL or a Base64 data URL")
                })?;
                if field == "markup" && parsed.scheme() != "https" {
                    return Err(
                        image_request_error("markup URL must use HTTPS").with_param("markup")
                    );
                }
                if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
                    return Err(image_request_error(
                        "image_url must be an HTTP(S) URL or a Base64 data URL",
                    ));
                }
                urp::ImageSource::Url {
                    url: url.clone(),
                    detail: None,
                }
            }
        }
        (None, Some(Value::String(file_id))) if !file_id.trim().is_empty() => {
            resource = Some(urp::MediaResource {
                protocol: urp::ProviderProtocol::OpenaiImage,
                provider_id: None,
                channel_id: None,
                credential_scope: None,
            });
            urp::ImageSource::FileId {
                file_id: file_id.clone(),
                detail: None,
            }
        }
        _ => {
            return Err(image_request_error(
                "each image reference requires exactly one non-empty image_url or file_id",
            ));
        }
    };
    Ok(urp::Node::Image {
        id: None,
        role: urp::OrdinaryRole::User,
        source,
        metadata: urp::MediaMetadata {
            image_mask: field == "mask",
            image_markup: field == "markup",
            resource,
            ..Default::default()
        },
        extra_body: HashMap::new(),
    })
}

async fn parse_multipart_image_edit(
    mut multipart: Multipart,
    capture_eligible: bool,
) -> AppResult<ParsedImageEdit> {
    let mut fields = Map::new();
    let mut images = Vec::new();
    let mut mask = None;
    let mut markup = None;
    let mut capture_parts = Vec::new();
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|err| image_request_error(err.to_string()))?
    {
        let name = field.name().unwrap_or("").to_owned();
        if matches!(name.as_str(), "image" | "image[]" | "mask" | "markup") {
            let image_mask = name == "mask";
            let image_markup = name == "markup";
            if image_markup && markup.is_some() {
                return Err(image_request_error("image edits accept at most one markup"));
            }
            if image_mask && mask.is_some() {
                return Err(image_request_error("image edits accept at most one mask"));
            }
            if !image_mask && !image_markup && images.len() >= 16 {
                return Err(image_request_error(
                    "image edits require 1 through 16 source images",
                ));
            }
            let filename = field.file_name().map(str::to_owned);
            let content_type = field.content_type().map(str::to_owned);
            let media_type = content_type
                .clone()
                .unwrap_or_else(|| infer_media_type_from_filename(filename.as_deref()));
            if image_markup && urp::media::mime_essence(&media_type) != "image/png" {
                return Err(image_request_error("markup must be PNG").with_param("markup"));
            }
            let (data, byte_length) = read_multipart_asset(&mut field).await?;
            if capture_eligible {
                capture_parts.push(crate::request_capture::CapturedMultipartPart::File {
                    name,
                    filename: filename.clone(),
                    content_type,
                    data_base64: data.clone(),
                    byte_length,
                });
            }
            let image = urp::Node::Image {
                id: None,
                role: urp::OrdinaryRole::User,
                source: urp::ImageSource::Base64 { media_type, data },
                metadata: urp::MediaMetadata {
                    image_mask,
                    image_markup,
                    filename,
                    ..Default::default()
                },
                extra_body: HashMap::new(),
            };
            if image_mask {
                mask = Some(image);
            } else if image_markup {
                markup = Some(image);
            } else {
                images.push(image);
            }
        } else if field.file_name().is_none() {
            let mut bytes = Vec::new();
            while let Some(chunk) = field
                .chunk()
                .await
                .map_err(|error| image_request_error(error.to_string()))?
            {
                if chunk.len() > crate::payload::CHUNK.saturating_sub(bytes.len()) {
                    return Err(
                        image_request_error("multipart text field exceeds 65536 bytes")
                            .with_param(&name),
                    );
                }
                bytes.extend_from_slice(&chunk);
            }
            let text = String::from_utf8_lossy(&bytes).into_owned();
            if capture_eligible {
                capture_parts.push(crate::request_capture::CapturedMultipartPart::Text {
                    name: name.clone(),
                    text: text.clone(),
                });
            }
            let value = match name.as_str() {
                "prompt" | "model" | "user" | "safety_identifier" | "max_multiplier" => {
                    Value::String(text)
                }
                "n" if text == "1" => Value::from(1),
                "n" => return Err(image_request_error("n must equal 1").with_param("n")),
                "stream" => match text.as_str() {
                    "true" => Value::Bool(true),
                    "false" => Value::Bool(false),
                    _ => return Err(image_request_error("stream must be true or false")),
                },
                _ => coerce_text_to_json_value(&text),
            };
            fields.insert(name, value);
        }
    }
    validate_edit_image_count(images.len())?;
    Ok(ParsedImageEdit {
        fields,
        images,
        mask,
        markup,
        capture_raw_input: Arc::new(if capture_eligible {
            crate::request_capture::multipart_capture_object(&capture_parts)
        } else {
            Value::Null
        }),
    })
}

pub(super) struct ImageRequestOptions {
    pub(super) generation: urp::ImageGenerationOptions,
    pub(super) user: Option<String>,
    pub(super) extra_body: HashMap<String, Value>,
}

impl ImageRequestOptions {
    pub(super) fn validate(&self, edit: bool, stream: bool) -> AppResult<()> {
        self.generation
            .validate_image_request(
                edit,
                stream,
                self.extra_body
                    .get("prompt_rewriter")
                    .is_some_and(|v| !v.is_null()),
            )
            .map_err(image_request_error)?;
        if let Some(value) = self
            .extra_body
            .get("safety_identifier")
            .filter(|v| !v.is_null())
            && !value.as_str().is_some_and(|id| id.chars().count() <= 128)
        {
            return Err(image_request_error(
                "safety_identifier must be a string of at most 128 characters",
            )
            .with_param("safety_identifier"));
        }
        Ok(())
    }

    pub(super) fn parse(mut extra_body: HashMap<String, Value>) -> AppResult<Self> {
        let generation = urp::ImageGenerationOptions::take_from_extra(&mut extra_body)
            .map_err(image_request_error)?;
        let user = match extra_body.remove("user") {
            None | Some(Value::Null) => None,
            Some(Value::String(user)) => Some(user),
            Some(_) => return Err(image_request_error("user must be a string or null")),
        };
        Ok(Self {
            generation,
            user,
            extra_body,
        })
    }
}

pub(super) fn parse_image_stream(value: Option<&Value>) -> AppResult<bool> {
    match value {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(flag)) => Ok(*flag),
        Some(_) => Err(image_request_error("stream must be a boolean or null")),
    }
}

pub(super) fn validate_image_count(value: Option<&Value>) -> AppResult<()> {
    match value {
        None | Some(Value::Null) => Ok(()),
        Some(value) if value.as_u64() == Some(1) => Ok(()),
        _ => Err(image_request_error("n must equal 1").with_param("n")),
    }
}

pub(super) fn build_extra_body(
    obj: &Map<String, Value>,
    exclude: &[&str],
) -> HashMap<String, Value> {
    let exclude_set: std::collections::HashSet<&str> = exclude.iter().copied().collect();
    obj.iter()
        .filter(|(k, _)| !exclude_set.contains(k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

pub(super) fn resolve_image_max_multiplier(
    body_value: Option<&Value>,
    headers: &HeaderMap,
    auth: &crate::auth::AuthResult,
) -> Option<Multiplier> {
    let ceiling = auth.max_multiplier;
    let requested = body_value
        .and_then(Value::as_str)
        .and_then(parse_positive_multiplier)
        .or_else(|| parse_max_multiplier_header(headers));

    match (ceiling, requested) {
        (Some(c), Some(r)) => Some(r.min(c)),
        (Some(c), None) => Some(c),
        (None, Some(r)) => Some(r),
        (None, None) => None,
    }
}

fn infer_media_type_from_filename(filename: Option<&str>) -> String {
    let ext = filename
        .and_then(|f| f.rsplit('.').next())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
    .to_string()
}

fn coerce_text_to_json_value(text: &str) -> Value {
    if let Ok(n) = text.parse::<i64>() {
        return Value::Number(n.into());
    }
    if let Ok(n) = text.parse::<f64>()
        && n.is_finite()
        && let Some(num) = serde_json::Number::from_f64(n)
    {
        return Value::Number(num);
    }
    match text {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ => Value::String(text.to_string()),
    }
}

async fn read_multipart_asset(
    field: &mut axum::extract::multipart::Field<'_>,
) -> AppResult<(String, usize)> {
    use std::io::Write;
    let mut inline = Vec::new();
    let mut encoder = None;
    let mut len = 0usize;
    while let Some(chunk) = field
        .chunk()
        .await
        .map_err(|error| image_request_error(error.to_string()))?
    {
        len = len.saturating_add(chunk.len());
        if len > crate::payload::CHUNK / 4 * 3 && encoder.is_none() {
            let spool = crate::payload::SpoolWriter::new().map_err(spool_failure)?;
            let mut writer = base64::write::EncoderWriter::new(
                spool,
                &base64::engine::general_purpose::STANDARD,
            );
            writer.write_all(&inline).map_err(spool_failure)?;
            inline.clear();
            encoder = Some(writer);
        }
        match &mut encoder {
            Some(writer) => writer.write_all(&chunk).map_err(spool_failure)?,
            None => inline.extend_from_slice(&chunk),
        }
    }
    match encoder {
        Some(mut writer) => {
            let payload = crate::payload::current()
                .ok_or_else(|| spool_failure(std::io::Error::other("missing payload scope")))?;
            let spool = writer
                .finish()
                .and_then(|spool| spool.finish())
                .map_err(spool_failure)?;
            Ok((payload.insert(spool), len))
        }
        None => Ok((
            base64::engine::general_purpose::STANDARD.encode(inline),
            len,
        )),
    }
}

fn spool_failure(error: std::io::Error) -> AppError {
    AppError::new(
        StatusCode::INSUFFICIENT_STORAGE,
        "payload_spool_failed",
        error.to_string(),
    )
}

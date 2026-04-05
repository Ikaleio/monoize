use crate::urp::encode::merge_extra;
use crate::urp::{
    FinishReason, ImageSource, Item, Part, Role, ToolResultContent, UrpRequest, UrpResponse,
};
use serde_json::{json, Map, Value};

pub fn encode_request(req: &UrpRequest, upstream_model: &str) -> Value {
    let mut input = Map::new();
    let mut prompt_parts: Vec<String> = Vec::new();

    for item in &req.inputs {
        match item {
            Item::Message { role, parts, .. } => match role {
                Role::System | Role::Developer => {
                    let text = collect_text(parts);
                    if !text.is_empty() {
                        input
                            .entry("system_prompt".to_string())
                            .or_insert_with(|| Value::String(String::new()));
                        if let Some(Value::String(existing)) = input.get_mut("system_prompt") {
                            if !existing.is_empty() {
                                existing.push('\n');
                            }
                            existing.push_str(&text);
                        }
                    }
                }
                Role::User => {
                    for part in parts {
                        match part {
                            Part::Text { content, .. } => {
                                prompt_parts.push(content.clone());
                            }
                            Part::Image { source, .. } => match source {
                                ImageSource::Url { url, .. } => {
                                    input.insert("image".to_string(), Value::String(url.clone()));
                                }
                                ImageSource::Base64 { media_type, data } => {
                                    let data_url = format!("data:{media_type};base64,{data}");
                                    input.insert("image".to_string(), Value::String(data_url));
                                }
                            },
                            Part::File { source, .. } => {
                                let url = match source {
                                    crate::urp::FileSource::Url { url } => url.clone(),
                                    crate::urp::FileSource::Base64 {
                                        media_type, data, ..
                                    } => {
                                        format!("data:{media_type};base64,{data}")
                                    }
                                };
                                input.insert("file".to_string(), Value::String(url));
                            }
                            Part::Audio { source, .. } => {
                                let url = match source {
                                    crate::urp::AudioSource::Url { url } => url.clone(),
                                    crate::urp::AudioSource::Base64 { media_type, data } => {
                                        format!("data:{media_type};base64,{data}")
                                    }
                                };
                                input.insert("audio".to_string(), Value::String(url));
                            }
                            _ => {}
                        }
                    }
                }
                Role::Assistant => {
                    let text = collect_text(parts);
                    if !text.is_empty() {
                        prompt_parts.push(text);
                    }
                }
                Role::Tool => {}
            },
            Item::ToolResult { content, .. } => {
                for c in content {
                    if let ToolResultContent::Text { text } = c {
                        prompt_parts.push(text.clone());
                    }
                }
            }
        }
    }

    if !prompt_parts.is_empty() {
        input.insert("prompt".to_string(), Value::String(prompt_parts.join("\n")));
    }

    if let Some(max_tokens) = req.max_output_tokens {
        input.insert("max_tokens".to_string(), Value::from(max_tokens));
        input.insert("max_new_tokens".to_string(), Value::from(max_tokens));
    }
    if let Some(temp) = req.temperature {
        input.insert("temperature".to_string(), Value::from(temp));
    }
    if let Some(top_p) = req.top_p {
        input.insert("top_p".to_string(), Value::from(top_p));
    }

    let mut body = Map::new();

    if upstream_model.contains(':') && !upstream_model.starts_with("deployment:") {
        body.insert(
            "version".to_string(),
            Value::String(upstream_model.to_string()),
        );
    }

    body.insert("input".to_string(), Value::Object(input));

    if let Some(true) = req.stream {
        body.insert("stream".to_string(), Value::Bool(true));
    }

    merge_extra(&mut body, &req.extra_body);
    body.remove("model");
    body.remove("max_multiplier");

    Value::Object(body)
}

fn collect_text(parts: &[Part]) -> String {
    let mut out = String::new();
    for p in parts {
        if let Part::Text { content, .. } = p {
            out.push_str(content);
        }
    }
    out
}

pub fn encode_response(resp: &UrpResponse, logical_model: &str) -> Value {
    let mut text_parts: Vec<String> = Vec::new();
    let mut image_urls: Vec<String> = Vec::new();

    for item in &resp.outputs {
        if let Item::Message { parts, .. } = item {
            for part in parts {
                match part {
                    Part::Text { content, .. } => text_parts.push(content.clone()),
                    Part::Image { source, .. } => match source {
                        ImageSource::Url { url, .. } => image_urls.push(url.clone()),
                        ImageSource::Base64 { media_type, data } => {
                            image_urls.push(format!("data:{media_type};base64,{data}"));
                        }
                    },
                    _ => {}
                }
            }
        }
    }

    let output: Value = if !image_urls.is_empty() {
        if image_urls.len() == 1 {
            Value::String(image_urls.into_iter().next().unwrap())
        } else {
            Value::Array(image_urls.into_iter().map(Value::String).collect())
        }
    } else {
        let combined = text_parts.join("");
        Value::String(combined)
    };

    let status = match resp.finish_reason {
        Some(FinishReason::Stop) | None => "succeeded",
        Some(FinishReason::Length) => "succeeded",
        Some(FinishReason::ContentFilter) => "failed",
        _ => "succeeded",
    };

    let mut body = json!({
        "id": resp.id,
        "model": logical_model,
        "status": status,
        "output": output,
    });

    if let Some(usage) = &resp.usage {
        body["metrics"] = json!({
            "input_token_count": usage.input_tokens,
            "output_token_count": usage.output_tokens,
        });
    }

    if let Some(obj) = body.as_object_mut() {
        merge_extra(obj, &resp.extra_body);
    }

    body
}

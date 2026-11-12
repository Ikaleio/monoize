use crate::urp::grouping::{Envelope, envelopes};
use crate::urp::{
    AudioSource, FileSource, ImageSource, Node, OrdinaryRole, ToolResultContent, UrpRequest,
    is_internal_key,
};
use serde_json::{Map, Value};

pub fn encode_request(req: &UrpRequest, upstream_model: &str) -> Value {
    let mut input = Map::new();
    let mut prompt_parts: Vec<String> = Vec::new();
    let mut images: Vec<Value> = Vec::new();

    for envelope in envelopes(&req.input) {
        match envelope {
            Envelope::Message { role, nodes, .. } => match role {
                OrdinaryRole::System | OrdinaryRole::Developer => {
                    let text = collect_text(nodes);
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
                OrdinaryRole::User => {
                    for node in nodes {
                        match node {
                            Node::Text { content, .. } => {
                                prompt_parts.push(content.clone());
                            }
                            Node::Image { source, .. } => {
                                if let Some(image) = image_source_to_value(source) {
                                    images.push(image);
                                }
                            }
                            Node::File { source, .. } => {
                                let url = match source {
                                    FileSource::Url { url } => url.clone(),
                                    FileSource::Base64 {
                                        media_type, data, ..
                                    } => format!("data:{media_type};base64,{data}"),
                                    FileSource::FileId { .. }
                                    | FileSource::Text { .. }
                                    | FileSource::Content { .. } => continue,
                                };
                                input.insert("file".to_string(), Value::String(url));
                            }
                            Node::Audio { source, .. } => {
                                let url = match source {
                                    AudioSource::Url { url } => url.clone(),
                                    AudioSource::Base64 { media_type, data } => {
                                        format!("data:{media_type};base64,{data}")
                                    }
                                };
                                input.insert("audio".to_string(), Value::String(url));
                            }
                            _ => {}
                        }
                    }
                }
                OrdinaryRole::Assistant => {
                    let text = collect_text(nodes);
                    if !text.is_empty() {
                        prompt_parts.push(text);
                    }
                }
            },
            Envelope::ToolResult { node, .. } => {
                if let Node::ToolResult { content, .. } = node {
                    for c in content {
                        if let ToolResultContent::Text { text, .. } = c {
                            prompt_parts.push(text.clone());
                        }
                    }
                }
            }
        }
    }

    if !prompt_parts.is_empty() {
        input.insert("prompt".to_string(), Value::String(prompt_parts.join("\n")));
    }

    if !images.is_empty() {
        let arr = Value::Array(images.clone());
        if images.len() == 1 {
            input
                .entry("image".to_string())
                .or_insert_with(|| images[0].clone());
            input
                .entry("input_image".to_string())
                .or_insert_with(|| images[0].clone());
        }
        input
            .entry("image_input".to_string())
            .or_insert_with(|| arr.clone());
        input
            .entry("input_images".to_string())
            .or_insert_with(|| arr.clone());
        input
            .entry("reference_images".to_string())
            .or_insert_with(|| arr);
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

    // extra_body fields that are objects named "input" merge into the input map;
    // all other extra_body fields also go into input (Replicate model params are
    // model-specific and live inside the input object).
    for (k, v) in &req.extra_body {
        if is_internal_key(k) {
            continue;
        }
        match k.as_str() {
            "model"
            | "max_multiplier"
            | "stream"
            | "version"
            | "webhook"
            | "webhook_events_filter" => {
                // these are top-level Replicate fields, not input fields
            }
            "input" => {
                if let Some(obj) = v.as_object() {
                    for (ik, iv) in obj {
                        if !is_internal_key(ik) {
                            input.entry(ik.clone()).or_insert_with(|| iv.clone());
                        }
                    }
                }
            }
            _ => {
                input.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
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

    // Only merge top-level Replicate fields from extra_body
    for (k, v) in &req.extra_body {
        if is_internal_key(k) {
            continue;
        }
        match k.as_str() {
            "webhook" | "webhook_events_filter" => {
                body.entry(k.clone()).or_insert_with(|| v.clone());
            }
            _ => {}
        }
    }

    body.remove("model");
    body.remove("max_multiplier");

    Value::Object(body)
}

fn image_source_to_value(source: &ImageSource) -> Option<Value> {
    match source {
        ImageSource::Url { url, .. } => Some(Value::String(url.clone())),
        ImageSource::Base64 { media_type, data } => {
            Some(Value::String(format!("data:{media_type};base64,{data}")))
        }
        ImageSource::FileId { .. } => None,
    }
}

fn collect_text(nodes: &[Node]) -> String {
    let mut out = String::new();
    for node in nodes {
        if let Node::Text { content, .. } = node {
            out.push_str(content);
        }
    }
    out
}

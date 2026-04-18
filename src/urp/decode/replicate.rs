use crate::urp::decode::split_extra;
use crate::urp::{
    FinishReason, ImageSource, Node, OrdinaryRole, UrpRequest, UrpResponse, Usage,
};
use serde_json::{Map, Value};
use std::collections::HashMap;

pub fn decode_request(value: &Value) -> Result<UrpRequest, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "replicate request must be object".to_string())?;

    let model = obj
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let mut input_nodes = Vec::new();

    let input = obj.get("input").and_then(|v| v.as_object());

    if let Some(input_obj) = input {
        if let Some(system_prompt) = input_obj.get("system_prompt").and_then(|v| v.as_str()) {
            if !system_prompt.is_empty() {
                input_nodes.push(Node::Text {
                    id: None,
                    role: OrdinaryRole::System,
                    content: system_prompt.to_string(),
                    phase: None,
                    extra_body: HashMap::new(),
                });
            }
        }

        if let Some(prompt) = input_obj.get("prompt").and_then(|v| v.as_str()) {
            if !prompt.is_empty() {
                input_nodes.push(Node::Text {
                    id: None,
                    role: OrdinaryRole::User,
                    content: prompt.to_string(),
                    phase: None,
                    extra_body: HashMap::new(),
                });
            }
        }

        if let Some(image_url) = input_obj.get("image").and_then(|v| v.as_str()) {
            input_nodes.push(Node::Image {
                id: None,
                role: OrdinaryRole::User,
                source: ImageSource::Url {
                    url: image_url.to_string(),
                    detail: None,
                },
                extra_body: HashMap::new(),
            });
        }
    }

    let max_tokens = input.and_then(|i| {
        i.get("max_tokens")
            .or_else(|| i.get("max_new_tokens"))
            .and_then(|v| v.as_u64())
    });

    let temperature = input.and_then(|i| i.get("temperature").and_then(|v| v.as_f64()));
    let top_p = input.and_then(|i| i.get("top_p").and_then(|v| v.as_f64()));

    let stream = obj.get("stream").and_then(|v| v.as_bool());

    Ok(UrpRequest {
        model,
        input: input_nodes,
        stream,
        temperature,
        top_p,
        max_output_tokens: max_tokens,
        reasoning: None,
        tools: None,
        tool_choice: None,
        response_format: None,
        user: None,
        extra_body: split_extra(obj, &["model", "input", "stream", "version"]),
    })
}

pub fn decode_response(value: &Value) -> Result<UrpResponse, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "replicate response must be object".to_string())?;

    let id = obj
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("replicate_response")
        .to_string();

    let model = obj
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let status = obj.get("status").and_then(|v| v.as_str()).unwrap_or("");

    let finish_reason = match status {
        "succeeded" => Some(FinishReason::Stop),
        "failed" | "canceled" | "aborted" => Some(FinishReason::Other),
        _ => None,
    };

    let mut output_nodes = Vec::new();

    if let Some(output) = obj.get("output") {
        parse_output_into_nodes(output, &mut output_nodes);
    }

    if let Some(error) = obj.get("error").and_then(|v| v.as_str()) {
        if !error.is_empty() && output_nodes.is_empty() {
            output_nodes.push(Node::Refusal {
                id: None,
                content: error.to_string(),
                extra_body: HashMap::new(),
            });
        }
    }

    let usage = parse_replicate_usage(obj);

    Ok(UrpResponse {
        id,
        model,
        created_at: None,
        output: output_nodes,
        finish_reason,
        usage,
        extra_body: split_extra(
            obj,
            &[
                "id", "model", "status", "output", "error", "metrics", "input", "version",
            ],
        ),
    })
}

fn parse_output_into_nodes(output: &Value, nodes: &mut Vec<Node>) {
    match output {
        Value::String(s) => {
            if looks_like_url(s) && looks_like_media_url(s) {
                nodes.push(Node::Image {
                    id: None,
                    role: OrdinaryRole::Assistant,
                    source: ImageSource::Url {
                        url: s.clone(),
                        detail: None,
                    },
                    extra_body: HashMap::new(),
                });
            } else {
                nodes.push(Node::Text {
                    id: None,
                    role: OrdinaryRole::Assistant,
                    content: s.clone(),
                    phase: None,
                    extra_body: HashMap::new(),
                });
            }
        }
        Value::Array(arr) => {
            let all_strings = arr.iter().all(|v| v.is_string());
            if all_strings {
                let all_urls = arr
                    .iter()
                    .filter_map(|v| v.as_str())
                    .all(|s| looks_like_url(s) && looks_like_media_url(s));
                if all_urls {
                    for v in arr {
                        if let Some(url) = v.as_str() {
                            nodes.push(Node::Image {
                                id: None,
                                role: OrdinaryRole::Assistant,
                                source: ImageSource::Url {
                                    url: url.to_string(),
                                    detail: None,
                                },
                                extra_body: HashMap::new(),
                            });
                        }
                    }
                } else {
                    let combined: String = arr
                        .iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join("");
                    if !combined.is_empty() {
                        nodes.push(Node::Text {
                            id: None,
                            role: OrdinaryRole::Assistant,
                            content: combined,
                            phase: None,
                            extra_body: HashMap::new(),
                        });
                    }
                }
            } else {
                let serialized = serde_json::to_string(output).unwrap_or_default();
                if !serialized.is_empty() {
                    nodes.push(Node::Text {
                        id: None,
                        role: OrdinaryRole::Assistant,
                        content: serialized,
                        phase: None,
                        extra_body: HashMap::new(),
                    });
                }
            }
        }
        Value::Null => {}
        other => {
            let serialized = serde_json::to_string(other).unwrap_or_default();
            if !serialized.is_empty() {
                nodes.push(Node::Text {
                    id: None,
                    role: OrdinaryRole::Assistant,
                    content: serialized,
                    phase: None,
                    extra_body: HashMap::new(),
                });
            }
        }
    }
}

fn looks_like_url(s: &str) -> bool {
    s.starts_with("https://") || s.starts_with("http://")
}

fn looks_like_media_url(s: &str) -> bool {
    let lower = s.to_lowercase();
    lower.contains(".png")
        || lower.contains(".jpg")
        || lower.contains(".jpeg")
        || lower.contains(".webp")
        || lower.contains(".gif")
        || lower.contains(".mp4")
        || lower.contains(".mp3")
        || lower.contains(".wav")
        || lower.contains("replicate.delivery")
}

fn parse_replicate_usage(obj: &Map<String, Value>) -> Option<Usage> {
    let metrics = obj.get("metrics").and_then(|v| v.as_object())?;
    let input_tokens = metrics
        .get("input_token_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output_tokens = metrics
        .get("output_token_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if input_tokens == 0 && output_tokens == 0 {
        return None;
    }
    Some(Usage {
        input_tokens,
        output_tokens,
        input_details: None,
        output_details: None,
        extra_body: HashMap::new(),
    })
}

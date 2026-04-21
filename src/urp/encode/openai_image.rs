use crate::urp::{Node, UrpRequest};
use serde_json::{Map, Value};

pub fn encode_request(req: &UrpRequest, model: &str) -> Value {
    let mut prompt_parts: Vec<String> = Vec::new();
    for item in &req.input {
        if let Node::Text {
            role: crate::urp::OrdinaryRole::User,
            content,
            ..
        } = item
            && !content.trim().is_empty()
        {
            prompt_parts.push(content.clone());
        }
    }
    let prompt = prompt_parts.join("\n");

    let mut body = Map::new();
    body.insert("model".to_string(), Value::String(model.to_string()));
    body.insert("prompt".to_string(), Value::String(prompt));

    for (k, v) in &req.extra_body {
        if k != "model" && k != "prompt" && k != "stream" {
            body.insert(k.clone(), v.clone());
        }
    }

    Value::Object(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::urp::{Node, OrdinaryRole};
    use serde_json::json;
    use std::collections::HashMap;

    fn empty_map() -> HashMap<String, Value> {
        HashMap::new()
    }

    #[test]
    fn encodes_user_text_parts_into_prompt_and_preserves_allowed_extra_fields() {
        let req = UrpRequest {
            model: "logical-model".to_string(),
            input: vec![
                Node::Text {
                    id: None,
                    role: OrdinaryRole::System,
                    content: "ignore system".to_string(),
                    phase: None,
                    extra_body: empty_map(),
                },
                Node::Text {
                    id: None,
                    role: OrdinaryRole::User,
                    content: "draw a cat".to_string(),
                    phase: None,
                    extra_body: empty_map(),
                },
                Node::Text {
                    id: None,
                    role: OrdinaryRole::User,
                    content: "  ".to_string(),
                    phase: None,
                    extra_body: empty_map(),
                },
                Node::Text {
                    id: None,
                    role: OrdinaryRole::Assistant,
                    content: "ignore assistant".to_string(),
                    phase: None,
                    extra_body: empty_map(),
                },
                Node::Text {
                    id: None,
                    role: OrdinaryRole::User,
                    content: "in watercolor".to_string(),
                    phase: None,
                    extra_body: empty_map(),
                },
            ],
            stream: Some(true),
            temperature: Some(0.3),
            top_p: Some(0.9),
            max_output_tokens: Some(100),
            reasoning: None,
            tools: None,
            tool_choice: None,
            response_format: None,
            user: None,
            extra_body: HashMap::from([
                ("size".to_string(), json!("1024x1024")),
                ("n".to_string(), json!(2)),
                ("stream".to_string(), json!(true)),
                ("prompt".to_string(), json!("override")),
                ("model".to_string(), json!("override-model")),
            ]),
        };

        let encoded = encode_request(&req, "gpt-image-1");
        assert_eq!(encoded["model"], json!("gpt-image-1"));
        assert_eq!(encoded["prompt"], json!("draw a cat\nin watercolor"));
        assert_eq!(encoded["size"], json!("1024x1024"));
        assert_eq!(encoded["n"], json!(2));
        assert!(encoded.get("stream").is_none());
    }
}

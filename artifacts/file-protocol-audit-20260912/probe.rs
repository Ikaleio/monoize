use monoize::urp::{self, ProviderProtocol, UrpRequest, UrpResponse};
use serde_json::{Value, json};
use std::io::{self, BufRead};

fn decode_request(source: &str, value: &Value) -> Result<UrpRequest, String> {
    match source {
        "chat" => urp::decode::openai_chat::decode_request(value),
        "responses" => urp::decode::openai_responses::decode_request(value),
        "messages" => urp::decode::anthropic::decode_request(value),
        "gemini" => urp::decode::gemini::decode_request(value),
        "urp" => return serde_json::from_value(value.clone()).map_err(|error| error.to_string()),
        _ => return Err("unknown source".into()),
    }.map_err(|error| format!("{error:?}"))
}

fn decode_response(source: &str, value: &Value) -> Result<UrpResponse, String> {
    match source {
        "chat" => urp::decode::openai_chat::decode_response(value),
        "responses" => urp::decode::openai_responses::decode_response(value),
        "messages" => urp::decode::anthropic::decode_response(value),
        "gemini" => urp::decode::gemini::decode_response(value),
        "urp" => return serde_json::from_value(value.clone()).map_err(|error| error.to_string()),
        _ => return Err("unknown source".into()),
    }.map_err(|error| format!("{error:?}"))
}

fn main() {
    for line in io::stdin().lock().lines() {
        let input: Value = serde_json::from_str(&line.unwrap()).unwrap();
        let source = input["source"].as_str().unwrap();
        if input["mode"] == "response" {
            match decode_response(source, &input["response"]) {
                Ok(response) => println!("{}", json!({"id":input["id"], "canonical":response,
                    "encoded":{
                        "chat":urp::encode::openai_chat::encode_response(&response,"audit-model"),
                        "responses":urp::encode::openai_responses::encode_response(&response,"audit-model"),
                        "messages":urp::encode::anthropic::encode_response(&response,"audit-model"),
                        "gemini":urp::encode::gemini::encode_response(&response,"audit-model")
                    }})),
                Err(error) => println!("{}", json!({"id":input["id"],"error":error})),
            }
            continue;
        }
        match decode_request(source, &input["request"]) {
            Ok(request) => {
                let mut encoded = serde_json::Map::new();
                for (target, protocol) in [
                    ("chat",ProviderProtocol::ChatCompletion),
                    ("responses",ProviderProtocol::Responses),
                    ("messages",ProviderProtocol::Messages),
                    ("gemini",ProviderProtocol::Gemini)
                ] {
                    let mut attempt = request.clone();
                    if input["strip"] == true {
                        urp::retain_provider_items_for_protocol(&mut attempt.input, protocol);
                        urp::strip_nested_extra_body(&mut attempt.input);
                    }
                    let value = match target {
                        "chat" => urp::encode::openai_chat::encode_request(&attempt, "audit-model"),
                        "responses" => urp::encode::openai_responses::encode_request(&attempt, "audit-model"),
                        "messages" => match urp::encode::anthropic::encode_request_checked(&attempt, "audit-model") {
                            Ok(value) => value,
                            Err(error) => json!({"encode_error":error})
                        },
                        _ => urp::encode::gemini::encode_request(&attempt, "audit-model"),
                    };
                    encoded.insert(target.into(),value);
                }
                println!("{}",json!({"id":input["id"],"canonical":request,"encoded":encoded}));
            }
            Err(error) => println!("{}",json!({"id":input["id"],"error":error})),
        }
    }
}

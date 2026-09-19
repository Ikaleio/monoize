use super::ProviderProtocol;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

const OWNED_FIELDS: &[&str] = &[
    "type",
    "url",
    "uri",
    "title",
    "start_index",
    "end_index",
    "startIndex",
    "endIndex",
    "cited_text",
    "document_index",
    "document_title",
    "start_char_index",
    "end_char_index",
    "start_page_number",
    "end_page_number",
    "start_block_index",
    "end_block_index",
    "file_id",
    "filename",
    "index",
    "container_id",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Citation {
    pub source: CitationSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer_range: Option<TextRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cited_text: Option<String>,
    pub origin_protocol: ProviderProtocol,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub extra_body: HashMap<String, Value>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub wrapper_extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextRange {
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CitationSource {
    Url {
        url: String,
        title: Option<String>,
    },
    Document {
        index: u64,
        title: Option<String>,
        range: DocumentRange,
    },
    File {
        citation_type: FileCitationType,
        file_id: String,
        filename: Option<String>,
        index: Option<u64>,
        container_id: Option<String>,
    },
    ProviderNative {
        body: Value,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileCitationType {
    FileCitation,
    ContainerFileCitation,
    FilePath,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "unit", rename_all = "snake_case")]
pub enum DocumentRange {
    Characters { start: u64, end: u64 },
    Pages { start: u64, end: u64 },
    Blocks { start: u64, end: u64 },
}

impl Citation {
    pub fn decode(value: Value, protocol: ProviderProtocol) -> Self {
        let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
        let payload = if protocol == ProviderProtocol::ChatCompletion && kind == "url_citation" {
            value.get("url_citation").unwrap_or(&value)
        } else {
            &value
        };
        let string = |key: &str| payload.get(key).and_then(Value::as_str).map(str::to_owned);
        let number = |key: &str| payload.get(key).and_then(Value::as_u64);
        let range = |start: &str, end: &str| {
            number(start)
                .zip(number(end))
                .map(|(start, end)| TextRange { start, end })
        };
        let source = if let Some(url) = string("url")
            .or_else(|| string("uri"))
            .filter(|_| matches!(kind, "url_citation" | "web_search_result_location"))
        {
            Some(CitationSource::Url {
                url,
                title: string("title"),
            })
        } else if let Some(index) = number("document_index") {
            let document_range = match kind {
                "char_location" => {
                    range("start_char_index", "end_char_index").map(|r| DocumentRange::Characters {
                        start: r.start,
                        end: r.end,
                    })
                }
                "page_location" => {
                    range("start_page_number", "end_page_number").map(|r| DocumentRange::Pages {
                        start: r.start,
                        end: r.end,
                    })
                }
                "content_block_location" => {
                    range("start_block_index", "end_block_index").map(|r| DocumentRange::Blocks {
                        start: r.start,
                        end: r.end,
                    })
                }
                _ => None,
            };
            document_range.map(|range| CitationSource::Document {
                index,
                title: string("document_title"),
                range,
            })
        } else if let Some(file_id) = string("file_id").filter(|_| {
            matches!(
                kind,
                "file_citation" | "container_file_citation" | "file_path"
            )
        }) {
            Some(CitationSource::File {
                citation_type: match kind {
                    "container_file_citation" => FileCitationType::ContainerFileCitation,
                    "file_path" => FileCitationType::FilePath,
                    _ => FileCitationType::FileCitation,
                },
                file_id,
                filename: string("filename"),
                index: number("index"),
                container_id: string("container_id"),
            })
        } else {
            None
        };
        let Some(source) = source else {
            return Self {
                source: CitationSource::ProviderNative { body: value },
                answer_range: None,
                cited_text: None,
                origin_protocol: protocol,
                extra_body: HashMap::new(),
                wrapper_extra: HashMap::new(),
            };
        };
        let answer_range =
            range("start_index", "end_index").or_else(|| range("startIndex", "endIndex"));
        let cited_text = string("cited_text");

        let extra_body = payload
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(k, _)| !OWNED_FIELDS.contains(&k.as_str()) && !k.starts_with("_monoize_"))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Self {
            source,
            answer_range,
            cited_text,
            origin_protocol: protocol,
            extra_body,
            wrapper_extra: if protocol == ProviderProtocol::ChatCompletion && kind == "url_citation"
            {
                value
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter(|(key, _)| {
                        !matches!(key.as_str(), "type" | "url_citation")
                            && !key.starts_with("_monoize_")
                    })
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            } else {
                HashMap::new()
            },
        }
    }

    pub fn encode(&self, protocol: ProviderProtocol, offset: u64) -> Option<Value> {
        let same = self.origin_protocol == protocol;
        let mut body = match &self.source {
            CitationSource::ProviderNative { body } => {
                return same.then(|| super::encode::sanitize_provider_item_wire_body(body));
            }
            CitationSource::Url { url, title } => {
                let mut body = Map::new();
                match protocol {
                    ProviderProtocol::Messages => {
                        if !same {
                            return None;
                        }
                        body.insert("type".into(), json!("web_search_result_location"));
                        body.insert("url".into(), json!(url));
                    }
                    ProviderProtocol::Gemini => {
                        if !same {
                            return None;
                        }
                        body.insert("uri".into(), json!(url));
                    }
                    ProviderProtocol::Responses | ProviderProtocol::ChatCompletion => {
                        if self.answer_range.is_none() && !same {
                            return None;
                        }
                        body.insert("type".into(), json!("url_citation"));
                        body.insert("url".into(), json!(url));
                    }
                    _ => return None,
                }
                if let Some(title) = title {
                    body.insert("title".into(), json!(title));
                }
                if let Some(range) = &self.answer_range {
                    let (start, end) = if protocol == ProviderProtocol::Gemini {
                        ("startIndex", "endIndex")
                    } else {
                        ("start_index", "end_index")
                    };
                    body.insert(start.into(), json!(range.start.saturating_add(offset)));
                    body.insert(end.into(), json!(range.end.saturating_add(offset)));
                }
                Value::Object(body)
            }
            CitationSource::Document {
                index,
                title,
                range,
            } => {
                if protocol != ProviderProtocol::Messages {
                    return None;
                }
                let (kind, start_key, end_key, start, end) = match range {
                    DocumentRange::Characters { start, end } => (
                        "char_location",
                        "start_char_index",
                        "end_char_index",
                        start,
                        end,
                    ),
                    DocumentRange::Pages { start, end } => (
                        "page_location",
                        "start_page_number",
                        "end_page_number",
                        start,
                        end,
                    ),
                    DocumentRange::Blocks { start, end } => (
                        "content_block_location",
                        "start_block_index",
                        "end_block_index",
                        start,
                        end,
                    ),
                };
                let mut body =
                    json!({"type":kind,"document_index":index,start_key:start,end_key:end});
                if let Some(title) = title {
                    body["document_title"] = json!(title);
                }
                body
            }
            CitationSource::File {
                citation_type,
                file_id,
                filename,
                index,
                container_id,
            } => {
                if protocol != ProviderProtocol::Responses || !same {
                    return None;
                }
                let mut body = json!({"type":citation_type,"file_id":file_id});
                if let Some(v) = filename {
                    body["filename"] = json!(v);
                }
                if let Some(v) = index {
                    body["index"] = json!(v);
                }
                if let Some(v) = container_id {
                    body["container_id"] = json!(v);
                }
                if let Some(range) = &self.answer_range {
                    body["start_index"] = json!(range.start.saturating_add(offset));
                    body["end_index"] = json!(range.end.saturating_add(offset));
                }
                body
            }
        };
        if let Some(text) = &self.cited_text {
            if protocol == ProviderProtocol::Messages {
                body["cited_text"] = json!(text);
            }
        }
        if same {
            if let Some(obj) = body.as_object_mut() {
                for (key, value) in &self.extra_body {
                    if !OWNED_FIELDS.contains(&key.as_str()) && !key.starts_with("_monoize_") {
                        obj.entry(key.clone()).or_insert_with(|| value.clone());
                    }
                }
            }
        }
        if protocol == ProviderProtocol::ChatCompletion
            && matches!(self.source, CitationSource::Url { .. })
        {
            body.as_object_mut()?.remove("type");
            body = json!({"type":"url_citation","url_citation":body});
            if same {
                super::encode::merge_extra(body.as_object_mut()?, &self.wrapper_extra);
            }
        }
        Some(body)
    }
}

pub fn decode(values: Vec<Value>, protocol: ProviderProtocol) -> Vec<Citation> {
    values
        .into_iter()
        .map(|v| Citation::decode(v, protocol))
        .collect()
}

pub fn encode(values: &[Citation], protocol: ProviderProtocol, offset: u64) -> Vec<Value> {
    values
        .iter()
        .filter_map(|v| v.encode(protocol, offset))
        .collect()
}

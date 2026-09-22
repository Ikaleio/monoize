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
        let source = if let Some(url) = string("url").or_else(|| string("uri")).filter(|_| {
            protocol == ProviderProtocol::Gemini
                || matches!(kind, "url_citation" | "web_search_result_location")
        }) {
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
            let answer_range = if protocol == ProviderProtocol::Gemini {
                number("endIndex").map(|end| TextRange {
                    start: number("startIndex").unwrap_or(0),
                    end,
                })
            } else {
                None
            };
            let mut body = value;
            if protocol == ProviderProtocol::Gemini {
                if let Some(object) = body.as_object_mut() {
                    object.remove("startIndex");
                    object.remove("endIndex");
                }
            }
            return Self {
                source: CitationSource::ProviderNative { body },
                answer_range,
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
                if !same {
                    return None;
                }
                let mut body = super::encode::sanitize_provider_item_wire_body(body);
                if protocol == ProviderProtocol::Gemini {
                    if let Some(object) = body.as_object_mut() {
                        object.remove("startIndex");
                        object.remove("endIndex");
                        if let Some(range) = &self.answer_range {
                            object.insert(
                                "startIndex".into(),
                                json!(range.start.saturating_add(offset)),
                            );
                            object
                                .insert("endIndex".into(), json!(range.end.saturating_add(offset)));
                        }
                    }
                }
                return Some(body);
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

const GEMINI_GROUNDING: &str = "_monoize_gemini_grounding";

fn byte_range(text: &str, start: u64, end: u64) -> Option<TextRange> {
    let start = usize::try_from(start).ok()?;
    let end = usize::try_from(end).ok()?;
    if start > end || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
        return None;
    }
    Some(TextRange {
        start: text[..start].chars().count() as u64,
        end: text[..end].chars().count() as u64,
    })
}

fn scalar_byte(text: &str, offset: u64) -> Option<usize> {
    let offset = usize::try_from(offset).ok()?;
    text.char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(text.len()))
        .nth(offset)
}

fn push_citation(citations: &mut Vec<Citation>, citation: Citation) {
    if !citations.contains(&citation) {
        citations.push(citation);
    }
}

/// Maps Gemini byte ranges and grounding links onto current canonical text nodes.
pub fn attach_gemini(candidate: &Map<String, Value>, nodes: &mut [super::Node]) {
    if let Some(sources) = candidate
        .get("citationMetadata")
        .and_then(|value| value.get("citationSources"))
        .and_then(Value::as_array)
    {
        for source in sources {
            let decoded = Citation::decode(source.clone(), ProviderProtocol::Gemini);
            let start = source
                .get("startIndex")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let end = source.get("endIndex").and_then(Value::as_u64);
            let mut offset = 0u64;
            for node in nodes.iter_mut() {
                let super::Node::Text {
                    content, citations, ..
                } = node
                else {
                    continue;
                };
                let limit = offset.saturating_add(content.len() as u64);
                let mut citation = decoded.clone();
                if let Some(end) = end {
                    if end >= start && start < limit && end > offset {
                        citation.answer_range = byte_range(
                            content,
                            start.saturating_sub(offset),
                            end.min(limit) - offset,
                        );
                        if citation.answer_range.is_some() {
                            push_citation(citations, citation);
                        }
                    }
                } else {
                    citation.answer_range = None;
                    push_citation(citations, citation);
                    break;
                }
                offset = limit;
            }
        }
    }
    let Some(grounding) = candidate.get("groundingMetadata") else {
        return;
    };
    let Some(chunks) = grounding.get("groundingChunks").and_then(Value::as_array) else {
        return;
    };
    let Some(supports) = grounding.get("groundingSupports").and_then(Value::as_array) else {
        return;
    };
    for support in supports {
        let Some(segment) = support.get("segment") else {
            continue;
        };
        let part_index = segment
            .get("partIndex")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        let text_node_index = nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| !matches!(node, super::Node::NextDownstreamEnvelopeExtra { .. }))
            .nth(part_index)
            .map(|(index, _)| index);
        let Some(super::Node::Text {
            content, citations, ..
        }) = text_node_index.and_then(|index| nodes.get_mut(index))
        else {
            continue;
        };
        let Some(range) = segment
            .get("endIndex")
            .and_then(Value::as_u64)
            .and_then(|end| {
                byte_range(
                    content,
                    segment
                        .get("startIndex")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    end,
                )
            })
        else {
            continue;
        };
        let Some(indices) = support
            .get("groundingChunkIndices")
            .and_then(Value::as_array)
        else {
            continue;
        };
        for (position, index) in indices.iter().enumerate() {
            let Some(chunk) = index.as_u64().and_then(|index| chunks.get(index as usize)) else {
                continue;
            };
            let Some((kind, source)) = ["web", "retrievedContext", "maps", "image"]
                .into_iter()
                .find_map(|kind| chunk.get(kind).map(|source| (kind, source)))
            else {
                continue;
            };
            let uri_key = if kind == "image" { "sourceUri" } else { "uri" };
            let Some(uri) = source.get(uri_key).and_then(Value::as_str) else {
                continue;
            };
            let mut citation = Citation::decode(
                json!({"uri":uri,"title":source.get("title")}),
                ProviderProtocol::Gemini,
            );
            citation.answer_range = Some(range.clone());
            let source_extra = source
                .as_object()
                .map(|source| super::decode::split_extra(source, &[uri_key, "title"]))
                .unwrap_or_default();
            let support_extra = support
                .as_object()
                .map(|support| {
                    super::decode::split_extra(
                        support,
                        &["segment", "groundingChunkIndices", "confidenceScores"],
                    )
                })
                .unwrap_or_default();
            let confidence = support
                .get("confidenceScores")
                .and_then(Value::as_array)
                .and_then(|scores| scores.get(position));
            citation.wrapper_extra.insert(GEMINI_GROUNDING.into(), json!({"kind":kind,"source":source_extra,"support":support_extra,"confidence":confidence}));
            push_citation(citations, citation);
        }
    }
}

/// Keeps provider presentation metadata without duplicate typed sources or answer text.
pub fn gemini_metadata_extra(candidate: &Map<String, Value>) -> Option<Value> {
    let mut metadata = candidate.get("groundingMetadata")?.as_object()?.clone();
    if metadata
        .get("groundingSupports")
        .and_then(Value::as_array)
        .is_some_and(|supports| !supports.is_empty())
    {
        metadata.remove("groundingSupports");
        metadata.remove("groundingChunks");
    }
    (!metadata.is_empty()).then_some(Value::Object(metadata))
}

/// Converts node-relative Unicode scalar ranges to candidate-relative UTF-8 byte ranges.
pub fn encode_gemini(nodes: &[super::Node]) -> Vec<Value> {
    let mut sources = Vec::new();
    let mut offset = 0u64;
    for node in nodes {
        let super::Node::Text {
            content, citations, ..
        } = node
        else {
            continue;
        };
        for citation in citations {
            let mut citation = citation.clone();
            if let Some(range) = &citation.answer_range {
                let Some((start, end)) =
                    scalar_byte(content, range.start).zip(scalar_byte(content, range.end))
                else {
                    continue;
                };
                if start > end {
                    continue;
                }
                citation.answer_range = Some(TextRange {
                    start: start as u64,
                    end: end as u64,
                });
            }
            if let Some(value) = citation.encode(ProviderProtocol::Gemini, offset) {
                if !sources.contains(&value) {
                    sources.push(value);
                }
            }
        }
        offset += content.len() as u64;
    }
    sources
}

/// Rebuilds native grounding links from typed citations and current text.
pub fn encode_gemini_grounding(nodes: &[super::Node]) -> Option<Value> {
    let mut chunks = Vec::<Value>::new();
    let mut supports = Vec::new();
    let mut part_index = 0;
    for node in nodes {
        if matches!(node, super::Node::NextDownstreamEnvelopeExtra { .. }) {
            continue;
        }
        if let super::Node::Text {
            content, citations, ..
        } = node
        {
            for citation in citations {
                let Some(shape) = citation.wrapper_extra.get(GEMINI_GROUNDING) else {
                    continue;
                };
                let CitationSource::Url { url, title } = &citation.source else {
                    continue;
                };
                let Some(range) = &citation.answer_range else {
                    continue;
                };
                let Some((start, end)) =
                    scalar_byte(content, range.start).zip(scalar_byte(content, range.end))
                else {
                    continue;
                };
                if start > end {
                    continue;
                }
                let kind = shape.get("kind").and_then(Value::as_str).unwrap_or("web");
                let mut source = shape
                    .get("source")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                source.insert(
                    if kind == "image" { "sourceUri" } else { "uri" }.into(),
                    json!(url),
                );
                source.remove("title");
                if let Some(title) = title {
                    source.insert("title".into(), json!(title));
                }
                let chunk = json!({kind:source});
                let index = chunks
                    .iter()
                    .position(|old| *old == chunk)
                    .unwrap_or_else(|| {
                        chunks.push(chunk);
                        chunks.len() - 1
                    });
                let mut support = shape
                    .get("support")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                support.insert("segment".into(), json!({"partIndex":part_index,"startIndex":start,"endIndex":end,"text":&content[start..end]}));
                support.insert("groundingChunkIndices".into(), json!([index]));
                if let Some(confidence) = shape.get("confidence").filter(|value| !value.is_null()) {
                    support.insert("confidenceScores".into(), json!([confidence]));
                }
                supports.push(Value::Object(support));
            }
        }
        part_index += 1;
    }
    (!supports.is_empty()).then(|| json!({"groundingChunks":chunks,"groundingSupports":supports}))
}

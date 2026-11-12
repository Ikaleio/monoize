//! Signature placement on encoded Parts (PG4f, PG5a, PG5a3, PG8): bound signature carriers,
//! restored server-tool Parts, current-turn dummy signatures, and Gemini provenance.

use crate::urp::decode::gemini::SIGNATURE_ID_PREFIX;
use crate::urp::{
    GEMINI_BOUND_ITEM_OFFSET_KEY, GEMINI_BOUND_OWNER_KEY, GEMINI_BOUND_SIGNATURE_KEY, Node,
    parse_reasoning_envelope,
};
use serde_json::{Value, json};
use std::collections::HashMap;

/// Documented validator-skip value for a current-turn function call without a Gemini signature.
const DUMMY_THOUGHT_SIGNATURE: &str = "skip_thought_signature_validator";

/// Tolerance for a downstream that inserted separators between merged texts.
const MERGED_SCAN_WINDOW: usize = 1024;

/// `(byte length, xxh3, response text offset)` of the text that owns a bound signature.
type Owner = (usize, u64, u64);

/// Attaches each bound signature placeholder to its owning Part and removes the placeholder.
/// A text-owner binding (byte length, xxh3, response text offset) selects, in order: the unsigned
/// text Part with that text at that offset; with `merged`, that text at or just after the offset
/// inside a merged text Part, which is split there; the nearest Part with exactly that text,
/// preceding the placeholder first. Remaining placeholders attach to the adjacent unsigned text
/// or media Part (preceding first, then following) or become a standalone empty-text Part.
/// With `merged`, restored server-tool Parts then return to their text offsets.
/// Returns removed placeholder positions, each relative to the vector after earlier removals;
/// `merged` callers must not rely on them.
pub(super) fn resolve_bound_signatures(parts: &mut Vec<Value>, merged: bool) -> Vec<usize> {
    fn accepts(part: &Value) -> bool {
        part.get(GEMINI_BOUND_SIGNATURE_KEY).is_none()
            && part.get("thoughtSignature").is_none()
            && (part.get("inlineData").is_some()
                || part.get("fileData").is_some()
                || plain_text(part).is_some())
    }
    fn owner_matches(text: &str, length: usize, hash: u64) -> bool {
        text.len() == length && xxhash_rust::xxh3::xxh3_64(text.as_bytes()) == hash
    }

    let placeholders: Vec<(usize, Value, Option<Owner>)> = parts
        .iter()
        .enumerate()
        .filter_map(|(index, part)| {
            let signature = part.get(GEMINI_BOUND_SIGNATURE_KEY)?.clone();
            let owner = part.get(GEMINI_BOUND_OWNER_KEY).and_then(|owner| {
                Some((
                    usize::try_from(owner.get(0)?.as_u64()?).ok()?,
                    owner.get(1)?.as_u64()?,
                    owner.get(2)?.as_u64()?,
                ))
            });
            Some((index, signature, owner))
        })
        .collect();
    if placeholders.is_empty() {
        place_server_tool_parts(parts, merged);
        return Vec::new();
    }
    let mut offsets = vec![0u64; parts.len()];
    let mut position = 0;
    for (index, part) in parts.iter().enumerate() {
        offsets[index] = position;
        position += plain_text(part).map_or(0, |text| text.len() as u64);
    }
    let mut signed: HashMap<usize, Value> = HashMap::new();
    let mut splits: HashMap<usize, Vec<(usize, usize, Value)>> = HashMap::new();
    let mut resolved = vec![false; placeholders.len()];
    let free = |index: usize, signed: &HashMap<usize, Value>, splits: &HashMap<_, Vec<_>>| {
        !signed.contains_key(&index) && !splits.contains_key(&index)
    };
    let exact = |index: usize, length: usize, hash: u64| {
        accepts(&parts[index])
            && plain_text(&parts[index]).is_some_and(|text| owner_matches(text, length, hash))
    };
    // Owner text at its offset.
    for (slot, (_, signature, owner)) in placeholders.iter().enumerate() {
        let Some((length, hash, offset)) = *owner else {
            continue;
        };
        if let Some(target) = (0..parts.len()).find(|index| {
            offsets[*index] == offset
                && free(*index, &signed, &splits)
                && exact(*index, length, hash)
        }) {
            signed.insert(target, signature.clone());
            resolved[slot] = true;
        }
    }
    // Owner text inside a merged text Part.
    if merged {
        for (slot, (_, signature, owner)) in placeholders.iter().enumerate() {
            let (Some((length, hash, offset)), false) = (*owner, resolved[slot]) else {
                continue;
            };
            let found = (0..parts.len()).find_map(|index| {
                if signed.contains_key(&index) || !accepts(&parts[index]) {
                    return None;
                }
                let text = plain_text(&parts[index])?;
                let local = usize::try_from(offset.checked_sub(offsets[index])?).ok()?;
                (local..=local.saturating_add(MERGED_SCAN_WINDOW))
                    .take_while(|start| start + length <= text.len())
                    .filter(|start| {
                        text.is_char_boundary(*start) && text.is_char_boundary(start + length)
                    })
                    .find(|start| owner_matches(&text[*start..start + length], length, hash))
                    .map(|start| (index, start))
            });
            if let Some((index, start)) = found {
                splits
                    .entry(index)
                    .or_default()
                    .push((start, length, signature.clone()));
                resolved[slot] = true;
            }
        }
    }
    // Owner text elsewhere, nearest first.
    for (slot, (index, signature, owner)) in placeholders.iter().enumerate() {
        let (Some((length, hash, _)), false) = (*owner, resolved[slot]) else {
            continue;
        };
        if let Some(target) = (0..*index)
            .rev()
            .chain(*index + 1..parts.len())
            .find(|target| free(*target, &signed, &splits) && exact(*target, length, hash))
        {
            signed.insert(target, signature.clone());
            resolved[slot] = true;
        }
    }
    // Adjacency.
    for (slot, (index, signature, _)) in placeholders.iter().enumerate() {
        if resolved[slot] {
            continue;
        }
        let target = [index.checked_sub(1), Some(index + 1)]
            .into_iter()
            .flatten()
            .find(|target| {
                parts.get(*target).is_some_and(accepts) && free(*target, &signed, &splits)
            });
        if let Some(target) = target {
            signed.insert(target, signature.clone());
            resolved[slot] = true;
        }
    }

    let mut removed = Vec::new();
    let mut slots = placeholders.iter().zip(&resolved).peekable();
    let mut rebuilt = Vec::with_capacity(parts.len());
    for (index, mut part) in std::mem::take(parts).into_iter().enumerate() {
        if let Some(((placeholder, signature, _), resolved)) =
            slots.next_if(|((placeholder, ..), _)| *placeholder == index)
        {
            if *resolved {
                removed.push(placeholder - removed.len());
            } else {
                rebuilt.push(json!({"text": "", "thoughtSignature": signature}));
            }
            continue;
        }
        if let Some(mut segments) = splits.remove(&index) {
            segments.sort_by_key(|(start, ..)| *start);
            let text = plain_text(&part).unwrap_or_default().to_string();
            let mut position = 0;
            for (start, length, signature) in segments {
                if start > position {
                    rebuilt.push(json!({ "text": &text[position..start] }));
                }
                rebuilt.push(
                    json!({"text": &text[start..start + length], "thoughtSignature": signature}),
                );
                position = start + length;
            }
            if position < text.len() {
                rebuilt.push(json!({ "text": &text[position..] }));
            }
            continue;
        }
        if let Some(signature) = signed.remove(&index) {
            part["thoughtSignature"] = signature;
        }
        rebuilt.push(part);
    }
    *parts = rebuilt;
    place_server_tool_parts(parts, merged);
    removed
}

/// The text of an answer text Part; thoughts and placeholders have none.
fn plain_text(part: &Value) -> Option<&str> {
    (part.get(GEMINI_BOUND_SIGNATURE_KEY).is_none()
        && part.get("thought").and_then(Value::as_bool) != Some(true))
    .then(|| part.get("text").and_then(Value::as_str))
    .flatten()
}

/// Removes the encoder-local offset of restored server-tool Parts. With `relocate`, a Part whose
/// text position differs from its offset (a downstream moved it away from merged text) returns
/// to that offset, splitting an unsigned text Part when the offset falls inside it.
fn place_server_tool_parts(parts: &mut Vec<Value>, relocate: bool) {
    let mut moved = Vec::new();
    let mut position = 0;
    let mut index = 0;
    while index < parts.len() {
        let offset = parts[index]
            .as_object_mut()
            .and_then(|part| part.remove(GEMINI_BOUND_ITEM_OFFSET_KEY))
            .and_then(|offset| offset.as_u64());
        if relocate && offset.is_some_and(|offset| offset != position) {
            moved.push((offset.unwrap_or_default(), parts.remove(index)));
            continue;
        }
        position += plain_text(&parts[index]).map_or(0, |text| text.len() as u64);
        index += 1;
    }
    for (offset, item) in moved {
        let mut position = 0;
        let mut index = 0;
        while index < parts.len() {
            let length = plain_text(&parts[index]).map_or(0, |text| text.len() as u64);
            let passable = length == 0
                && (parts[index].get("toolCall").is_some()
                    || parts[index].get("toolResponse").is_some()
                    || parts[index].get("text").is_some());
            if position == offset && !passable {
                break;
            }
            if position < offset && offset < position + length {
                let at = (offset - position) as usize;
                let text = plain_text(&parts[index]).unwrap_or_default();
                if parts[index].get("thoughtSignature").is_none() && text.is_char_boundary(at) {
                    let tail = json!({ "text": &text[at..] });
                    parts[index] = json!({ "text": &text[..at] });
                    parts.insert(index + 1, tail);
                }
                index += 1;
                break;
            }
            position += length;
            index += 1;
        }
        parts.insert(index, item);
    }
}

/// PG4f: Gemini 3+ rejects a current-turn step whose first functionCall lacks a signature. The
/// current turn starts after the last user Content that holds anything besides function
/// responses; each model Content after it is one step.
pub(super) fn sign_current_turn_calls(contents: &mut [Value]) {
    let start = contents
        .iter()
        .rposition(|content| {
            content.get("role").and_then(Value::as_str) == Some("user")
                && content
                    .get("parts")
                    .and_then(Value::as_array)
                    .is_some_and(|parts| {
                        parts
                            .iter()
                            .any(|part| part.get("functionResponse").is_none())
                    })
        })
        .map_or(0, |index| index + 1);
    for content in &mut contents[start..] {
        if content.get("role").and_then(Value::as_str) != Some("model") {
            continue;
        }
        let Some(part) = content
            .get_mut("parts")
            .and_then(Value::as_array_mut)
            .and_then(|parts| {
                parts
                    .iter_mut()
                    .find(|part| part.get("functionCall").is_some())
            })
        else {
            continue;
        };
        let unsigned = match part.get("thoughtSignature") {
            None | Some(Value::Null) => true,
            Some(Value::String(signature)) => signature.is_empty(),
            Some(_) => false,
        };
        if unsigned {
            part["thoughtSignature"] = json!(DUMMY_THOUGHT_SIGNATURE);
        }
    }
}

/// Gemini accepts only its own signatures. Before envelope unwrapping, clears call signatures
/// and drops reasoning signatures whose provenance is not Gemini: an `mz2.` envelope must name
/// provider type `gemini`, an `mz1.` sigil must bind an `rs_gemini_` item id, and a raw
/// reasoning value must belong to an `rs_gemini_` Reasoning id. Raw call signatures can only
/// arrive through the Gemini call transport and are kept. Reasoning left without text is
/// removed.
pub fn retain_gemini_signature_provenance(nodes: &mut Vec<Node>) {
    fn gemini_provenance(value: &Value, id: Option<&str>, raw_accepted: bool) -> bool {
        let gemini_id = |id: &str| id.starts_with(SIGNATURE_ID_PREFIX);
        match parse_reasoning_envelope(value) {
            Some(envelope) if envelope.v == 2 => envelope.provider_type == "gemini",
            Some(envelope) => envelope.item_id.as_deref().is_some_and(gemini_id),
            None => raw_accepted || id.is_some_and(gemini_id),
        }
    }
    nodes.retain_mut(|node| match node {
        Node::ToolCall { signature, .. } => {
            if signature
                .as_ref()
                .is_some_and(|value| !gemini_provenance(value, None, true))
            {
                *signature = None;
            }
            true
        }
        Node::Reasoning {
            id,
            metadata,
            content,
            summary,
            encrypted,
            ..
        } => {
            let foreign = encrypted.as_ref().is_some_and(|value| {
                !gemini_provenance(value, id.as_deref().or(metadata.item_id.as_deref()), false)
            });
            if foreign {
                *encrypted = None;
                return content.as_deref().is_some_and(|text| !text.is_empty())
                    || summary.as_deref().is_some_and(|text| !text.is_empty());
            }
            true
        }
        _ => true,
    });
}

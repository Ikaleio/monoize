//! Incremental decoders for `input_json_delta` fragments.

use serde_json::Value;

/// Decodes the `{"input": "<string>"}` wrapper of a custom tool call fragment by fragment,
/// yielding only the characters of the decoded string.
#[derive(Debug, Clone)]
pub(super) struct CustomToolInputDecoder {
    phase: CustomToolInputPhase,
    string_decoder: JsonStringDecoder,
    key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CustomToolInputPhase {
    Start,
    KeyStart,
    Key,
    Colon,
    ValueStart,
    Value,
    Tail,
    Done,
}

impl CustomToolInputDecoder {
    pub(super) fn new() -> Self {
        Self {
            phase: CustomToolInputPhase::Start,
            string_decoder: JsonStringDecoder::default(),
            key: String::new(),
        }
    }

    pub(super) fn push_fragment(&mut self, fragment: &str) -> Result<String, String> {
        let mut decoded = String::new();
        for character in fragment.chars() {
            match self.phase {
                CustomToolInputPhase::Start => {
                    if json_whitespace(character) {
                        continue;
                    }
                    if character != '{' {
                        return Err("custom tool input wrapper must be a JSON object".to_string());
                    }
                    self.phase = CustomToolInputPhase::KeyStart;
                }
                CustomToolInputPhase::KeyStart => {
                    if json_whitespace(character) {
                        continue;
                    }
                    if character != '"' {
                        return Err(
                            "custom tool input wrapper must contain the input field".to_string()
                        );
                    }
                    self.string_decoder = JsonStringDecoder::default();
                    self.key.clear();
                    self.phase = CustomToolInputPhase::Key;
                }
                CustomToolInputPhase::Key => match self.string_decoder.push(character)? {
                    JsonStringStep::Continue => {}
                    JsonStringStep::Emit(character) => self.key.push(character),
                    JsonStringStep::End => {
                        if self.key != "input" {
                            return Err(
                                "custom tool input wrapper field must be named input".to_string()
                            );
                        }
                        self.phase = CustomToolInputPhase::Colon;
                    }
                },
                CustomToolInputPhase::Colon => {
                    if json_whitespace(character) {
                        continue;
                    }
                    if character != ':' {
                        return Err(
                            "custom tool input wrapper is missing the input separator".to_string()
                        );
                    }
                    self.phase = CustomToolInputPhase::ValueStart;
                }
                CustomToolInputPhase::ValueStart => {
                    if json_whitespace(character) {
                        continue;
                    }
                    if character != '"' {
                        return Err("custom tool input must be a JSON string".to_string());
                    }
                    self.string_decoder = JsonStringDecoder::default();
                    self.phase = CustomToolInputPhase::Value;
                }
                CustomToolInputPhase::Value => match self.string_decoder.push(character)? {
                    JsonStringStep::Continue => {}
                    JsonStringStep::Emit(character) => decoded.push(character),
                    JsonStringStep::End => self.phase = CustomToolInputPhase::Tail,
                },
                CustomToolInputPhase::Tail => {
                    if json_whitespace(character) {
                        continue;
                    }
                    if character != '}' {
                        return Err(
                            "custom tool input wrapper must contain only the input field"
                                .to_string(),
                        );
                    }
                    self.phase = CustomToolInputPhase::Done;
                }
                CustomToolInputPhase::Done => {
                    if !json_whitespace(character) {
                        return Err(
                            "custom tool input wrapper has data after the JSON object".to_string()
                        );
                    }
                }
            }
        }
        Ok(decoded)
    }

    pub(super) fn finish(&self) -> Result<(), String> {
        if self.phase == CustomToolInputPhase::Done {
            Ok(())
        } else {
            Err("custom tool input wrapper ended before the JSON object was complete".to_string())
        }
    }
}

fn json_whitespace(character: char) -> bool {
    matches!(character, ' ' | '\n' | '\r' | '\t')
}

/// Decodes the body of one JSON string literal after its opening quote.
#[derive(Debug, Clone, Default)]
struct JsonStringDecoder {
    escape: JsonStringEscape,
}

#[derive(Debug, Clone, Copy, Default)]
enum JsonStringEscape {
    #[default]
    None,
    Escape,
    Unicode {
        value: u16,
        digits: u8,
    },
    HighSurrogateSlash {
        high: u16,
    },
    HighSurrogateU {
        high: u16,
    },
    LowSurrogate {
        high: u16,
        value: u16,
        digits: u8,
    },
}

enum JsonStringStep {
    Continue,
    Emit(char),
    End,
}

const MISSING_LOW_SURROGATE: &str =
    "custom tool input high surrogate is not followed by a low surrogate";

impl JsonStringDecoder {
    fn push(&mut self, character: char) -> Result<JsonStringStep, String> {
        match std::mem::take(&mut self.escape) {
            JsonStringEscape::None => match character {
                '"' => Ok(JsonStringStep::End),
                '\\' => {
                    self.escape = JsonStringEscape::Escape;
                    Ok(JsonStringStep::Continue)
                }
                character if (character as u32) < 0x20 => {
                    Err("custom tool input contains an unescaped control character".to_string())
                }
                character => Ok(JsonStringStep::Emit(character)),
            },
            JsonStringEscape::Escape => {
                let decoded = match character {
                    '"' => '"',
                    '\\' => '\\',
                    '/' => '/',
                    'b' => '\u{0008}',
                    'f' => '\u{000c}',
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    'u' => {
                        self.escape = JsonStringEscape::Unicode {
                            value: 0,
                            digits: 0,
                        };
                        return Ok(JsonStringStep::Continue);
                    }
                    _ => {
                        return Err("custom tool input contains an invalid JSON escape".to_string());
                    }
                };
                Ok(JsonStringStep::Emit(decoded))
            }
            JsonStringEscape::Unicode { value, digits } => {
                let value = (value << 4) | json_hex_digit(character)?;
                let digits = digits + 1;
                if digits < 4 {
                    self.escape = JsonStringEscape::Unicode { value, digits };
                    return Ok(JsonStringStep::Continue);
                }
                if (0xd800..=0xdbff).contains(&value) {
                    self.escape = JsonStringEscape::HighSurrogateSlash { high: value };
                    return Ok(JsonStringStep::Continue);
                }
                if (0xdc00..=0xdfff).contains(&value) {
                    return Err("custom tool input contains an unmatched low surrogate".to_string());
                }
                char::from_u32(value as u32)
                    .map(JsonStringStep::Emit)
                    .ok_or_else(|| {
                        "custom tool input contains an invalid Unicode escape".to_string()
                    })
            }
            JsonStringEscape::HighSurrogateSlash { high } => {
                if character != '\\' {
                    return Err(MISSING_LOW_SURROGATE.to_string());
                }
                self.escape = JsonStringEscape::HighSurrogateU { high };
                Ok(JsonStringStep::Continue)
            }
            JsonStringEscape::HighSurrogateU { high } => {
                if character != 'u' {
                    return Err(MISSING_LOW_SURROGATE.to_string());
                }
                self.escape = JsonStringEscape::LowSurrogate {
                    high,
                    value: 0,
                    digits: 0,
                };
                Ok(JsonStringStep::Continue)
            }
            JsonStringEscape::LowSurrogate {
                high,
                value,
                digits,
            } => {
                let value = (value << 4) | json_hex_digit(character)?;
                let digits = digits + 1;
                if digits < 4 {
                    self.escape = JsonStringEscape::LowSurrogate {
                        high,
                        value,
                        digits,
                    };
                    return Ok(JsonStringStep::Continue);
                }
                if !(0xdc00..=0xdfff).contains(&value) {
                    return Err(MISSING_LOW_SURROGATE.to_string());
                }
                let scalar = 0x10000 + (((high as u32) - 0xd800) << 10) + ((value as u32) - 0xdc00);
                char::from_u32(scalar)
                    .map(JsonStringStep::Emit)
                    .ok_or_else(|| {
                        "custom tool input contains an invalid surrogate pair".to_string()
                    })
            }
        }
    }
}

fn json_hex_digit(character: char) -> Result<u16, String> {
    character
        .to_digit(16)
        .map(|digit| digit as u16)
        .ok_or_else(|| "custom tool input contains an invalid Unicode escape".to_string())
}

/// Assembles the `input` member of a server-side Messages block from `input_json_delta`
/// fragments. The start block's `input` stays in force until the first delta arrives.
#[derive(Debug, Clone)]
pub(super) struct PartialJsonInput {
    assembled: String,
    replace_on_next_delta: bool,
    saw_delta: bool,
}

impl PartialJsonInput {
    pub(super) fn from_content_block(content_block: &Value) -> Self {
        let input = content_block.get("input");
        Self {
            assembled: json_value_to_string(input),
            replace_on_next_delta: input_is_placeholder(input),
            saw_delta: false,
        }
    }

    pub(super) fn merge_delta(&mut self, delta: &Value) {
        if delta.get("type").and_then(Value::as_str) != Some("input_json_delta") {
            return;
        }
        let Some(partial_json) = delta.get("partial_json").and_then(Value::as_str) else {
            return;
        };
        if self.replace_on_next_delta {
            self.assembled.clear();
            self.replace_on_next_delta = false;
        }
        self.assembled.push_str(partial_json);
        self.saw_delta = true;
    }

    /// `body` with `input` replaced by the assembled deltas, parsed as JSON when valid.
    pub(super) fn apply_to(self, mut body: Value) -> Value {
        if !self.saw_delta {
            return body;
        }
        let input =
            serde_json::from_str(&self.assembled).unwrap_or_else(|_| Value::String(self.assembled));
        if let Some(object) = body.as_object_mut() {
            object.insert("input".to_string(), input);
        }
        body
    }
}

pub(super) fn json_value_to_string(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    }
}

/// A start-block `input` that only reserves the slot: absent, null, or `{}`.
pub(super) fn input_is_placeholder(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::Object(object)) => object.is_empty(),
        Some(_) => false,
    }
}

//! Typed canonical model for decision requests (`decision-api.spec.md` §5)
//! and conversion between the System One and OpenAI Decisions wire formats.

mod openai;
mod system_one;

use serde_json::{Map, Value};

/// Wire format of a decision endpoint or Channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionFormat {
    SystemOne,
    OpenaiDecisions,
}

impl DecisionFormat {
    pub const ALL: [Self; 2] = [Self::SystemOne, Self::OpenaiDecisions];

    /// DR-P1: upstream path for a Channel whose effective API type is this format.
    pub fn upstream_path(self) -> &'static str {
        match self {
            Self::SystemOne => "/v1/systemone",
            Self::OpenaiDecisions => "/v1/decisions",
        }
    }

    /// DR-V3, DR-V4: shape checks that run before routing.
    pub fn validate_shape(self, body: &Map<String, Value>) -> Result<(), String> {
        match self {
            Self::SystemOne => system_one::validate_shape(body),
            Self::OpenaiDecisions => openai::validate_shape(body),
        }
    }

    pub fn decode_request(self, body: &Map<String, Value>) -> Result<DecisionRequest, String> {
        match self {
            Self::SystemOne => system_one::decode_request(body),
            Self::OpenaiDecisions => openai::decode_request(body),
        }
    }

    /// Encodes `request` as an upstream body in this format. The caller sets `model`.
    pub fn encode_request(self, request: &DecisionRequest) -> Result<Map<String, Value>, String> {
        match self {
            Self::SystemOne => system_one::encode_request(request),
            Self::OpenaiDecisions => openai::encode_request(request),
        }
    }

    /// Decodes an upstream response body in this format into one answer per question.
    pub fn decode_answers(
        self,
        request: &DecisionRequest,
        body: &Value,
    ) -> Result<Vec<Answer>, String> {
        match self {
            Self::SystemOne => system_one::decode_answers(request, body),
            Self::OpenaiDecisions => openai::decode_answers(request, body),
        }
    }

    /// Encodes a downstream response body in this format.
    pub fn encode_response(
        self,
        request: &DecisionRequest,
        answers: &[Answer],
        usage: &DecisionUsage,
        model: &str,
    ) -> Result<Value, String> {
        match self {
            Self::SystemOne => system_one::encode_response(request, answers, usage, model),
            Self::OpenaiDecisions => Ok(openai::encode_response(request, answers, usage, model)),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecisionRequest {
    pub input: DecisionInput,
    pub questions: Vec<Question>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DecisionInput {
    Text(String),
    /// A System One `state` that is a JSON object or array.
    Structured(Value),
    /// OpenAI user-message content parts in message order.
    Parts(Vec<InputPart>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputPart {
    Text(String),
    Image {
        image_url: String,
        detail: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Question {
    pub name: Option<String>,
    pub instructions: Value,
    pub kind: QuestionKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum QuestionKind {
    Binary {
        when_true: Option<Value>,
        when_false: Option<Value>,
    },
    Choice(Vec<ChoiceOption>),
    Score(Vec<ScoreLevel>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChoiceOption {
    pub value: ChoiceValue,
    pub description: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ChoiceValue {
    Text(String),
    Bool(bool),
}

impl ChoiceValue {
    fn from_json(value: &Value) -> Option<Self> {
        match value {
            Value::String(text) => Some(Self::Text(text.clone())),
            Value::Bool(flag) => Some(Self::Bool(*flag)),
            _ => None,
        }
    }

    fn to_json(&self) -> Value {
        match self {
            Self::Text(text) => Value::String(text.clone()),
            Self::Bool(flag) => Value::Bool(*flag),
        }
    }

    /// DR-B13: System One option key.
    fn key(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Bool(flag) => flag.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScoreLevel {
    pub label: Value,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    Binary {
        probability: f64,
    },
    /// `index` and `probabilities` follow the question's option order.
    Choice {
        index: usize,
        probabilities: Vec<f64>,
        confidence: Option<f64>,
    },
    /// `probabilities` follow the question's level order.
    Score {
        score: f64,
        probabilities: Vec<f64>,
        confidence: Option<f64>,
    },
    Refusal,
}

/// Token counts reported by a decision upstream (DR-S1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DecisionUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub cache_write_tokens: u64,
}

impl DecisionUsage {
    /// Reads `usage` from an upstream body. Absent counts read as zero.
    pub fn from_body(body: &Value) -> Self {
        let usage = body.get("usage");
        let count = |value: Option<&Value>| value.and_then(Value::as_u64).unwrap_or(0);
        let details = usage.and_then(|usage| usage.get("input_tokens_details"));
        Self {
            input_tokens: count(usage.and_then(|usage| usage.get("input_tokens"))),
            output_tokens: count(usage.and_then(|usage| usage.get("output_tokens"))),
            cached_tokens: count(details.and_then(|details| details.get("cached_tokens"))),
            cache_write_tokens: count(details.and_then(|details| details.get("cache_write_tokens"))),
        }
    }
}

/// DR-B3: a string as itself, any other value as compact JSON.
fn text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// DR-B11: System One question keys for `questions`, in question order.
fn system_one_keys(questions: &[Question]) -> Result<Vec<String>, String> {
    let mut used = std::collections::HashSet::new();
    for name in questions.iter().filter_map(|question| question.name.as_deref()) {
        if !used.insert(name.to_string()) {
            return Err(format!("duplicate question name: {name}"));
        }
    }
    Ok(questions
        .iter()
        .enumerate()
        .map(|(index, question)| match &question.name {
            Some(name) => name.clone(),
            None => {
                let mut key = format!("q{index}");
                while !used.insert(key.clone()) {
                    key.push('_');
                }
                key
            }
        })
        .collect())
}

fn number(answer: &Map<String, Value>, field: &str, path: &str) -> Result<f64, String> {
    answer
        .get(field)
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("{path}.{field} must be a number"))
}

fn insert_confidence(out: &mut Map<String, Value>, confidence: Option<f64>) {
    if let Some(confidence) = confidence {
        out.insert("confidence".to_string(), Value::from(confidence));
    }
}

fn kind_mismatch(path: &str, answer_type: Option<&str>) -> String {
    format!(
        "{path} has type {} that does not match its question",
        answer_type.unwrap_or("<missing>")
    )
}

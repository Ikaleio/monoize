//! System One wire format (TypeSafe `POST /v1/systemone`).

use super::*;

pub(super) fn validate_shape(body: &Map<String, Value>) -> Result<(), String> {
    match body.get("state") {
        Some(Value::String(_) | Value::Object(_) | Value::Array(_)) => {}
        Some(_) => return Err("state must be a string, object, or array".to_string()),
        None => return Err("missing state".to_string()),
    }
    match body.get("questions") {
        Some(Value::Object(questions)) if !questions.is_empty() => Ok(()),
        _ => Err("questions must be a non-empty object".to_string()),
    }
}

pub(super) fn decode_request(body: &Map<String, Value>) -> Result<DecisionRequest, String> {
    let input = match body.get("state") {
        Some(Value::String(text)) => DecisionInput::Text(text.clone()),
        Some(value @ (Value::Object(_) | Value::Array(_))) => DecisionInput::Structured(value.clone()),
        _ => return Err("state must be a string, object, or array".to_string()),
    };
    let Some(Value::Object(entries)) = body.get("questions") else {
        return Err("questions must be a non-empty object".to_string());
    };
    let questions = entries
        .iter()
        .map(|(key, question)| decode_question(key, question))
        .collect::<Result<_, _>>()?;
    Ok(DecisionRequest { input, questions })
}

fn decode_question(key: &str, value: &Value) -> Result<Question, String> {
    let path = format!("questions.{key}");
    let question = value
        .as_object()
        .ok_or_else(|| format!("{path} must be an object"))?;
    let instructions = question
        .get("instructions")
        .cloned()
        .ok_or_else(|| format!("{path}.instructions is required"))?;
    let kind = match question.get("type").and_then(Value::as_str) {
        Some("noul") => {
            let criteria = match question.get("criteria") {
                None | Some(Value::Null) => None,
                Some(Value::Object(criteria)) => Some(criteria),
                Some(_) => return Err(format!("{path}.criteria must be an object")),
            };
            QuestionKind::Binary {
                when_true: criteria.and_then(|criteria| criteria.get("true")).cloned(),
                when_false: criteria.and_then(|criteria| criteria.get("false")).cloned(),
            }
        }
        Some("choice") => {
            let Some(Value::Object(criteria)) = question.get("criteria") else {
                return Err(format!("{path}.criteria must be an object"));
            };
            QuestionKind::Choice(
                criteria
                    .iter()
                    .map(|(option, description)| ChoiceOption {
                        value: ChoiceValue::Text(option.clone()),
                        description: (!description.is_null()).then(|| description.clone()),
                    })
                    .collect(),
            )
        }
        Some("score") => {
            let Some(Value::Array(levels)) = question.get("criteria") else {
                return Err(format!("{path}.criteria must be an array"));
            };
            QuestionKind::Score(
                levels
                    .iter()
                    .map(|label| ScoreLevel {
                        label: label.clone(),
                        description: None,
                    })
                    .collect(),
            )
        }
        _ => return Err(format!("{path}.type must be noul, choice, or score")),
    };
    Ok(Question {
        name: Some(key.to_string()),
        instructions,
        kind,
    })
}

pub(super) fn encode_request(request: &DecisionRequest) -> Result<Map<String, Value>, String> {
    let state = match &request.input {
        DecisionInput::Text(text) => Value::String(text.clone()),
        DecisionInput::Structured(value) => value.clone(),
        DecisionInput::Parts(parts) => {
            let texts = parts
                .iter()
                .map(|part| match part {
                    InputPart::Text(text) => Ok(text.as_str()),
                    InputPart::Image { .. } => {
                        Err("image input requires an openai_decisions channel".to_string())
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;
            Value::String(texts.join("\n\n"))
        }
    };
    let keys = system_one_keys(&request.questions)?;
    let mut questions = Map::new();
    for (key, question) in keys.into_iter().zip(&request.questions) {
        questions.insert(key, encode_question(question)?);
    }
    let mut body = Map::new();
    body.insert("state".to_string(), state);
    body.insert("questions".to_string(), Value::Object(questions));
    Ok(body)
}

fn encode_question(question: &Question) -> Result<Value, String> {
    let mut out = Map::new();
    out.insert("instructions".to_string(), question.instructions.clone());
    match &question.kind {
        QuestionKind::Binary {
            when_true,
            when_false,
        } => {
            out.insert("type".to_string(), Value::from("noul"));
            let mut criteria = Map::new();
            if let Some(value) = when_true {
                criteria.insert("true".to_string(), value.clone());
            }
            if let Some(value) = when_false {
                criteria.insert("false".to_string(), value.clone());
            }
            if !criteria.is_empty() {
                out.insert("criteria".to_string(), Value::Object(criteria));
            }
        }
        QuestionKind::Choice(options) => {
            out.insert("type".to_string(), Value::from("choice"));
            let mut criteria = Map::new();
            for option in options {
                let key = option.value.key();
                let description = option.description.clone().unwrap_or(Value::Null);
                if criteria.insert(key.clone(), description).is_some() {
                    return Err(format!("duplicate choice value: {key}"));
                }
            }
            out.insert("criteria".to_string(), Value::Object(criteria));
        }
        QuestionKind::Score(levels) => {
            out.insert("type".to_string(), Value::from("score"));
            let criteria = levels
                .iter()
                .map(|level| {
                    let label = text(&level.label);
                    Value::String(match &level.description {
                        Some(description) => format!("{label}: {description}"),
                        None => label,
                    })
                })
                .collect();
            out.insert("criteria".to_string(), Value::Array(criteria));
        }
    }
    Ok(Value::Object(out))
}

pub(super) fn decode_answers(request: &DecisionRequest, body: &Value) -> Result<Vec<Answer>, String> {
    let answers = body
        .get("answers")
        .and_then(Value::as_object)
        .ok_or("answers must be an object")?;
    let keys = system_one_keys(&request.questions)?;
    keys.iter()
        .zip(&request.questions)
        .map(|(key, question)| {
            let path = format!("answers.{key}");
            let answer = answers
                .get(key)
                .and_then(Value::as_object)
                .ok_or_else(|| format!("{path} is missing"))?;
            decode_answer(&path, question, answer)
        })
        .collect()
}

fn decode_answer(path: &str, question: &Question, answer: &Map<String, Value>) -> Result<Answer, String> {
    let answer_type = answer.get("type").and_then(Value::as_str);
    if answer_type == Some("refusal") {
        return Ok(Answer::Refusal);
    }
    let confidence = answer.get("confidence").and_then(Value::as_f64);
    let probabilities = answer.get("probabilities").and_then(Value::as_object);
    let probability_of = |key: &str| {
        probabilities
            .and_then(|probabilities| probabilities.get(key))
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
    };
    match (&question.kind, answer_type) {
        (QuestionKind::Binary { .. }, Some("noul")) => Ok(Answer::Binary {
            probability: number(answer, "noul", path)?,
        }),
        (QuestionKind::Choice(options), Some("choice")) => {
            let chosen = answer
                .get("choice")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{path}.choice must be a string"))?;
            let index = options
                .iter()
                .position(|option| option.value.key() == chosen)
                .ok_or_else(|| format!("{path}.choice {chosen} is not an option"))?;
            Ok(Answer::Choice {
                index,
                probabilities: options
                    .iter()
                    .map(|option| probability_of(&option.value.key()))
                    .collect(),
                confidence,
            })
        }
        (QuestionKind::Score(levels), Some("score")) => Ok(Answer::Score {
            score: number(answer, "score", path)?,
            probabilities: (0..levels.len())
                .map(|index| probability_of(&index.to_string()))
                .collect(),
            confidence,
        }),
        _ => Err(kind_mismatch(path, answer_type)),
    }
}

pub(super) fn encode_response(
    request: &DecisionRequest,
    answers: &[Answer],
    usage: &DecisionUsage,
    model: &str,
) -> Result<Value, String> {
    let keys = system_one_keys(&request.questions)?;
    let mut encoded = Map::new();
    for ((key, question), answer) in keys.into_iter().zip(&request.questions).zip(answers) {
        encoded.insert(key, encode_answer(question, answer));
    }
    Ok(serde_json::json!({
        "model": model,
        "answers": encoded,
        "usage": {
            "input_tokens": usage.input_tokens,
            "output_tokens": usage.output_tokens,
        },
    }))
}

fn encode_answer(question: &Question, answer: &Answer) -> Value {
    let mut out = Map::new();
    match (answer, &question.kind) {
        (Answer::Binary { probability }, _) => {
            out.insert("type".to_string(), Value::from("noul"));
            out.insert("noul".to_string(), Value::from(*probability));
        }
        (
            Answer::Choice {
                index,
                probabilities,
                confidence,
            },
            QuestionKind::Choice(options),
        ) => {
            out.insert("type".to_string(), Value::from("choice"));
            out.insert("choice".to_string(), Value::from(options[*index].value.key()));
            let probabilities = options
                .iter()
                .zip(probabilities)
                .map(|(option, probability)| (option.value.key(), Value::from(*probability)))
                .collect();
            out.insert("probabilities".to_string(), Value::Object(probabilities));
            insert_confidence(&mut out, *confidence);
        }
        (
            Answer::Score {
                score,
                probabilities,
                confidence,
            },
            QuestionKind::Score(levels),
        ) => {
            out.insert("type".to_string(), Value::from("score"));
            out.insert("score".to_string(), Value::from(*score));
            let legend = levels
                .iter()
                .enumerate()
                .map(|(index, level)| (index.to_string(), Value::String(text(&level.label))))
                .collect();
            out.insert("legend".to_string(), Value::Object(legend));
            let probabilities = probabilities
                .iter()
                .enumerate()
                .map(|(index, probability)| (index.to_string(), Value::from(*probability)))
                .collect();
            out.insert("probabilities".to_string(), Value::Object(probabilities));
            insert_confidence(&mut out, *confidence);
        }
        // Decoders pair Choice and Score answers only with matching question
        // kinds, so the mismatched pairs in this arm cannot occur.
        (Answer::Refusal, _) | (Answer::Choice { .. } | Answer::Score { .. }, _) => {
            out.insert("type".to_string(), Value::from("refusal"));
        }
    }
    Value::Object(out)
}

//! OpenAI Decisions wire format (`POST /v1/decisions`).

use super::*;

pub(super) fn validate_shape(body: &Map<String, Value>) -> Result<(), String> {
    match body.get("input") {
        Some(Value::String(_) | Value::Array(_)) => {}
        Some(_) => return Err("input must be a string or an array".to_string()),
        None => return Err("missing input".to_string()),
    }
    match body.get("questions") {
        Some(Value::Array(questions)) if !questions.is_empty() => Ok(()),
        _ => Err("questions must be a non-empty array".to_string()),
    }
}

pub(super) fn decode_request(body: &Map<String, Value>) -> Result<DecisionRequest, String> {
    let input = match body.get("input") {
        Some(Value::String(text)) => DecisionInput::Text(text.clone()),
        Some(Value::Array(messages)) => DecisionInput::Parts(decode_messages(messages)?),
        _ => return Err("input must be a string or an array".to_string()),
    };
    let Some(Value::Array(entries)) = body.get("questions") else {
        return Err("questions must be a non-empty array".to_string());
    };
    let questions = entries
        .iter()
        .enumerate()
        .map(|(index, question)| decode_question(&format!("questions[{index}]"), question))
        .collect::<Result<_, _>>()?;
    Ok(DecisionRequest { input, questions })
}

fn decode_messages(messages: &[Value]) -> Result<Vec<InputPart>, String> {
    let mut parts = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        let path = format!("input[{index}]");
        let message = message
            .as_object()
            .ok_or_else(|| format!("{path} must be an object"))?;
        if message
            .get("type")
            .is_some_and(|value| value.as_str() != Some("message"))
        {
            return Err(format!("{path}.type must be message"));
        }
        if message
            .get("role")
            .is_some_and(|value| value.as_str() != Some("user"))
        {
            return Err(format!("{path}.role must be user"));
        }
        match message.get("content") {
            Some(Value::String(text)) => parts.push(InputPart::Text(text.clone())),
            Some(Value::Array(content)) => {
                for (part_index, part) in content.iter().enumerate() {
                    parts.push(decode_part(&format!("{path}.content[{part_index}]"), part)?);
                }
            }
            _ => return Err(format!("{path}.content must be a string or an array")),
        }
    }
    Ok(parts)
}

fn decode_part(path: &str, part: &Value) -> Result<InputPart, String> {
    let string_field = |field: &str| {
        part.get(field)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("{path}.{field} must be a string"))
    };
    match part.get("type").and_then(Value::as_str) {
        Some("input_text") => Ok(InputPart::Text(string_field("text")?)),
        Some("input_image") => Ok(InputPart::Image {
            image_url: string_field("image_url")?,
            detail: part.get("detail").and_then(Value::as_str).map(str::to_string),
        }),
        _ => Err(format!("{path}.type must be input_text or input_image")),
    }
}

fn decode_question(path: &str, value: &Value) -> Result<Question, String> {
    let question = value
        .as_object()
        .ok_or_else(|| format!("{path} must be an object"))?;
    let instructions = match question.get("instructions") {
        Some(Value::String(text)) => Value::String(text.clone()),
        _ => return Err(format!("{path}.instructions must be a string")),
    };
    let name = match question.get("name") {
        None | Some(Value::Null) => None,
        Some(Value::String(name)) => Some(name.clone()),
        Some(_) => return Err(format!("{path}.name must be a string")),
    };
    let entries = |field: &str| match question.get(field) {
        Some(Value::Array(entries)) => Ok(entries),
        _ => Err(format!("{path}.{field} must be an array")),
    };
    let kind = match question.get("type").and_then(Value::as_str) {
        Some("predicate") => QuestionKind::Binary {
            when_true: None,
            when_false: None,
        },
        Some("choice") => QuestionKind::Choice(
            entries("choices")?
                .iter()
                .enumerate()
                .map(|(index, choice)| {
                    let value = choice
                        .get("value")
                        .and_then(ChoiceValue::from_json)
                        .ok_or_else(|| {
                            format!("{path}.choices[{index}].value must be a string or boolean")
                        })?;
                    Ok(ChoiceOption {
                        value,
                        description: choice.get("description").filter(|d| !d.is_null()).cloned(),
                    })
                })
                .collect::<Result<_, String>>()?,
        ),
        Some("score") => QuestionKind::Score(
            entries("levels")?
                .iter()
                .enumerate()
                .map(|(index, level)| {
                    let label = level
                        .get("label")
                        .cloned()
                        .ok_or_else(|| format!("{path}.levels[{index}].label is required"))?;
                    Ok(ScoreLevel {
                        label,
                        description: level
                            .get("description")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    })
                })
                .collect::<Result<_, String>>()?,
        ),
        _ => return Err(format!("{path}.type must be predicate, choice, or score")),
    };
    Ok(Question {
        name,
        instructions,
        kind,
    })
}

pub(super) fn encode_request(request: &DecisionRequest) -> Result<Map<String, Value>, String> {
    let input = match &request.input {
        DecisionInput::Text(text) => Value::String(text.clone()),
        DecisionInput::Structured(value) => Value::String(text(value)),
        DecisionInput::Parts(parts) => {
            let content = parts.iter().map(encode_part).collect();
            serde_json::json!([{ "role": "user", "content": Value::Array(content) }])
        }
    };
    let questions = request.questions.iter().map(encode_question).collect();
    let mut body = Map::new();
    body.insert("input".to_string(), input);
    body.insert("questions".to_string(), Value::Array(questions));
    Ok(body)
}

fn encode_part(part: &InputPart) -> Value {
    match part {
        InputPart::Text(text) => serde_json::json!({ "type": "input_text", "text": text }),
        InputPart::Image { image_url, detail } => {
            let mut out = serde_json::json!({ "type": "input_image", "image_url": image_url });
            if let Some(detail) = detail {
                out["detail"] = Value::from(detail.as_str());
            }
            out
        }
    }
}

fn encode_question(question: &Question) -> Value {
    let mut out = Map::new();
    if let Some(name) = &question.name {
        out.insert("name".to_string(), Value::from(name.as_str()));
    }
    let mut instructions = text(&question.instructions);
    match &question.kind {
        QuestionKind::Binary {
            when_true,
            when_false,
        } => {
            out.insert("type".to_string(), Value::from("predicate"));
            // DR-B22: predicates have no criteria field, so the criteria
            // travel inside the instructions.
            if let Some(value) = when_true {
                instructions.push_str(&format!("\n\nAnswer true when: {}", text(value)));
            }
            if let Some(value) = when_false {
                instructions.push_str(&format!("\n\nAnswer false when: {}", text(value)));
            }
        }
        QuestionKind::Choice(options) => {
            out.insert("type".to_string(), Value::from("choice"));
            let choices = options
                .iter()
                .map(|option| {
                    let mut choice = Map::new();
                    choice.insert("value".to_string(), option.value.to_json());
                    if let Some(description) = &option.description {
                        choice.insert("description".to_string(), Value::String(text(description)));
                    }
                    Value::Object(choice)
                })
                .collect();
            out.insert("choices".to_string(), Value::Array(choices));
        }
        QuestionKind::Score(levels) => {
            out.insert("type".to_string(), Value::from("score"));
            let levels = levels
                .iter()
                .map(|level| {
                    let mut out = Map::new();
                    out.insert("label".to_string(), Value::String(text(&level.label)));
                    if let Some(description) = &level.description {
                        out.insert("description".to_string(), Value::from(description.as_str()));
                    }
                    Value::Object(out)
                })
                .collect();
            out.insert("levels".to_string(), Value::Array(levels));
        }
    }
    out.insert("instructions".to_string(), Value::String(instructions));
    Value::Object(out)
}

pub(super) fn decode_answers(request: &DecisionRequest, body: &Value) -> Result<Vec<Answer>, String> {
    let answers = body
        .get("answers")
        .and_then(Value::as_array)
        .ok_or("answers must be an array")?;
    if answers.len() != request.questions.len() {
        return Err(format!(
            "answers has {} entries for {} questions",
            answers.len(),
            request.questions.len()
        ));
    }
    request
        .questions
        .iter()
        .zip(answers)
        .enumerate()
        .map(|(index, (question, answer))| {
            let path = format!("answers[{index}]");
            let answer = answer
                .as_object()
                .ok_or_else(|| format!("{path} must be an object"))?;
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
    let entries = answer
        .get("probabilities")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let probability_where = |matches: &dyn Fn(&Value) -> bool| {
        entries
            .iter()
            .find(|entry| entry.get("value").is_some_and(matches))
            .and_then(|entry| entry.get("probability"))
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
    };
    match (&question.kind, answer_type) {
        (QuestionKind::Binary { .. }, Some("predicate")) => Ok(Answer::Binary {
            probability: number(answer, "probability", path)?,
        }),
        (QuestionKind::Choice(options), Some("choice")) => {
            let chosen = answer
                .get("choice")
                .and_then(ChoiceValue::from_json)
                .ok_or_else(|| format!("{path}.choice must be a string or boolean"))?;
            let index = options
                .iter()
                .position(|option| option.value == chosen)
                .ok_or_else(|| format!("{path}.choice {} is not an option", chosen.key()))?;
            Ok(Answer::Choice {
                index,
                probabilities: options
                    .iter()
                    .map(|option| {
                        probability_where(&|value| {
                            ChoiceValue::from_json(value).as_ref() == Some(&option.value)
                        })
                    })
                    .collect(),
                confidence,
            })
        }
        (QuestionKind::Score(levels), Some("score")) => Ok(Answer::Score {
            score: number(answer, "score", path)?,
            probabilities: (0..levels.len())
                .map(|index| probability_where(&|value| value.as_u64() == Some(index as u64)))
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
) -> Value {
    let answers: Vec<Value> = request
        .questions
        .iter()
        .zip(answers)
        .map(|(question, answer)| encode_answer(question, answer))
        .collect();
    serde_json::json!({
        "model": model,
        "answers": answers,
        "usage": {
            "input_tokens": usage.input_tokens,
            "input_tokens_details": {
                "cached_tokens": usage.cached_tokens,
                "cache_write_tokens": usage.cache_write_tokens,
            },
            "output_tokens": usage.output_tokens,
            "output_tokens_details": { "reasoning_tokens": 0 },
            "total_tokens": usage.input_tokens + usage.output_tokens,
        },
    })
}

fn encode_answer(question: &Question, answer: &Answer) -> Value {
    let mut out = Map::new();
    match (answer, &question.kind) {
        (Answer::Binary { probability }, _) => {
            out.insert("type".to_string(), Value::from("predicate"));
            out.insert("probability".to_string(), Value::from(*probability));
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
            out.insert("choice".to_string(), options[*index].value.to_json());
            let probabilities = options
                .iter()
                .zip(probabilities)
                .map(|(option, probability)| {
                    serde_json::json!({ "value": option.value.to_json(), "probability": probability })
                })
                .collect();
            out.insert("probabilities".to_string(), Value::Array(probabilities));
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
            let probabilities = levels
                .iter()
                .zip(probabilities)
                .enumerate()
                .map(|(index, (level, probability))| {
                    serde_json::json!({
                        "value": index,
                        "label": text(&level.label),
                        "probability": probability,
                    })
                })
                .collect();
            out.insert("probabilities".to_string(), Value::Array(probabilities));
            insert_confidence(&mut out, *confidence);
        }
        // Decoders pair Choice and Score answers only with matching question
        // kinds, so the mismatched pairs in this arm cannot occur.
        (Answer::Refusal, _) | (Answer::Choice { .. } | Answer::Score { .. }, _) => {
            out.insert("type".to_string(), Value::from("refusal"));
        }
    }
    if let Some(name) = &question.name {
        out.insert("name".to_string(), Value::from(name.as_str()));
    }
    Value::Object(out)
}

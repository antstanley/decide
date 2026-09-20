//! The API's vocabulary: the request and response shapes, the documented limits, and
//! validation.
//!
//! This is the only module that knows the documented shapes
//! ([`docs/api.md`](../docs/api.md)), which is why it is also where the limits live. A
//! limit is never changed in one place only: the constant here, the table in
//! `docs/api.md`, and the boundary test move together.
//!
//! The two JSON values have different authors, and therefore opposite rules
//! ([D8](../docs/design.md#d8-the-response-is-parsed-tolerantly-the-document-strictly)):
//! [`Document::from_value`] refuses an unknown key anywhere, because the caller wrote it and
//! is in the same room as the fix; [`Response::from_value`] ignores what it does not know,
//! because `TypeSafe` owns that value and a deployment can gain a field at any time.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::error::DecideError;

/// The most options a `choice` may carry
/// ([`docs/api.md#limits`](../docs/api.md#limits)).
pub const MAX_CHOICE_OPTIONS: usize = 255;

/// The fewest levels a `score` may carry.
pub const MIN_SCORE_LEVELS: usize = 2;

/// The most levels a `score` may carry.
pub const MAX_SCORE_LEVELS: usize = 10;

/// The model a request names when neither `--model` nor the document does.
pub const DEFAULT_MODEL: &str = "jev-latest";

/// The API root every call is built on, when nothing names another one.
///
/// It is spelled as the evaluation endpoint rather than as the bare host, because that is
/// the URL a caller reads out of the API documentation and pastes into `--base-url`;
/// [`crate::input::resolve_base_url`] reduces either spelling to the root the two calls are
/// built from.
pub const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai/v1/systemone";

/// The keys a request document may carry, as the error message lists them.
const REQUEST_KEYS: &str = "\"state\", \"model\", \"questions\"";

/// The keys a question may carry, as the error message lists them.
const QUESTION_KEYS: &str = "\"type\", \"instructions\", \"criteria\"";

/// One of the three question types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestionKind {
    /// A yes/no question, answered with the probability that the answer is yes.
    Noul,
    /// A question with a set of options to pick from.
    Choice,
    /// A question with ordered levels to rate along.
    Score,
}

impl QuestionKind {
    /// The `type` string this kind is sent and answered as.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Noul => "noul",
            Self::Choice => "choice",
            Self::Score => "score",
        }
    }

    /// The kind a `type` string names, or `None` if it names none of the three.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "noul" => Some(Self::Noul),
            "choice" => Some(Self::Choice),
            "score" => Some(Self::Score),
            _ => None,
        }
    }
}

/// A question's `criteria`, which is shaped differently by each question type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Criteria {
    /// A noul's optional `true`/`false` descriptions.
    Noul(BTreeMap<String, Value>),
    /// A choice's named options, each with its description or `null`.
    Choice(BTreeMap<String, Value>),
    /// A score's ordered levels, first to last.
    Score(Vec<Value>),
}

impl Criteria {
    /// The criteria as JSON, which is what goes on the wire.
    #[must_use]
    pub fn to_value(&self) -> Value {
        match self {
            Self::Noul(map) | Self::Choice(map) => {
                let mut out = Map::new();
                for (key, value) in map {
                    out.insert(key.clone(), value.clone());
                }
                Value::Object(out)
            }
            Self::Score(levels) => Value::Array(levels.clone()),
        }
    }

    /// How many entries the criteria has: the noul descriptions, the options, or the
    /// levels.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Noul(map) | Self::Choice(map) => map.len(),
            Self::Score(levels) => levels.len(),
        }
    }

    /// Whether there are no entries at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// One typed question, with its instructions and its criteria.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    /// Which of the three shapes this question takes.
    pub kind: QuestionKind,
    /// What the model is asked. A string, an object, or an array.
    pub instructions: Value,
    /// The criteria, which a `choice` and a `score` require and a `noul` may omit.
    pub criteria: Option<Criteria>,
}

impl Question {
    /// A noul question, with optional `true`/`false` descriptions.
    #[must_use]
    pub fn noul(instructions: Value, criteria: Option<BTreeMap<String, Value>>) -> Self {
        Self {
            kind: QuestionKind::Noul,
            instructions,
            criteria: criteria.map(Criteria::Noul),
        }
    }

    /// A choice question over the options given.
    #[must_use]
    pub fn choice(instructions: Value, criteria: BTreeMap<String, Value>) -> Self {
        Self {
            kind: QuestionKind::Choice,
            instructions,
            criteria: Some(Criteria::Choice(criteria)),
        }
    }

    /// A score question over the levels given, in ascending order.
    #[must_use]
    pub fn score(instructions: Value, criteria: Vec<Value>) -> Self {
        Self {
            kind: QuestionKind::Score,
            instructions,
            criteria: Some(Criteria::Score(criteria)),
        }
    }

    /// The question as JSON, which is what goes on the wire.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert(
            "type".to_string(),
            Value::String(self.kind.as_str().to_string()),
        );
        map.insert("instructions".to_string(), self.instructions.clone());
        if let Some(criteria) = &self.criteria {
            map.insert("criteria".to_string(), criteria.to_value());
        }
        Value::Object(map)
    }

    /// Check the documented limits this question cannot break, naming `id` in every
    /// message.
    pub fn validate(&self, id: &str) -> Result<(), DecideError> {
        let location = format!("question \"{id}\"");

        if matches!(&self.instructions, Value::String(text) if text.trim().is_empty()) {
            return Err(invalid(
                &location,
                "the instruction is empty; a question with no instruction asks nothing",
            ));
        }

        match self.kind {
            QuestionKind::Noul => validate_noul(&location, self.criteria.as_ref()),
            QuestionKind::Choice => validate_choice(&location, self.criteria.as_ref()),
            QuestionKind::Score => validate_score(&location, self.criteria.as_ref()),
        }
    }
}

/// A noul may carry no criteria at all, and the criteria it does carry uses two keys.
fn validate_noul(location: &str, criteria: Option<&Criteria>) -> Result<(), DecideError> {
    match criteria {
        None => Ok(()),
        Some(Criteria::Noul(map)) => {
            for key in map.keys() {
                if key != "true" && key != "false" {
                    return Err(invalid(
                        location,
                        &format!(
                            "unknown criteria key \"{key}\"; a noul's criteria has only the \
                             keys \"true\" and \"false\""
                        ),
                    ));
                }
            }
            Ok(())
        }
        Some(_) => Err(invalid(
            location,
            "a noul's criteria is an object keyed \"true\" and \"false\"",
        )),
    }
}

/// A choice picks from at least one option and at most [`MAX_CHOICE_OPTIONS`].
fn validate_choice(location: &str, criteria: Option<&Criteria>) -> Result<(), DecideError> {
    let Some(Criteria::Choice(options)) = criteria else {
        return Err(invalid(
            location,
            "a choice needs \"criteria\": a map of options to choose from",
        ));
    };

    let count = options.len();
    if count == 0 {
        return Err(invalid(
            location,
            "a choice needs at least one option (none were given)",
        ));
    }
    if count > MAX_CHOICE_OPTIONS {
        return Err(invalid(
            location,
            &format!("a choice has at most {MAX_CHOICE_OPTIONS} options ({count} were given)"),
        ));
    }
    Ok(())
}

/// A score rates along at least two levels and at most [`MAX_SCORE_LEVELS`].
fn validate_score(location: &str, criteria: Option<&Criteria>) -> Result<(), DecideError> {
    let Some(Criteria::Score(levels)) = criteria else {
        return Err(invalid(
            location,
            "a score needs \"criteria\": an array of levels from first to last",
        ));
    };

    let count = levels.len();
    if count < MIN_SCORE_LEVELS {
        return Err(invalid(
            location,
            &format!("a score needs at least {MIN_SCORE_LEVELS} levels ({count} were given)"),
        ));
    }
    if count > MAX_SCORE_LEVELS {
        return Err(invalid(
            location,
            &format!("a score has at most {MAX_SCORE_LEVELS} levels ({count} were given)"),
        ));
    }
    Ok(())
}

/// A parsed request document, before the state and the model are resolved.
///
/// `state` is `Option<Value>` rather than `Value` because `Some(Value::Null)` — an explicit
/// `"state": null` — is a different fact from a document that omits the key, and
/// [D7](../docs/design.md#d7-the-state-comes-from-exactly-one-place) turns on exactly that
/// difference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    /// The document's own state, if it carried one.
    pub state: Option<Value>,
    /// The document's model, if it named one.
    pub model: Option<String>,
    /// The questions, keyed by the caller's ids.
    pub questions: BTreeMap<String, Question>,
}

impl Document {
    /// Parse a request document from bytes, strictly.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, DecideError> {
        let value: Value =
            serde_json::from_slice(bytes).map_err(|error| DecideError::DocumentNotJson {
                reason: error.to_string(),
            })?;
        Self::from_value(&value)
    }

    /// Validate a request document that has already been parsed as JSON, strictly.
    pub fn from_value(value: &Value) -> Result<Self, DecideError> {
        let Value::Object(map) = value else {
            return Err(DecideError::DocumentNotObject);
        };

        for key in map.keys() {
            if !matches!(key.as_str(), "state" | "model" | "questions") {
                return Err(unknown_key("the request document", key, REQUEST_KEYS));
            }
        }

        let questions = match map.get("questions") {
            Some(value) => parse_questions(value)?,
            None => {
                return Err(invalid(
                    "the request document",
                    "there is no \"questions\" key; a request must ask at least one question",
                ));
            }
        };

        let model = match map.get("model") {
            None => None,
            Some(Value::String(name)) => Some(name.clone()),
            Some(other) => {
                return Err(invalid(
                    "the request document",
                    &format!("\"model\" is {}; it must be a string", json_kind(other)),
                ));
            }
        };

        Ok(Self {
            state: map.get("state").cloned(),
            model,
            questions,
        })
    }
}

/// Parse the `questions` object, strictly.
fn parse_questions(value: &Value) -> Result<BTreeMap<String, Question>, DecideError> {
    let Value::Object(map) = value else {
        return Err(invalid(
            "the request document",
            &format!(
                "\"questions\" is {}; it must be an object of question ids",
                json_kind(value)
            ),
        ));
    };

    if map.is_empty() {
        return Err(invalid(
            "the request document",
            "\"questions\" is empty; a request must ask at least one question",
        ));
    }

    let mut questions = BTreeMap::new();
    for (id, question) in map {
        questions.insert(id.clone(), parse_question(id, question)?);
    }
    Ok(questions)
}

/// Parse one question object, strictly: an unknown key names itself and the question.
fn parse_question(id: &str, value: &Value) -> Result<Question, DecideError> {
    let location = format!("question \"{id}\"");

    let Value::Object(map) = value else {
        return Err(invalid(
            &location,
            &format!("the question is {}; it must be an object", json_kind(value)),
        ));
    };

    for key in map.keys() {
        if !matches!(key.as_str(), "type" | "instructions" | "criteria") {
            return Err(unknown_key(&location, key, QUESTION_KEYS));
        }
    }

    let kind = parse_kind(&location, map.get("type"))?;

    let instructions = map.get("instructions").ok_or_else(|| {
        invalid(
            &location,
            "there is no \"instructions\"; a question with no instruction asks nothing",
        )
    })?;

    let criteria = parse_criteria(&location, kind, map.get("criteria"))?;

    Ok(Question {
        kind,
        instructions: instructions.clone(),
        criteria,
    })
}

/// Parse a question's `type`, refusing anything that is not one of the three by name.
fn parse_kind(location: &str, value: Option<&Value>) -> Result<QuestionKind, DecideError> {
    let named = |value: &Value| {
        invalid(
            location,
            &format!(
                "\"type\" is {}; it must be one of \"noul\", \"choice\", or \"score\"",
                json_kind(value)
            ),
        )
    };

    match value {
        Some(Value::String(name)) => QuestionKind::parse(name).ok_or_else(|| {
            invalid(
                location,
                &format!(
                    "\"type\" is \"{name}\"; it must be one of \"noul\", \"choice\", or \
                     \"score\""
                ),
            )
        }),
        Some(other) => Err(named(other)),
        None => Err(invalid(
            location,
            "there is no \"type\"; it must be one of \"noul\", \"choice\", or \"score\"",
        )),
    }
}

/// Parse a question's criteria, into the shape its type calls for.
fn parse_criteria(
    location: &str,
    kind: QuestionKind,
    value: Option<&Value>,
) -> Result<Option<Criteria>, DecideError> {
    match (kind, value) {
        (QuestionKind::Noul, None) => Ok(None),
        (QuestionKind::Noul, Some(value)) => Ok(Some(parse_noul_criteria(location, value)?)),
        (QuestionKind::Choice, Some(value)) => Ok(Some(parse_choice_criteria(location, value)?)),
        (QuestionKind::Score, Some(value)) => Ok(Some(parse_score_criteria(location, value)?)),
        (QuestionKind::Choice, None) => Err(invalid(
            location,
            "a choice needs \"criteria\": a map of options to choose from",
        )),
        (QuestionKind::Score, None) => Err(invalid(
            location,
            "a score needs \"criteria\": an array of levels from first to last",
        )),
    }
}

/// Parse a noul's criteria into a map, leaving the key set to `validate`.
fn parse_noul_criteria(location: &str, value: &Value) -> Result<Criteria, DecideError> {
    let Value::Object(map) = value else {
        return Err(invalid(
            location,
            &format!(
                "a noul's \"criteria\" is {}; it must be an object keyed \"true\" and \"false\"",
                json_kind(value)
            ),
        ));
    };
    Ok(Criteria::Noul(map.clone().into_iter().collect()))
}

/// Parse a choice's criteria into a map of option name to description.
fn parse_choice_criteria(location: &str, value: &Value) -> Result<Criteria, DecideError> {
    let Value::Object(map) = value else {
        return Err(invalid(
            location,
            &format!(
                "a choice's \"criteria\" is {}; it must be an object of options",
                json_kind(value)
            ),
        ));
    };
    Ok(Criteria::Choice(map.clone().into_iter().collect()))
}

/// Parse a score's criteria into the ordered list of levels.
fn parse_score_criteria(location: &str, value: &Value) -> Result<Criteria, DecideError> {
    let Value::Array(levels) = value else {
        return Err(invalid(
            location,
            &format!(
                "a score's \"criteria\" is {}; it must be an array of levels",
                json_kind(value)
            ),
        ));
    };
    Ok(Criteria::Score(levels.clone()))
}

/// One request, ready to be sent: the state, the model, and the questions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// What the questions are about. May be `null`.
    pub state: Value,
    /// The model or alias that handles the call.
    pub model: String,
    /// The questions, keyed by the caller's ids.
    pub questions: BTreeMap<String, Question>,
}

impl Request {
    /// Assemble a request and check every documented limit it could break.
    pub fn new(
        state: Value,
        model: String,
        questions: BTreeMap<String, Question>,
    ) -> Result<Self, DecideError> {
        let request = Self {
            state,
            model,
            questions,
        };
        request.validate()?;
        Ok(request)
    }

    /// Check every limit that can be checked without a tokenizer.
    pub fn validate(&self) -> Result<(), DecideError> {
        check_state_kind(&self.state)?;
        if self.questions.is_empty() {
            return Err(invalid(
                "the request document",
                "\"questions\" is empty; a request must ask at least one question",
            ));
        }
        for (id, question) in &self.questions {
            if id.is_empty() {
                return Err(invalid(
                    "the request document",
                    "a question id cannot be empty, because an empty id cannot be addressed",
                ));
            }
            question.validate(id)?;
        }
        Ok(())
    }

    /// The request as JSON, which is what goes on the wire and what `--dry-run` prints.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut questions = Map::new();
        for (id, question) in &self.questions {
            questions.insert(id.clone(), question.to_value());
        }
        let mut map = Map::new();
        map.insert("state".to_string(), self.state.clone());
        map.insert("model".to_string(), Value::String(self.model.clone()));
        map.insert("questions".to_string(), Value::Object(questions));
        Value::Object(map)
    }

    /// How many questions the request asks.
    #[must_use]
    pub fn question_count(&self) -> usize {
        self.questions.len()
    }

    /// The id of the single question, when the request asked exactly one.
    #[must_use]
    pub fn only_question_id(&self) -> Option<&str> {
        if self.questions.len() == 1 {
            self.questions.keys().next().map(String::as_str)
        } else {
            None
        }
    }
}

/// One answer, with the type it claims and the object it arrived in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// The answer's own type, which must agree with its question.
    pub kind: QuestionKind,
    /// The answer object, whole, for `--field` to walk.
    pub value: Value,
}

impl Answer {
    /// The answer's own value: `noul`, `choice`, or `score`, whichever its type has.
    #[must_use]
    pub fn own_value(&self) -> Option<&Value> {
        self.value.get(self.kind.as_str())
    }
}

/// A parsed response, held together with the questions it answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// The body, as a value, for printing and for `--select`.
    pub value: Value,
    /// The versioned model id that answered, when the body named one.
    pub model: Option<String>,
    /// The answers, keyed by the ids the request sent.
    pub answers: BTreeMap<String, Answer>,
}

impl Response {
    /// Parse a response body against the request it answers, tolerantly.
    pub fn from_slice(bytes: &[u8], request: &Request) -> Result<Self, DecideError> {
        let value: Value =
            serde_json::from_slice(bytes).map_err(|error| DecideError::ResponseNotJson {
                reason: error.to_string(),
            })?;
        Self::from_value(value, request)
    }

    /// Validate a response that has already been parsed as JSON, tolerantly.
    ///
    /// Unknown fields are ignored
    /// ([D8](../docs/design.md#d8-the-response-is-parsed-tolerantly-the-document-strictly));
    /// what is checked is the answer contract: every question has an answer, and every
    /// answer's type agrees with its question.
    pub fn from_value(value: Value, request: &Request) -> Result<Self, DecideError> {
        let Value::Object(map) = &value else {
            return Err(contract(&format!(
                "the body is {}, not an object",
                json_kind(&value)
            )));
        };

        let answers_value = map.get("answers").ok_or_else(|| {
            contract("there is no \"answers\" object, so no question was answered")
        })?;
        let Value::Object(answer_map) = answers_value else {
            return Err(contract(&format!(
                "\"answers\" is {}, not an object",
                json_kind(answers_value)
            )));
        };

        let mut answers = BTreeMap::new();
        for (id, question) in &request.questions {
            let answer = answer_map
                .get(id)
                .ok_or_else(|| contract(&format!("there is no answer for question \"{id}\"")))?;
            answers.insert(id.clone(), parse_answer(id, answer, question.kind)?);
        }

        let model = match map.get("model") {
            Some(Value::String(name)) => Some(name.clone()),
            _ => None,
        };

        Ok(Self {
            value,
            model,
            answers,
        })
    }

    /// The token usage the body reported, if it reported one this client understands.
    #[must_use]
    pub fn usage(&self) -> Option<Usage> {
        let usage = self.value.get("usage")?;
        Some(Usage {
            input_tokens: usage.get("input_tokens")?.as_u64()?,
            output_tokens: usage.get("output_tokens")?.as_u64()?,
        })
    }
}

/// Check one answer against the question it claims to answer.
fn parse_answer(id: &str, value: &Value, kind: QuestionKind) -> Result<Answer, DecideError> {
    let Value::Object(map) = value else {
        return Err(contract(&format!(
            "the answer for question \"{id}\" is {}, not an object",
            json_kind(value)
        )));
    };

    let found = match map.get("type") {
        Some(Value::String(name)) => name.as_str(),
        _ => {
            return Err(contract(&format!(
                "the answer for question \"{id}\" has no \"type\""
            )));
        }
    };

    if QuestionKind::parse(found) != Some(kind) {
        return Err(contract(&format!(
            "question \"{id}\" asked for a {}, but the response answered with \"{found}\"",
            kind.as_str()
        )));
    }

    let own = map.get(kind.as_str()).ok_or_else(|| {
        contract(&format!(
            "the answer for question \"{id}\" has no \"{}\"",
            kind.as_str()
        ))
    })?;

    let (shape_is_right, wanted) = match kind {
        QuestionKind::Noul | QuestionKind::Score => (own.is_number(), "a number"),
        QuestionKind::Choice => (own.is_string(), "a string"),
    };
    if !shape_is_right {
        return Err(contract(&format!(
            "the answer for question \"{id}\": \"{}\" is {}, not {wanted}",
            kind.as_str(),
            json_kind(own)
        )));
    }

    Ok(Answer {
        kind,
        value: value.clone(),
    })
}

/// Token accounting, as the response reports it.
///
/// Nothing in this program thresholds it; it is here because `--verbose` reports it and
/// because a caller selecting `usage.input_tokens` should be reading a documented field
/// rather than a string it guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    /// What the request cost.
    pub input_tokens: u64,
    /// What the answer cost.
    pub output_tokens: u64,
}

/// One model in the `GET /v1/models` list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    /// The alias, such as `jev-latest`.
    pub name: String,
    /// What it is for, if the API said.
    pub description: Option<String>,
    /// When it was released, if the API said.
    pub release_date: Option<String>,
}

/// The `GET /v1/models` body, printed whole and viewed through [`Model`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelsResponse {
    /// The body, as a value, for printing and for `--select`.
    pub value: Value,
}

impl ModelsResponse {
    /// Parse a models body. Only valid JSON is required; the entries are tolerant.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, DecideError> {
        let value: Value =
            serde_json::from_slice(bytes).map_err(|error| DecideError::ResponseNotJson {
                reason: error.to_string(),
            })?;
        Ok(Self { value })
    }

    /// Whether the body lists any model at all.
    ///
    /// JSON is not enough to call a response an answer: a proxy, a captive portal, or a
    /// base URL pointing at some other service can answer `200` with a body of its own, and
    /// the tolerant parse above would accept it. What this requires is the shape the
    /// documentation shows — a `models` array holding at least one entry with a `name` —
    /// and no more than that, so an extra field is still ignored.
    #[must_use]
    pub fn lists_models(&self) -> bool {
        !self.models().is_empty()
    }

    /// The models the body lists, skipping any entry that is not an object with a name.
    #[must_use]
    pub fn models(&self) -> Vec<Model> {
        let Some(Value::Array(entries)) = self.value.get("models") else {
            return Vec::new();
        };
        entries
            .iter()
            .filter_map(|entry| {
                let name = entry.get("name")?.as_str()?.to_string();
                Some(Model {
                    name,
                    description: entry
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    release_date: entry
                        .get("release_date")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                })
            })
            .collect()
    }
}

/// Refuse a state that is neither a string, an object, an array, nor `null`.
pub fn check_state_kind(state: &Value) -> Result<(), DecideError> {
    match state {
        Value::Number(_) => Err(DecideError::StateKindUnsupported { kind: "a number" }),
        Value::Bool(_) => Err(DecideError::StateKindUnsupported { kind: "a boolean" }),
        _ => Ok(()),
    }
}

/// The words for a JSON value's kind, for a message that has to name it.
#[must_use]
pub const fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// An error naming an unknown key, where it is, and what was expected there.
fn unknown_key(location: &str, key: &str, expected: &str) -> DecideError {
    invalid(
        location,
        &format!("unknown key \"{key}\"; expected one of {expected}"),
    )
}

/// An error about something in the document, located for the caller.
fn invalid(location: &str, problem: &str) -> DecideError {
    DecideError::Invalid {
        location: location.to_string(),
        problem: problem.to_string(),
    }
}

/// An error about the response breaking the answer contract.
fn contract(problem: &str) -> DecideError {
    DecideError::ResponseContract {
        problem: problem.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The vendor's quickstart document, copied from the documentation.
    const QUICKSTART: &str = include_str!("../tests/data/request_quickstart.json");
    /// The vendor's three-answer response, copied from the documentation.
    const ANSWERS: &str = include_str!("../tests/data/response_answers.json");

    /// Parse a document from a JSON literal.
    fn document(json: &str) -> Result<Document, DecideError> {
        Document::from_slice(json.as_bytes())
    }

    /// Parse a document and assemble the request it describes, limits included.
    fn request(json: &str) -> Result<Request, DecideError> {
        let document = document(json)?;
        Request::new(
            document.state.unwrap_or(Value::Null),
            document.model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            document.questions,
        )
    }

    /// A one-question request, for the answer-contract tests.
    fn noul_request() -> Request {
        let mut questions = BTreeMap::new();
        questions.insert(
            "is_urgent".to_string(),
            Question::noul(Value::String("urgent?".to_string()), None),
        );
        Request::new(Value::Null, DEFAULT_MODEL.to_string(), questions).expect("a valid request")
    }

    /// A choice question over `count` generated options.
    fn choice_of(count: usize) -> Question {
        let mut options = BTreeMap::new();
        for index in 0..count {
            options.insert(format!("option{index}"), Value::Null);
        }
        Question::choice(Value::String("which one?".to_string()), options)
    }

    /// A score question over `count` generated levels.
    fn score_of(count: usize) -> Question {
        let levels = (0..count)
            .map(|index| Value::String(format!("level{index}")))
            .collect();
        Question::score(Value::String("how much?".to_string()), levels)
    }

    /// The request the quickstart document describes.
    fn quickstart_request() -> Request {
        let document = document(QUICKSTART).expect("the vendor's document parses");
        Request::new(
            document.state.clone().unwrap_or(Value::Null),
            "jev-latest".to_string(),
            document.questions,
        )
        .expect("the vendor's document is a valid request")
    }

    #[test]
    fn the_quickstart_document_becomes_the_request_that_was_written() {
        let expected: Value = serde_json::from_str(QUICKSTART).expect("the fixture is JSON");

        assert_eq!(quickstart_request().to_value(), expected);
    }

    #[test]
    fn a_question_id_that_is_missing_an_instruction_is_refused() {
        let error = document(r#"{"questions":{"q":{"type":"noul"}}}"#)
            .expect_err("an instruction is required");

        assert!(error.to_string().contains("question \"q\""), "{error}");
        assert!(error.to_string().contains("instructions"), "{error}");
    }

    #[test]
    fn an_unknown_key_at_the_top_level_names_the_request_document() {
        let error = document(r#"{"question":{"q":{}}}"#).expect_err("the key is unknown");
        let rendered = error.to_string();

        assert!(rendered.contains("\"question\""), "{rendered}");
        assert!(rendered.contains("the request document"), "{rendered}");
        assert!(rendered.contains("\"questions\""), "{rendered}");
    }

    #[test]
    fn criteriaa_is_refused_where_the_question_is_parsed() {
        let json = r#"{"questions":{"department":{"type":"choice",
            "instructions":"which team?","criteriaa":{"billing":"payments"}}}}"#;

        let error = document(json).expect_err("the key is a typo, and the answer would be wrong");

        let rendered = error.to_string();
        assert!(rendered.contains("criteriaa"), "{rendered}");
        assert!(rendered.contains("question \"department\""), "{rendered}");
    }

    #[test]
    fn an_unknown_key_inside_a_question_names_the_question() {
        let json = concat!(
            r#"{"questions":{"is_urgent":"#,
            r#"{"type":"noul","instruction":"Does this convey urgency?"}}}"#,
        );

        let error = document(json).expect_err("`instruction` is not a key a question has");

        let rendered = error.to_string();
        assert!(rendered.contains("question \"is_urgent\""), "{rendered}");
        assert!(rendered.contains("\"instruction\""), "{rendered}");
        assert!(rendered.contains("\"instructions\""), "{rendered}");
    }

    #[test]
    fn an_unknown_question_type_is_refused_by_name() {
        let error = document(r#"{"questions":{"q":{"type":"boolean","instructions":"x"}}}"#)
            .expect_err("only three types exist");

        let rendered = error.to_string();
        assert!(rendered.contains("\"boolean\""), "{rendered}");
        assert!(rendered.contains("question \"q\""), "{rendered}");
    }

    #[test]
    fn a_document_that_is_not_an_object_is_refused() {
        let error = document("[]").expect_err("a document is an object");

        assert!(matches!(error, DecideError::DocumentNotObject), "{error:?}");
    }

    #[test]
    fn a_document_that_is_not_json_is_refused() {
        let error = document("{\"questions\":").expect_err("that is not JSON");

        assert!(
            matches!(error, DecideError::DocumentNotJson { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn a_document_without_questions_is_refused() {
        let error = document(r#"{"state":"hello"}"#).expect_err("a request asks something");

        assert!(error.to_string().contains("\"questions\""), "{error}");
    }

    #[test]
    fn a_document_with_no_questions_in_it_is_refused() {
        let error = document(r#"{"questions":{}}"#).expect_err("a request asks something");

        assert!(
            error.to_string().contains("at least one question"),
            "{error}"
        );
    }

    #[test]
    fn a_choice_without_criteria_is_refused() {
        let error = document(r#"{"questions":{"q":{"type":"choice","instructions":"x"}}}"#)
            .expect_err("a choice picks from options");

        assert!(error.to_string().contains("\"criteria\""), "{error}");
    }

    #[test]
    fn a_score_without_criteria_is_refused() {
        let error = document(r#"{"questions":{"q":{"type":"score","instructions":"x"}}}"#)
            .expect_err("a score rates along levels");

        assert!(error.to_string().contains("\"criteria\""), "{error}");
    }

    #[test]
    fn a_choice_without_options_is_refused() {
        let error = choice_of(0).validate("answer").expect_err("no options");

        assert_eq!(
            error.to_string(),
            "question \"answer\": a choice needs at least one option (none were given)"
        );
    }

    #[test]
    fn two_hundred_and_fifty_five_options_are_accepted_and_two_hundred_and_fifty_six_are_not() {
        assert!(choice_of(MAX_CHOICE_OPTIONS).validate("answer").is_ok());

        let error = choice_of(MAX_CHOICE_OPTIONS.saturating_add(1))
            .validate("answer")
            .expect_err("one too many");
        assert_eq!(
            error.to_string(),
            "question \"answer\": a choice has at most 255 options (256 were given)"
        );
    }

    #[test]
    fn a_score_needs_at_least_two_levels() {
        let error = score_of(0).validate("answer").expect_err("no levels");
        assert_eq!(
            error.to_string(),
            "question \"answer\": a score needs at least 2 levels (0 were given)"
        );

        let error = score_of(1).validate("answer").expect_err("one level");
        assert_eq!(
            error.to_string(),
            "question \"answer\": a score needs at least 2 levels (1 were given)"
        );
    }

    #[test]
    fn ten_levels_are_accepted_and_eleven_are_not() {
        assert!(score_of(MIN_SCORE_LEVELS).validate("answer").is_ok());
        assert!(score_of(MAX_SCORE_LEVELS).validate("answer").is_ok());

        let error = score_of(MAX_SCORE_LEVELS.saturating_add(1))
            .validate("answer")
            .expect_err("one too many");
        assert_eq!(
            error.to_string(),
            "question \"answer\": a score has at most 10 levels (11 were given)"
        );
    }

    #[test]
    fn a_noul_criteria_key_other_than_true_or_false_is_refused() {
        let json = r#"{"questions":{"q":{"type":"noul","instructions":"x",
            "criteria":{"maybe":"perhaps"}}}}"#;

        let error = request(json).expect_err("a noul has two answers");

        let rendered = error.to_string();
        assert!(rendered.contains("\"maybe\""), "{rendered}");
        assert!(rendered.contains("\"true\""), "{rendered}");
    }

    #[test]
    fn a_noul_may_carry_both_descriptions() {
        let json = r#"{"questions":{"q":{"type":"noul","instructions":"x",
            "criteria":{"true":"urgent","false":"routine"}}}}"#;

        let document = document(json).expect("both keys are legal");
        let question = document.questions.get("q").expect("the question is there");

        assert_eq!(
            question.criteria.as_ref().map(Criteria::len),
            Some(2),
            "the criteria survived the parse"
        );
    }

    #[test]
    fn a_question_id_that_is_empty_is_refused() {
        let json = r#"{"questions":{"":{"type":"noul","instructions":"x"}}}"#;
        let document = document(json).expect("the key is a string, so it parses");

        let error = Request::new(Value::Null, DEFAULT_MODEL.to_string(), document.questions)
            .expect_err("an empty id cannot be addressed");

        assert!(error.to_string().contains("id cannot be empty"), "{error}");
    }

    #[test]
    fn a_string_instruction_that_is_empty_is_refused() {
        let error = document(r#"{"questions":{"q":{"type":"noul","instructions":"   "}}}"#)
            .expect("the shape is right")
            .questions
            .remove("q")
            .expect("the question is there")
            .validate("q")
            .expect_err("an empty instruction asks nothing");

        assert!(
            error.to_string().contains("instruction is empty"),
            "{error}"
        );
    }

    #[test]
    fn a_state_that_is_a_number_is_refused_by_name() {
        let error = Request::new(
            Value::from(42),
            DEFAULT_MODEL.to_string(),
            BTreeMap::from([("q".to_string(), choice_of(1))]),
        )
        .expect_err("the API does not accept a number");

        assert_eq!(
            error.to_string(),
            "the state is a number; the API accepts a string, an object, or an array"
        );
    }

    #[test]
    fn a_state_that_is_a_boolean_is_refused_by_name() {
        let error = Request::new(
            Value::Bool(true),
            DEFAULT_MODEL.to_string(),
            BTreeMap::from([("q".to_string(), choice_of(1))]),
        )
        .expect_err("the API does not accept a boolean");

        assert_eq!(
            error.to_string(),
            "the state is a boolean; the API accepts a string, an object, or an array"
        );
    }

    #[test]
    fn a_request_with_no_questions_is_refused() {
        let error = Request::new(Value::Null, DEFAULT_MODEL.to_string(), BTreeMap::new())
            .expect_err("a request asks something");

        assert!(
            error.to_string().contains("at least one question"),
            "{error}"
        );
    }

    #[test]
    fn a_model_that_is_not_a_string_is_refused() {
        let error = document(r#"{"model":7,"questions":{"q":{"type":"noul","instructions":"x"}}}"#)
            .expect_err("a model is a name");

        assert!(error.to_string().contains("\"model\""), "{error}");
    }

    #[test]
    fn all_three_answer_shapes_parse_against_their_questions() {
        let request = quickstart_request();

        let response = Response::from_slice(ANSWERS.as_bytes(), &request)
            .expect("the vendor's response parses");

        assert_eq!(response.model.as_deref(), Some("jev-1.13.0"));
        assert_eq!(response.answers.len(), 3);
        let answer = response
            .answers
            .get("is_urgent")
            .expect("the noul is there");
        assert_eq!(answer.kind, QuestionKind::Noul);
        assert!(answer.own_value().is_some_and(Value::is_number));
    }

    #[test]
    fn the_usage_the_response_reported_is_readable() {
        let response = Response::from_slice(ANSWERS.as_bytes(), &quickstart_request())
            .expect("the vendor's response parses");

        assert_eq!(
            response.usage(),
            Some(Usage {
                input_tokens: 296,
                output_tokens: 20
            })
        );
    }

    #[test]
    fn a_response_without_usable_usage_reports_none_rather_than_failing() {
        let json = r#"{"answers":{"is_urgent":{"type":"noul","noul":0.95}},
            "usage":{"input_tokens":"many"}}"#;

        let response = Response::from_slice(json.as_bytes(), &noul_request())
            .expect("usage is not part of the answer contract");

        assert_eq!(response.usage(), None);
    }

    #[test]
    fn an_extra_field_in_a_response_is_ignored() {
        let extra = include_str!("../tests/data/response_with_extra_fields.json");
        let mut questions = BTreeMap::new();
        questions.insert(
            "is_urgent".to_string(),
            Question::noul(Value::String("urgent?".to_string()), None),
        );
        let request = Request::new(Value::Null, DEFAULT_MODEL.to_string(), questions)
            .expect("a valid request");

        let response = Response::from_slice(extra.as_bytes(), &request)
            .expect("a field this client has never heard of is not an error");

        assert_eq!(response.model.as_deref(), Some("jev-1.13.0"));
    }

    #[test]
    fn an_answer_whose_type_disagrees_with_its_question_is_refused() {
        let mut questions = BTreeMap::new();
        questions.insert(
            "department".to_string(),
            Question::choice(
                Value::String("which?".to_string()),
                BTreeMap::from([("billing".to_string(), Value::Null)]),
            ),
        );
        let request = Request::new(Value::Null, DEFAULT_MODEL.to_string(), questions)
            .expect("a valid request");
        let json = r#"{"answers":{"department":{"type":"score","score":1.0}}}"#;

        let error = Response::from_slice(json.as_bytes(), &request)
            .expect_err("a renamed answer is not a plausible number");

        assert!(
            error
                .to_string()
                .contains("question \"department\" asked for a choice"),
            "{error}"
        );
    }

    #[test]
    fn a_question_with_no_answer_is_refused() {
        let request = quickstart_request();
        let json = r#"{"model":"jev-1.13.0","answers":{}}"#;

        let error = Response::from_slice(json.as_bytes(), &request)
            .expect_err("every question is answered");

        assert!(
            error.to_string().contains("no answer for question"),
            "{error}"
        );
    }

    #[test]
    fn a_response_body_that_is_not_json_is_refused() {
        let error = Response::from_slice(b"<html>nope</html>", &quickstart_request())
            .expect_err("that is not JSON");

        assert!(
            matches!(error, DecideError::ResponseNotJson { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn a_response_that_is_not_an_object_is_refused() {
        let error = Response::from_slice(b"[]", &quickstart_request())
            .expect_err("a response is an object");

        assert!(
            matches!(error, DecideError::ResponseContract { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn an_answer_that_is_not_an_object_is_refused() {
        let json = r#"{"answers":{"is_urgent":"yes"}}"#;

        let error = Response::from_slice(json.as_bytes(), &noul_request())
            .expect_err("an answer is an object");

        assert!(error.to_string().contains("not an object"), "{error}");
    }

    #[test]
    fn a_noul_answer_that_is_not_a_number_is_refused() {
        let json = r#"{"answers":{"is_urgent":{"type":"noul","noul":"very"}}}"#;

        let error = Response::from_slice(json.as_bytes(), &noul_request())
            .expect_err("a probability is a number");

        assert!(error.to_string().contains("not a number"), "{error}");
    }

    #[test]
    fn the_models_response_lists_the_aliases() {
        let models = ModelsResponse::from_slice(include_bytes!("../tests/data/models.json"))
            .expect("the fixture parses");
        let listed = models.models();

        assert_eq!(listed.len(), 2);
        assert_eq!(
            listed.first().map(|model| model.name.as_str()),
            Some("jev-latest")
        );
        assert_eq!(
            listed
                .get(1)
                .and_then(|model| model.release_date.as_deref()),
            Some("2026-09-12")
        );
    }

    #[test]
    fn only_a_models_list_counts_as_the_apis_answer() {
        let answer = |json: &str| {
            ModelsResponse::from_slice(json.as_bytes())
                .expect("valid JSON parses")
                .lists_models()
        };

        // The shape the documentation shows, and one entry with a name is enough.
        assert!(answer(r#"{"models":[{"name":"jev-latest"}]}"#));
        assert!(answer(
            r#"{"models":[{"name":"jev-latest","description":"d","release_date":"r"}],"extra":1}"#
        ));
        assert!(answer(include_str!("../tests/data/models.json")));

        // Everything else is JSON that is not the API answering: a proxy, a captive portal,
        // or a base URL pointing at some other service.
        assert!(!answer(r#"{"status":"ok","proxy":"corporate"}"#));
        assert!(!answer("{}"));
        assert!(!answer(r#"{"models":[]}"#));
        assert!(!answer(r#"{"models":"jev-latest"}"#));
        assert!(!answer(r#"{"models":[{"no_name":"x"}]}"#));
        assert!(!answer("[]"));
    }

    #[test]
    fn a_models_body_that_is_not_json_is_refused() {
        let error = ModelsResponse::from_slice(b"not json").expect_err("that is not JSON");

        assert!(
            matches!(error, DecideError::ResponseNotJson { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn a_json_kind_is_named_in_words() {
        assert_eq!(json_kind(&Value::Null), "null");
        assert_eq!(json_kind(&Value::Bool(true)), "a boolean");
        assert_eq!(json_kind(&Value::from(1)), "a number");
        assert_eq!(json_kind(&Value::from("x")), "a string");
        assert_eq!(json_kind(&Value::Array(Vec::new())), "an array");
        assert_eq!(json_kind(&Value::Object(Map::new())), "an object");
    }
}

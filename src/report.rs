//! What comes back: the path selection of `--select`, `--value`, and `--field`, and the
//! rendering of a value to the bytes stdout carries.
//!
//! Three jobs, one per decision:
//!
//! - **`--select`** walks a dotted path from the response root
//!   ([D16](../docs/design.md#d16-three-extraction-flags-split-by-how-far-the-path-reaches)).
//!   The walk uses `.get()` and returns a `Result`, because a path that is not there is a
//!   caller error to name rather than a crash.
//! - **Rendering** prints a scalar raw and a composite as JSON
//!   ([D3](../docs/design.md#d3-stdout-is-the-apis-response-and-nothing-else),
//!   [D4](../docs/design.md#d4-compact-json-by-default---pretty-opts-in)).
//! - Nothing here touches a stream; `run` writes the string once, at the end.

use serde_json::{Map, Value};

use crate::error::DecideError;
use crate::wire::{self, Answer, Request, Response};

/// Resolve a dotted path from the response root, for `--select`.
pub fn select<'a>(root: &'a Value, path: &str) -> Result<&'a Value, DecideError> {
    walk(root, path, "--select")
}

/// Walk a dotted path from `root`, naming `flag` in any error.
///
/// A segment walks into an object as a key or into an array as an index. On an object a
/// numeric segment *is* the key, which is how `--field legend.2` finds the level numbered
/// `2`.
pub fn walk<'a>(root: &'a Value, path: &str, flag: &'static str) -> Result<&'a Value, DecideError> {
    if path.is_empty() {
        return Err(DecideError::PathNotFound {
            flag,
            path: path.to_string(),
            problem: "the path is empty".to_string(),
        });
    }

    let mut current = root;
    let mut walked: Vec<&str> = Vec::new();

    for segment in path.split('.') {
        let next = match current {
            Value::Object(map) => map.get(segment).ok_or_else(|| DecideError::PathNotFound {
                flag,
                path: path.to_string(),
                problem: format!(
                    "no key \"{segment}\" at {}; available keys: {}",
                    location(&walked),
                    keys(map)
                ),
            })?,
            Value::Array(items) => {
                let index: usize = segment.parse().map_err(|_| DecideError::PathNotFound {
                    flag,
                    path: path.to_string(),
                    problem: format!(
                        "no index \"{segment}\" at {}; an array is walked by index, not \
                                 by key",
                        location(&walked)
                    ),
                })?;
                items.get(index).ok_or_else(|| DecideError::PathNotFound {
                    flag,
                    path: path.to_string(),
                    problem: format!(
                        "no index {index} at {}; the array has {} entries",
                        location(&walked),
                        items.len()
                    ),
                })?
            }
            other => {
                return Err(DecideError::PathNotFound {
                    flag,
                    path: path.to_string(),
                    problem: format!(
                        "{} is {}, not an object or an array",
                        location(&walked),
                        wire::json_kind(other)
                    ),
                });
            }
        };
        walked.push(segment);
        current = next;
    }

    Ok(current)
}

/// The single answer, when the request asked exactly one question.
pub fn one_answer<'a>(
    response: &'a Response,
    request: &Request,
    flag: &'static str,
) -> Result<&'a Answer, DecideError> {
    let Some(id) = request.only_question_id() else {
        return Err(DecideError::OneAnswerRequired {
            flag,
            count: request.question_count(),
        });
    };
    response
        .answers
        .get(id)
        .ok_or_else(|| DecideError::ResponseContract {
            problem: format!("there is no answer for question \"{id}\""),
        })
}

/// The one answer's own value, for `--value`.
pub fn answer_value(answer: &Answer) -> Result<&Value, DecideError> {
    answer
        .own_value()
        .ok_or_else(|| DecideError::ResponseContract {
            problem: format!(
                "the answer has no \"{}\" for --value to print",
                answer.kind.as_str()
            ),
        })
}

/// Render one value the way `--select`, `--value`, and `--field` print it: a scalar raw,
/// with no JSON quoting, and an object or array as JSON.
#[must_use]
pub fn render_value(value: &Value, pretty: bool) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(_) | Value::Bool(_) | Value::Null => value.to_string(),
        Value::Array(_) | Value::Object(_) => render_json(value, pretty),
    }
}

/// Render a whole JSON value, compact unless `pretty`.
///
/// Keys come out sorted because `serde_json`'s map is a `BTreeMap`, which is where
/// [D4](../docs/design.md#d4-compact-json-by-default---pretty-opts-in)'s determinism comes
/// from rather than from the field order in any declaration.
#[must_use]
pub fn render_json(value: &Value, pretty: bool) -> String {
    if pretty {
        // A `Value` always serialises, so the fallback handles a `Result` that cannot be
        // an `Err`; it is here because the `Result` still has to be consumed.
        serde_json::to_string_pretty(value).unwrap_or_default()
    } else {
        value.to_string()
    }
}

/// The words for where a path walk had got to: the response root, or the prefix so far.
fn location(walked: &[&str]) -> String {
    if walked.is_empty() {
        "the response root".to_string()
    } else {
        format!("\"{}\"", walked.join("."))
    }
}

/// The keys available at a path segment, quoted, for a message that has to list them.
fn keys(map: &Map<String, Value>) -> String {
    if map.is_empty() {
        return "none".to_string();
    }
    map.keys()
        .map(|key| format!("\"{key}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;
    use crate::wire::{Question, Response};

    /// A request asking the questions named.
    fn request_with(ids: &[&str]) -> Request {
        let mut questions = BTreeMap::new();
        for id in ids {
            questions.insert(
                (*id).to_string(),
                Question::noul(Value::String("is it so?".to_string()), None),
            );
        }
        Request::new(Value::Null, "jev-latest".to_string(), questions).expect("a valid request")
    }

    /// A response to that request.
    fn response_for(ids: &[&str], body: &str) -> (Request, Response) {
        let request = request_with(ids);
        let response =
            Response::from_slice(body.as_bytes(), &request).expect("the body is a response");
        (request, response)
    }

    #[test]
    fn a_string_value_is_printed_without_quotes() {
        assert_eq!(render_value(&json!("technical"), false), "technical");
    }

    #[test]
    fn a_number_is_printed_as_json_printed_it() {
        assert_eq!(render_value(&json!(1.0), false), "1.0");
        assert_eq!(render_value(&json!(296), false), "296");
        assert_eq!(render_value(&json!(0.78), false), "0.78");
    }

    #[test]
    fn a_boolean_and_null_are_printed_literally() {
        assert_eq!(render_value(&json!(true), false), "true");
        assert_eq!(render_value(&json!(false), false), "false");
        assert_eq!(render_value(&Value::Null, false), "null");
    }

    #[test]
    fn an_object_is_printed_as_json_with_its_keys_sorted() {
        let value = json!({"b": 1, "a": 2});

        assert_eq!(render_value(&value, false), r#"{"a":2,"b":1}"#);
        assert_eq!(render_json(&value, false), r#"{"a":2,"b":1}"#);
    }

    #[test]
    fn pretty_indents_a_composite() {
        let value = json!({"a": 1});

        assert_eq!(render_json(&value, true), "{\n  \"a\": 1\n}");
    }

    #[test]
    fn a_path_walks_into_an_object_by_key() {
        let response = json!({"answers": {"department": {"choice": "technical"}}});

        let value = select(&response, "answers.department.choice").expect("the path resolves");

        assert_eq!(render_value(value, false), "technical");
    }

    #[test]
    fn a_numeric_segment_is_an_index_in_an_array_and_a_key_in_an_object() {
        let response = json!({"models": [{"name": "jev-latest"}], "legend": {"2": "very angry"}});

        assert_eq!(
            render_value(select(&response, "models.0.name").expect("an index"), false),
            "jev-latest"
        );
        assert_eq!(
            render_value(select(&response, "legend.2").expect("a key"), false),
            "very angry"
        );
    }

    #[test]
    fn a_missing_path_names_the_segment_and_the_keys_available() {
        let response = json!({"answers": {"is_urgent": {"noul": 0.95}}});

        let error = select(&response, "answers.urgency.noul").expect_err("there is no urgency");

        let rendered = error.to_string();
        assert_eq!(error.exit_code(), 2);
        assert!(rendered.contains("--select"), "{rendered}");
        assert!(rendered.contains("answers.urgency.noul"), "{rendered}");
        assert!(rendered.contains("no key \"urgency\""), "{rendered}");
        assert!(rendered.contains("\"is_urgent\""), "{rendered}");
    }

    #[test]
    fn a_missing_key_at_the_root_names_the_response_root() {
        let response = json!({"answers": {}});

        let error = select(&response, "answer").expect_err("there is no answer key");

        let rendered = error.to_string();
        assert!(rendered.contains("the response root"), "{rendered}");
        assert!(rendered.contains("\"answers\""), "{rendered}");
    }

    #[test]
    fn an_out_of_range_index_names_the_length() {
        let response = json!({"models": [{"name": "a"}]});

        let error = select(&response, "models.7.name").expect_err("there is no index 7");

        let rendered = error.to_string();
        assert!(rendered.contains("no index 7"), "{rendered}");
        assert!(rendered.contains("1 entries"), "{rendered}");
    }

    #[test]
    fn a_segment_that_is_not_a_number_on_an_array_is_refused() {
        let response = json!({"models": [{"name": "a"}]});

        let error = select(&response, "models.name").expect_err("an array is not a map");

        assert!(error.to_string().contains("walked by index"), "{error}");
    }

    #[test]
    fn a_path_through_a_scalar_is_refused() {
        let response = json!({"usage": {"input_tokens": 296}});

        let error =
            select(&response, "usage.input_tokens.extra").expect_err("a number has no keys");

        assert!(
            error.to_string().contains("not an object or an array"),
            "{error}"
        );
    }

    #[test]
    fn an_empty_path_is_refused() {
        let error = select(&json!({}), "").expect_err("nothing to walk");

        assert!(error.to_string().contains("the path is empty"), "{error}");
    }

    #[test]
    fn an_answer_keyed_with_a_dot_is_reachable_by_value_but_not_by_select() {
        let body = r#"{"answers":{"my.id":{"type":"noul","noul":0.5}}}"#;
        let (request, response) = response_for(&["my.id"], body);

        // `--value` sidesteps the grammar entirely: it never spells the id.
        let answer = one_answer(&response, &request, "--value").expect("the one answer");
        assert_eq!(
            render_value(answer_value(answer).expect("a noul"), false),
            "0.5"
        );

        // `--select` cannot, because the dot is the separator.
        let error = select(&response.value, "answers.my.id.noul").expect_err("the dot splits it");
        assert!(error.to_string().contains("no key \"my\""), "{error}");
    }

    #[test]
    fn a_value_is_refused_when_the_request_asked_more_than_one_question() {
        let body = r#"{"answers":{"a":{"type":"noul","noul":0.5},"b":{"type":"noul","noul":0.5}}}"#;
        let (request, response) = response_for(&["a", "b"], body);

        let error = one_answer(&response, &request, "--value").expect_err("the value of two");

        assert_eq!(error.exit_code(), 2);
        assert!(
            error.to_string().contains("exactly one question"),
            "{error}"
        );
        assert!(error.to_string().contains("asked 2"), "{error}");
        assert!(error.to_string().contains("--select"), "{error}");
    }

    #[test]
    fn a_field_path_starts_at_the_one_answer() {
        let body = r#"{"answers":{"q":{"type":"score","score":1.05,"confidence":0.92,
            "legend":{"0":"calm","1":"frustrated","2":"very angry"}}}}"#;
        let mut questions = BTreeMap::new();
        questions.insert(
            "q".to_string(),
            Question::score(
                Value::String("how frustrated?".to_string()),
                vec![
                    Value::String("calm".to_string()),
                    Value::String("frustrated".to_string()),
                ],
            ),
        );
        let request = Request::new(Value::Null, "jev-latest".to_string(), questions)
            .expect("a valid request");
        let response =
            Response::from_slice(body.as_bytes(), &request).expect("the body is a response");
        let answer = one_answer(&response, &request, "--field").expect("the one answer");

        assert_eq!(
            render_value(
                walk(&answer.value, "legend.2", "--field").expect("a key"),
                false
            ),
            "very angry"
        );
        assert_eq!(
            render_value(
                walk(&answer.value, "confidence", "--field").expect("a key"),
                false
            ),
            "0.92"
        );
    }

    #[test]
    fn a_field_error_names_the_field_flag() {
        let error = walk(&json!({"a": 1}), "b", "--field").expect_err("there is no b");

        assert!(error.to_string().contains("--field"), "{error}");
    }
}

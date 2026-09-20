//! Where the parts of a request come from.
//!
//! The state rules of
//! [D7](../docs/design.md#d7-the-state-comes-from-exactly-one-place), and the
//! flags-to-question builder of
//! [D6](../docs/design.md#d6-two-forms-for-the-questions-flags-for-one-a-document-for-many).
//!
//! The state rules are the reason [`Stdin`] exists as a type rather than a bare `Read`: the
//! difference between a terminal and a pipe is a branch that is otherwise only verifiable by
//! hand, and every "which source won" question is a unit test with an in-memory reader.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::DecideError;
use crate::wire::{self, Question};

/// The standard input, together with the one fact about it that changes behaviour: whether
/// it is a terminal.
///
/// A piped-in state is used when nothing else supplies one; a terminal is a usage error
/// rather than a wait, because a command that blocks for input a script cannot type is a
/// hang.
pub struct Stdin<R> {
    reader: R,
    terminal: bool,
}

impl<R: Read> Stdin<R> {
    /// Pair a reader with the answer `is_terminal()` would give for it.
    pub const fn new(reader: R, terminal: bool) -> Self {
        Self { reader, terminal }
    }

    /// Whether this stdin is a terminal.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        self.terminal
    }

    /// Read all of stdin as UTF-8 text.
    pub fn read_all(&mut self) -> Result<String, std::io::Error> {
        let mut buffer = String::new();
        self.reader.read_to_string(&mut buffer)?;
        Ok(buffer)
    }
}

/// The state flags, as [`resolve_state`] needs them.
#[derive(Debug, Clone, Default)]
pub struct StateArgs<'a> {
    /// `--state TEXT`.
    pub state: Option<&'a str>,
    /// `--state-file PATH`, where `-` means stdin.
    pub state_file: Option<&'a Path>,
    /// `--state-json`.
    pub state_json: bool,
    /// `--no-state`.
    pub no_state: bool,
}

/// One place a state can come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// `--state`.
    State,
    /// `--state-file`.
    StateFile,
    /// `--no-state`.
    NoState,
    /// The `state` key of the request document.
    Document,
    /// A piped stdin.
    Stdin,
}

impl Source {
    /// The name a caller would use for this source.
    const fn name(self) -> &'static str {
        match self {
            Self::State => "--state",
            Self::StateFile => "--state-file",
            Self::NoState => "--no-state",
            Self::Document => "the request document's \"state\"",
            Self::Stdin => "stdin",
        }
    }
}

/// Resolve the one state this request sends, from the five places it can come from.
///
/// `stdin_consumed` says whether stdin has already been read as the request document, in
/// which case it cannot also be the state. `document_state` is `Some(null)` when the
/// document carried `"state": null`, which is a source; the caller must distinguish that
/// from the key being absent.
pub fn resolve_state<R: Read>(
    args: &StateArgs<'_>,
    document_state: Option<&Value>,
    stdin: &mut Stdin<R>,
    stdin_consumed: bool,
) -> Result<Value, DecideError> {
    if args.state_json {
        if args.no_state {
            return Err(DecideError::StateJsonWithNoState);
        }
        if document_state.is_some() {
            return Err(DecideError::StateJsonWithDocumentState);
        }
    }

    let mut sources: Vec<Source> = Vec::new();
    if args.state.is_some() {
        sources.push(Source::State);
    }
    if args.state_file.is_some() {
        sources.push(Source::StateFile);
    }
    if args.no_state {
        sources.push(Source::NoState);
    }
    if document_state.is_some() {
        sources.push(Source::Document);
    }

    // stdin is used when no other source is named and it was not already read as the
    // document, which is the table in `docs/cli.md`.
    //
    // A *named* source always wins over a pipe, including a state the request document
    // carries: a document with its own state is the documented shape of `ask`, and a
    // command whose behaviour turned on whether stdin happened to be `/dev/null` (which is
    // not a terminal, and is what a cron job, a CI runner, and a subprocess all hand in)
    // would be a command that breaks in exactly the place it is meant to be used. The
    // conflict the documentation calls out is still an error, because it is an *explicit*
    // one: `--state-file -` names the pipe as the state, and a document that carries a
    // state as well is two sources.
    if sources.is_empty() && !stdin_consumed && !stdin.is_terminal() {
        sources.push(Source::Stdin);
    }

    if sources.len() > 1 {
        return Err(DecideError::TwoStateSources {
            first: sources.first().map_or("", |source| source.name()),
            second: sources.get(1).map_or("", |source| source.name()),
        });
    }

    let state = match sources.first().copied() {
        Some(Source::NoState) => Value::Null,
        Some(Source::Document) => document_state.map_or(Value::Null, Clone::clone),
        Some(Source::State) => value_from_text(args.state.unwrap_or_default(), args.state_json)?,
        Some(Source::StateFile) => {
            let path = args.state_file.unwrap_or_else(|| Path::new(""));
            if path.as_os_str() == "-" && stdin_consumed {
                return Err(DecideError::TwoStateSources {
                    first: "--state-file",
                    second: "the request document on stdin",
                });
            }
            let text = if path.as_os_str() == "-" {
                read_stdin(stdin)?
            } else {
                read_to_string(path)?
            };
            value_from_text(&text, args.state_json)?
        }
        Some(Source::Stdin) => {
            let text = read_stdin(stdin)?;
            value_from_text(&text, args.state_json)?
        }
        None => {
            if args.state_json {
                return Err(DecideError::StateJsonWithoutSource);
            }
            return Err(DecideError::NoStateSource);
        }
    };

    wire::check_state_kind(&state)?;
    if matches!(&state, Value::String(text) if text.trim().is_empty()) {
        return Err(DecideError::EmptyState);
    }
    Ok(state)
}

/// Turn text into a state: the text itself, or the JSON it holds.
fn value_from_text(text: &str, as_json: bool) -> Result<Value, DecideError> {
    if as_json {
        let value: Value =
            serde_json::from_str(text).map_err(|error| DecideError::StateNotJson {
                reason: error.to_string(),
            })?;
        Ok(value)
    } else {
        Ok(Value::String(text.to_string()))
    }
}

/// Resolve the model from its four possible sources.
///
/// The flag wins, then the document, then the environment, then the default. A model is a
/// knob, so it overrides silently
/// ([D17](../docs/design.md#d17-a-model-is-a-knob-a-state-and-a-question-are-meaning)).
#[must_use]
pub fn resolve_model(
    flag: Option<&str>,
    document: Option<&str>,
    environment: Option<&str>,
) -> String {
    flag.or(document)
        .or(environment)
        .unwrap_or(wire::DEFAULT_MODEL)
        .to_string()
}

/// Check a base URL has a scheme and normalise away one trailing slash, so that
/// `/v1/systemone` is never appended to a base that already ends in one.
pub fn resolve_base_url(raw: &str) -> Result<String, DecideError> {
    let trimmed = raw.trim_end_matches('/');
    let host = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"));
    match host {
        Some(host) if !host.is_empty() => Ok(trimmed.to_string()),
        _ => Err(DecideError::BaseUrlWithoutScheme {
            url: raw.to_string(),
        }),
    }
}

/// Read the request document for `ask`, and say whether stdin was consumed by it.
///
/// A missing `FILE`, or `-`, reads stdin; with a terminal on stdin and no file to read,
/// this is a usage error rather than a wait.
pub fn read_document<R: Read>(
    file: Option<&Path>,
    stdin: &mut Stdin<R>,
) -> Result<(String, bool), DecideError> {
    match file {
        Some(path) if path.as_os_str() != "-" => Ok((read_to_string(path)?, false)),
        _ => {
            if stdin.is_terminal() {
                return Err(DecideError::NoDocument);
            }
            Ok((read_stdin(stdin)?, true))
        }
    }
}

/// Read a file, naming the path and the OS error if it cannot be read.
pub fn read_to_string(path: &Path) -> Result<String, DecideError> {
    std::fs::read_to_string(path).map_err(|error| DecideError::UnreadableFile {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })
}

/// Read stdin as UTF-8 text.
fn read_stdin<R: Read>(stdin: &mut Stdin<R>) -> Result<String, DecideError> {
    stdin
        .read_all()
        .map_err(|error| DecideError::UnreadableFile {
            path: PathBuf::from("-"),
            reason: error.to_string(),
        })
}

/// Build the single noul question the `noul` subcommand asks.
#[must_use]
pub fn noul_question(instruction: &str, yes: Option<&str>, no: Option<&str>) -> Question {
    let mut criteria = BTreeMap::new();
    if let Some(yes) = yes {
        criteria.insert("true".to_string(), Value::String(yes.to_string()));
    }
    if let Some(no) = no {
        criteria.insert("false".to_string(), Value::String(no.to_string()));
    }
    let criteria = if criteria.is_empty() {
        None
    } else {
        Some(criteria)
    };
    Question::noul(Value::String(instruction.to_string()), criteria)
}

/// Build the single choice question the `choice` subcommand asks.
///
/// An option splits on its **first** `=`, so a description may contain one; a bare name
/// sends `null`, and `name=` sends an empty string, which is not the same thing.
pub fn choice_question(instruction: &str, options: &[String]) -> Result<Question, DecideError> {
    let mut criteria = BTreeMap::new();
    for option in options {
        let (name, description) = match option.split_once('=') {
            Some((name, description)) => (name, Value::String(description.to_string())),
            None => (option.as_str(), Value::Null),
        };
        if criteria.contains_key(name) {
            return Err(DecideError::DuplicateOption {
                name: name.to_string(),
            });
        }
        criteria.insert(name.to_string(), description);
    }
    Ok(Question::choice(
        Value::String(instruction.to_string()),
        criteria,
    ))
}

/// Build the single score question the `score` subcommand asks.
#[must_use]
pub fn score_question(instruction: &str, levels: &[String]) -> Question {
    let levels = levels
        .iter()
        .map(|level| Value::String(level.clone()))
        .collect();
    Question::score(Value::String(instruction.to_string()), levels)
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write as _};

    use serde_json::json;

    use super::*;
    use crate::wire::Criteria;

    /// A stdin that answers `is_terminal` the way the test says, over in-memory text.
    fn stdin(text: &str, terminal: bool) -> Stdin<Cursor<Vec<u8>>> {
        Stdin::new(Cursor::new(text.as_bytes().to_vec()), terminal)
    }

    /// State flags with nothing named.
    fn nothing() -> StateArgs<'static> {
        StateArgs::default()
    }

    #[test]
    fn stdin_is_read_when_nothing_else_supplies_a_state() {
        let mut pipe = stdin("  Hello from a pipe\n", false);

        let state = resolve_state(&nothing(), None, &mut pipe, false).expect("stdin supplies it");

        assert_eq!(state, Value::String("  Hello from a pipe\n".to_string()));
    }

    #[test]
    fn a_terminal_with_no_state_named_is_a_usage_error() {
        let mut terminal = stdin("", true);

        let error = resolve_state(&nothing(), None, &mut terminal, false)
            .expect_err("a terminal is never a wait");

        let rendered = error.to_string();
        assert_eq!(error.exit_code(), 2);
        assert!(rendered.contains("--state"), "{rendered}");
        assert!(rendered.contains("--state-file"), "{rendered}");
        assert!(rendered.contains("--no-state"), "{rendered}");
    }

    #[test]
    fn whitespace_only_state_is_refused_because_a_pipeline_delivered_nothing() {
        let args = StateArgs {
            state: Some("   \n\t "),
            ..nothing()
        };
        let mut terminal = stdin("", true);

        let error = resolve_state(&args, None, &mut terminal, false)
            .expect_err("an empty pipeline is never what was meant");

        assert!(error.to_string().contains("whitespace only"), "{error}");
    }

    #[test]
    fn a_piped_whitespace_only_state_is_refused_too() {
        let mut pipe = stdin("\n\n", false);

        let error = resolve_state(&nothing(), None, &mut pipe, false)
            .expect_err("an empty pipeline is never what was meant");

        assert!(error.to_string().contains("whitespace only"), "{error}");
    }

    #[test]
    fn no_state_sends_null() {
        let args = StateArgs {
            no_state: true,
            ..nothing()
        };
        let mut terminal = stdin("", true);

        let state = resolve_state(&args, None, &mut terminal, false).expect("null is a state");

        assert_eq!(state, Value::Null);
    }

    #[test]
    fn a_piped_state_and_a_state_in_the_document_are_refused_together() {
        // `--state-file -` names the pipe as the state, so a document that carries one too
        // is two sources: the caller said where the state is, twice.
        let args = StateArgs {
            state_file: Some(Path::new("-")),
            ..nothing()
        };
        let document_state = json!("in the document");
        let mut pipe = stdin("on stdin", false);

        let error = resolve_state(&args, Some(&document_state), &mut pipe, false)
            .expect_err("a state given twice has no defensible winner");

        let rendered = error.to_string();
        assert_eq!(error.exit_code(), 2);
        assert!(rendered.contains("--state-file"), "{rendered}");
        assert!(rendered.contains("request document"), "{rendered}");
    }

    #[test]
    fn a_pipe_is_left_alone_when_the_document_names_its_own_state() {
        // `/dev/null` is not a terminal, and a cron job, a CI runner, and a subprocess all
        // hand one in. A document with a state is the documented shape of `ask`, so this
        // must not become an error that only happens off a terminal.
        let document_state = json!("in the document");
        let mut pipe = stdin("", false);

        let state = resolve_state(&nothing(), Some(&document_state), &mut pipe, false)
            .expect("the document names the state");

        assert_eq!(state, document_state);
    }

    #[test]
    fn two_named_state_sources_are_refused_by_name() {
        let args = StateArgs {
            state: Some("one"),
            no_state: true,
            ..nothing()
        };
        let mut terminal = stdin("", true);

        let error = resolve_state(&args, None, &mut terminal, false).expect_err("two sources");

        let rendered = error.to_string();
        assert!(
            rendered.contains("--state") && rendered.contains("--no-state"),
            "{rendered}"
        );
    }

    #[test]
    fn a_named_state_source_leaves_a_pipe_alone() {
        let args = StateArgs {
            state: Some("from the flag"),
            ..nothing()
        };
        let mut pipe = stdin("from the pipe", false);

        let state = resolve_state(&args, None, &mut pipe, false).expect("the flag wins");

        assert_eq!(state, Value::String("from the flag".to_string()));
    }

    #[test]
    fn a_document_state_is_used_when_it_is_the_only_source() {
        let document_state = json!({"a": 1});
        let mut terminal = stdin("", true);

        let state = resolve_state(&nothing(), Some(&document_state), &mut terminal, false)
            .expect("the document supplies it");

        assert_eq!(state, document_state);
    }

    #[test]
    fn a_document_state_of_null_is_a_state() {
        let document_state = Value::Null;
        let mut terminal = stdin("", true);

        let state = resolve_state(&nothing(), Some(&document_state), &mut terminal, false)
            .expect("null is a state");

        assert_eq!(state, Value::Null);
    }

    #[test]
    fn a_state_file_is_read_and_a_dash_means_stdin() {
        let mut file = tempfile::NamedTempFile::new().expect("a temporary file");
        file.write_all(b"from a file").expect("write");

        let args = StateArgs {
            state_file: Some(file.path()),
            ..nothing()
        };
        let mut terminal = stdin("", true);
        let state = resolve_state(&args, None, &mut terminal, false).expect("the file supplies it");
        assert_eq!(state, Value::String("from a file".to_string()));

        let args = StateArgs {
            state_file: Some(Path::new("-")),
            ..nothing()
        };
        let mut pipe = stdin("from stdin", false);
        let state = resolve_state(&args, None, &mut pipe, false).expect("the dash is stdin");
        assert_eq!(state, Value::String("from stdin".to_string()));
    }

    #[test]
    fn an_unreadable_state_file_names_the_path() {
        let args = StateArgs {
            state_file: Some(Path::new("/nonexistent/state.txt")),
            ..nothing()
        };
        let mut terminal = stdin("", true);

        let error = resolve_state(&args, None, &mut terminal, false).expect_err("no such file");

        let rendered = error.to_string();
        assert_eq!(error.exit_code(), 2);
        assert!(rendered.contains("/nonexistent/state.txt"), "{rendered}");
    }

    #[test]
    fn state_json_parses_the_text_of_a_state() {
        let args = StateArgs {
            state: Some(r#"{"ticket": 42}"#),
            state_json: true,
            ..nothing()
        };
        let mut terminal = stdin("", true);

        let state = resolve_state(&args, None, &mut terminal, false).expect("valid JSON");

        assert_eq!(state, json!({"ticket": 42}));
    }

    #[test]
    fn state_json_reads_a_piped_state_as_json() {
        let mut pipe = stdin("[1, 2, 3]", false);

        let args = StateArgs {
            state_json: true,
            ..nothing()
        };
        let state = resolve_state(&args, None, &mut pipe, false).expect("valid JSON");

        assert_eq!(state, json!([1, 2, 3]));
    }

    #[test]
    fn state_json_with_text_that_is_not_json_is_refused_by_name() {
        let args = StateArgs {
            state: Some("{not json"),
            state_json: true,
            ..nothing()
        };
        let mut terminal = stdin("", true);

        let error = resolve_state(&args, None, &mut terminal, false).expect_err("that is not JSON");

        assert!(error.to_string().contains("--state-json"), "{error}");
    }

    #[test]
    fn state_json_with_a_number_is_refused_by_name() {
        let args = StateArgs {
            state: Some("42"),
            state_json: true,
            ..nothing()
        };
        let mut terminal = stdin("", true);

        let error = resolve_state(&args, None, &mut terminal, false)
            .expect_err("the API does not accept a number");

        assert!(
            error.to_string().contains("the state is a number"),
            "{error}"
        );
    }

    #[test]
    fn state_json_with_no_state_is_refused() {
        let args = StateArgs {
            state_json: true,
            no_state: true,
            ..nothing()
        };
        let mut terminal = stdin("", true);

        let error = resolve_state(&args, None, &mut terminal, false).expect_err("they disagree");

        assert!(error.to_string().contains("--no-state"), "{error}");
    }

    #[test]
    fn state_json_with_a_document_state_is_refused() {
        let args = StateArgs {
            state_json: true,
            ..nothing()
        };
        let document_state = json!("already json");
        let mut terminal = stdin("", true);

        let error = resolve_state(&args, Some(&document_state), &mut terminal, false)
            .expect_err("the document's state is already JSON");

        assert!(error.to_string().contains("already carries"), "{error}");
    }

    #[test]
    fn state_json_with_nothing_to_describe_is_refused() {
        let args = StateArgs {
            state_json: true,
            ..nothing()
        };
        let mut terminal = stdin("", true);

        let error = resolve_state(&args, None, &mut terminal, false).expect_err("nothing to parse");

        assert!(error.to_string().contains("--state-json"), "{error}");
    }

    #[test]
    fn a_document_read_from_stdin_leaves_no_stdin_for_the_state() {
        // `ask` with no file: the document consumed stdin, so there is nothing left.
        let mut pipe = stdin("{\"questions\":{}}", false);

        let error = resolve_state(&nothing(), None, &mut pipe, true)
            .expect_err("stdin cannot be both the document and the state");

        assert_eq!(error.exit_code(), 2);
    }

    #[test]
    fn a_state_file_on_stdin_is_refused_when_the_document_already_used_it() {
        let args = StateArgs {
            state_file: Some(Path::new("-")),
            ..nothing()
        };
        let mut pipe = stdin("", false);

        let error = resolve_state(&args, None, &mut pipe, true).expect_err("stdin is taken");

        assert!(error.to_string().contains("--state-file"), "{error}");
    }

    #[test]
    fn the_model_is_the_flag_then_the_document_then_the_environment_then_the_default() {
        assert_eq!(
            resolve_model(Some("flag"), Some("doc"), Some("env")),
            "flag"
        );
        assert_eq!(resolve_model(None, Some("doc"), Some("env")), "doc");
        assert_eq!(resolve_model(None, None, Some("env")), "env");
        assert_eq!(resolve_model(None, None, None), wire::DEFAULT_MODEL);
    }

    #[test]
    fn a_base_url_without_a_scheme_is_refused_by_name() {
        let error = resolve_base_url("api.typesafe.ai").expect_err("that is a relative URL");

        assert_eq!(error.exit_code(), 2);
        assert!(error.to_string().contains("http://"), "{error}");
        assert!(error.to_string().contains("api.typesafe.ai"), "{error}");
    }

    #[test]
    fn a_trailing_slash_in_the_base_url_is_trimmed() {
        assert_eq!(
            resolve_base_url("https://api.typesafe.ai/").expect("a scheme is there"),
            "https://api.typesafe.ai"
        );
        assert_eq!(
            resolve_base_url("http://127.0.0.1:8080").expect("a scheme is there"),
            "http://127.0.0.1:8080"
        );
        assert!(resolve_base_url("https://").is_err());
    }

    #[test]
    fn an_option_splits_on_the_first_equals() {
        let options = vec!["billing=a=b".to_string(), "sales=1+1=2".to_string()];

        let question = choice_question("which team?", &options).expect("two options");

        let expected = json!({"billing": "a=b", "sales": "1+1=2"});
        assert_eq!(
            question.criteria.as_ref().map(Criteria::to_value),
            Some(expected)
        );
    }

    #[test]
    fn a_bare_option_sends_null_and_an_empty_description_is_not_the_same_thing() {
        let options = vec!["sales".to_string(), "support=".to_string()];

        let question = choice_question("which team?", &options).expect("two options");

        assert_eq!(
            question.criteria.as_ref().map(Criteria::to_value),
            Some(json!({"sales": null, "support": ""}))
        );
    }

    #[test]
    fn a_repeated_option_is_refused_by_name() {
        let options = vec![
            "billing=payments".to_string(),
            "billing=invoices".to_string(),
        ];

        let error = choice_question("which team?", &options).expect_err("a key cannot repeat");

        assert_eq!(error.exit_code(), 2);
        assert!(error.to_string().contains("\"billing\""), "{error}");
    }

    #[test]
    fn a_choice_with_no_options_is_refused_by_the_question_validator() {
        let question = choice_question("which team?", &[]).expect("the builder does not count");

        let error = question
            .validate("answer")
            .expect_err("a choice needs an option");

        assert_eq!(
            error.to_string(),
            "question \"answer\": a choice needs at least one option (none were given)"
        );
    }

    #[test]
    fn yes_and_no_become_the_two_noul_criteria() {
        let question = noul_question("is it urgent?", Some("time-sensitive"), Some("routine"));

        assert_eq!(
            question.criteria.as_ref().map(Criteria::to_value),
            Some(json!({"true": "time-sensitive", "false": "routine"}))
        );
    }

    #[test]
    fn a_noul_without_descriptions_has_no_criteria_at_all() {
        let question = noul_question("is it urgent?", None, None);

        assert!(
            question.criteria.is_none(),
            "the key is omitted, not sent empty"
        );
        assert_eq!(
            question.to_value(),
            json!({"type": "noul", "instructions": "is it urgent?"})
        );
    }

    #[test]
    fn only_the_descriptions_that_were_given_are_sent() {
        let question = noul_question("is it urgent?", Some("time-sensitive"), None);

        assert_eq!(
            question.criteria.as_ref().map(Criteria::to_value),
            Some(json!({"true": "time-sensitive"}))
        );
    }

    #[test]
    fn levels_keep_the_order_they_were_given_in() {
        let levels = vec![
            "calm".to_string(),
            "frustrated".to_string(),
            "very angry".to_string(),
        ];

        let question = score_question("how frustrated?", &levels);

        assert_eq!(
            question.criteria.as_ref().map(Criteria::to_value),
            Some(json!(["calm", "frustrated", "very angry"]))
        );
    }

    #[test]
    fn a_document_is_read_from_a_file_and_leaves_stdin_alone() {
        let mut file = tempfile::NamedTempFile::new().expect("a temporary file");
        file.write_all(b"{\"questions\":{}}").expect("write");
        let mut pipe = stdin("the state", false);

        let (text, consumed) = read_document(Some(file.path()), &mut pipe).expect("read");

        assert_eq!(text, "{\"questions\":{}}");
        assert!(!consumed, "a file does not consume stdin");
    }

    #[test]
    fn a_document_with_no_file_is_read_from_stdin() {
        let mut pipe = stdin("{\"questions\":{}}", false);

        let (text, consumed) = read_document(None, &mut pipe).expect("read");

        assert_eq!(text, "{\"questions\":{}}");
        assert!(consumed, "stdin was the document");
    }

    #[test]
    fn read_document_understands_a_dash_as_stdin() {
        let mut pipe = stdin("{\"questions\":{}}", false);

        let (text, consumed) = read_document(Some(Path::new("-")), &mut pipe).expect("read");

        assert_eq!(text, "{\"questions\":{}}");
        assert!(consumed);
    }

    #[test]
    fn no_document_with_a_terminal_on_stdin_is_a_usage_error() {
        let mut terminal = stdin("", true);

        let error = read_document(None, &mut terminal).expect_err("nothing to read");

        assert_eq!(error.exit_code(), 2);
        assert!(error.to_string().contains("name a file"), "{error}");
    }

    #[test]
    fn an_unreadable_document_names_the_path() {
        let mut terminal = stdin("", true);

        let error = read_document(
            Some(Path::new("/nonexistent/questions.json")),
            &mut terminal,
        )
        .expect_err("no such file");

        assert!(error.to_string().contains("questions.json"), "{error}");
    }

    #[test]
    fn the_stdin_type_answers_both_questions_a_state_branch_asks() {
        let terminal = stdin("", true);
        assert!(terminal.is_terminal());

        let mut pipe = stdin("data", false);
        assert!(!pipe.is_terminal());
        assert_eq!(pipe.read_all().expect("read"), "data");
    }
}

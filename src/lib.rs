//! Ask Jev for a typed decision, and get JSON a shell script can branch on.
//!
//! `decide` is a thin, opinionated transport over `TypeSafe`'s System One ("Jev") model:
//! one state, a set of typed questions, and one JSON response with no text to parse. It
//! is written for two callers — a shell script a human wrote, and a language model driving
//! a bash tool — and the second one is why every mistake is refused locally and by name.
//!
//! Nothing in this crate touches a stream
//! ([D14](../docs/design.md#d14-all-io-is-injected-so-nothing-in-the-crate-prints)): [`run`]
//! receives an [`Io`] holding stdin, stdout, and stderr, so what lands on stdout is an
//! assertion in a unit test rather than something captured from a subprocess.
//!
//! ```rust
//! use std::io::Cursor;
//!
//! use clap::Parser;
//!
//! use decide::{Cli, Io, Stdin};
//!
//! // A dry run needs no credential and opens no connection: it prints the request body.
//! let cli = Cli::try_parse_from([
//!     "decide", "choice", "which team?", "--no-state",
//!     "--option", "billing", "--option", "technical", "--dry-run",
//! ])
//! .unwrap();
//! let mut io = Io {
//!     stdin: Stdin::new(Cursor::new(Vec::new()), true),
//!     stdout: Vec::new(),
//!     stderr: Vec::new(),
//! };
//!
//! decide::run(&cli, &mut io)?;
//!
//! assert_eq!(
//!     String::from_utf8(io.stdout).unwrap(),
//!     concat!(
//!         r#"{"model":"jev-latest","questions":{"answer":{"criteria":"#,
//!         r#"{"billing":null,"technical":null},"instructions":"which team?","#,
//!         r#""type":"choice"}},"state":null}"#,
//!         "\n",
//!     ),
//! );
//! # Ok::<(), decide::DecideError>(())
//! ```

#![forbid(unsafe_code)]

pub mod cli;
pub mod client;
pub mod error;
pub mod input;
pub mod report;
pub mod wire;

use std::collections::BTreeMap;
use std::io::{Read, Write};

use serde_json::Value;

pub use cli::{Cli, Command, EvalArgs, GlobalArgs};
pub use error::DecideError;
pub use input::Stdin;

/// The three streams a run may touch, injected rather than reached for.
pub struct Io<In, Out, Err> {
    /// Standard input, with whether it is a terminal.
    pub stdin: Stdin<In>,
    /// The response, and nothing else.
    pub stdout: Out,
    /// Progress and errors.
    pub stderr: Err,
}

/// Run one invocation and write its result to `io.stdout`.
///
/// `--dry-run` prints the request body and returns without a credential or a connection;
/// everything else resolves the credential, makes one call (retrying per
/// [D10](docs/design.md#d10-retries-on-by-default-with-the-sdks-policy)), and prints the
/// response, a value selected out of it, or the one answer.
///
/// # Errors
///
/// Every [`DecideError`], whose [`exit_code`](DecideError::exit_code) separates a wrong
/// invocation from a failed evaluation.
pub fn run<In: Read, Out: Write, Err: Write>(
    cli: &Cli,
    io: &mut Io<In, Out, Err>,
) -> Result<(), DecideError> {
    match &cli.command {
        Command::Models => run_models(cli, io),
        _ => run_eval(cli, io),
    }
}

/// The evaluation path: one request, then its response.
fn run_eval<In: Read, Out: Write, Err: Write>(
    cli: &Cli,
    io: &mut Io<In, Out, Err>,
) -> Result<(), DecideError> {
    let Some(eval) = cli.command.eval() else {
        // `run` dispatches `models` to `run_models` before it gets here. Falling back to
        // that path keeps the arm honest without a panic.
        return run_models(cli, io);
    };

    let prepared = prepare(&cli.command, &mut io.stdin)?;
    let state = input::resolve_state(
        &state_args(eval),
        prepared.document_state.as_ref(),
        &mut io.stdin,
        prepared.stdin_consumed,
    )?;

    let environment_model = std::env::var("TYPESAFE_DEFAULT_MODEL").ok();
    let model = input::resolve_model(
        eval.model.as_deref(),
        prepared.document_model.as_deref(),
        environment_model.as_deref(),
    );
    let request = wire::Request::new(state, model, prepared.questions)?;

    // "The value" of several answers is not defined, and a caller that asked for it must
    // not pay for a request to find that out (D9).
    if eval.value || eval.field.is_some() {
        let flag = if eval.value { "--value" } else { "--field" };
        if request.only_question_id().is_none() {
            return Err(DecideError::OneAnswerRequired {
                flag,
                count: request.question_count(),
            });
        }
    }

    if eval.dry_run {
        let text = report::render_json(&request.to_value(), cli.global.pretty);
        return emit(&mut io.stdout, &text);
    }

    let config = client_config(&cli.global)?;
    let body = report::render_json(&request.to_value(), false);
    let response_text = client::post_systemone(&config, &body, &mut io.stderr)?;

    let response = wire::Response::from_slice(response_text.as_bytes(), &request)?;
    let text = render_response(&response, &request, eval, &cli.global)?;
    emit(&mut io.stdout, &text)
}

/// The `models` path: one GET, printed whole or selected from.
fn run_models<In: Read, Out: Write, Err: Write>(
    cli: &Cli,
    io: &mut Io<In, Out, Err>,
) -> Result<(), DecideError> {
    let config = client_config(&cli.global)?;
    let body = client::get_models(&config, &mut io.stderr)?;
    let response = wire::ModelsResponse::from_slice(body.as_bytes())?;

    let text = match &cli.global.select {
        Some(path) => {
            report::render_value(report::select(&response.value, path)?, cli.global.pretty)
        }
        None => report::render_json(&response.value, cli.global.pretty),
    };
    emit(&mut io.stdout, &text)
}

/// The questions a request asks, and what the document supplied besides them.
struct Prepared {
    /// The questions, keyed by their ids.
    questions: BTreeMap<String, wire::Question>,
    /// The document's own state, if it carried one.
    document_state: Option<Value>,
    /// The document's model, if it named one.
    document_model: Option<String>,
    /// Whether stdin was read as the request document.
    stdin_consumed: bool,
}

impl Prepared {
    /// A request built from one shorthand question.
    fn single(id: &str, question: wire::Question) -> Self {
        let mut questions = BTreeMap::new();
        questions.insert(id.to_string(), question);
        Self {
            questions,
            document_state: None,
            document_model: None,
            stdin_consumed: false,
        }
    }
}

/// Build the questions from whichever source the subcommand uses.
fn prepare<In: Read>(command: &Command, stdin: &mut Stdin<In>) -> Result<Prepared, DecideError> {
    match command {
        Command::Ask { file, .. } => {
            let (text, stdin_consumed) = input::read_document(file.as_deref(), stdin)?;
            let document = wire::Document::from_slice(text.as_bytes())?;
            Ok(Prepared {
                questions: document.questions,
                document_state: document.state,
                document_model: document.model,
                stdin_consumed,
            })
        }
        Command::Noul {
            instruction,
            id,
            yes,
            no,
            ..
        } => Ok(Prepared::single(
            id,
            input::noul_question(instruction, yes.as_deref(), no.as_deref()),
        )),
        Command::Choice {
            instruction,
            id,
            option,
            ..
        } => Ok(Prepared::single(
            id,
            input::choice_question(instruction, option)?,
        )),
        Command::Score {
            instruction,
            id,
            level,
            ..
        } => Ok(Prepared::single(
            id,
            input::score_question(instruction, level),
        )),
        // `models` asks no question; `run` sends it to `run_models` first, so this is a
        // fallback rather than a path, and the request validator refuses it by name.
        Command::Models => Ok(Prepared {
            questions: BTreeMap::new(),
            document_state: None,
            document_model: None,
            stdin_consumed: false,
        }),
    }
}

/// The state flags, in the shape the resolver wants them.
fn state_args(eval: &EvalArgs) -> input::StateArgs<'_> {
    input::StateArgs {
        state: eval.state.as_deref(),
        state_file: eval.state_file.as_deref(),
        state_json: eval.state_json,
        no_state: eval.no_state,
    }
}

/// Everything a call needs, credential included.
fn client_config(global: &GlobalArgs) -> Result<client::ClientConfig, DecideError> {
    Ok(client::ClientConfig {
        base_url: input::resolve_base_url(&global.base_url)?,
        api_key: client::resolve_api_key(
            std::env::var("TYPESAFE_API_KEY").ok(),
            global.api_key_file.as_deref(),
        )?,
        timeout_secs: global.timeout,
        retries: global.retries,
        backoff_ms: global.backoff_ms,
        verbose: global.verbose,
    })
}

/// Render the one thing the caller asked to see, in the precedence the conflicts leave
/// open.
fn render_response(
    response: &wire::Response,
    request: &wire::Request,
    eval: &EvalArgs,
    global: &GlobalArgs,
) -> Result<String, DecideError> {
    if let Some(path) = &global.select {
        return report::select(&response.value, path)
            .map(|value| report::render_value(value, global.pretty));
    }
    if eval.value {
        let answer = report::one_answer(response, request, "--value")?;
        return report::answer_value(answer)
            .map(|value| report::render_value(value, global.pretty));
    }
    if let Some(path) = &eval.field {
        let answer = report::one_answer(response, request, "--field")?;
        let value = report::walk(&answer.value, path, "--field")?;
        return Ok(report::render_value(value, global.pretty));
    }
    Ok(report::render_json(&response.value, global.pretty))
}

/// Write the output, once, whole, and with the trailing newline a shell expects.
fn emit<Out: Write>(out: &mut Out, text: &str) -> Result<(), DecideError> {
    out.write_all(text.as_bytes())
        .and_then(|()| out.write_all(b"\n"))
        .and_then(|()| out.flush())
        .map_err(|error| DecideError::Output {
            reason: error.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use clap::Parser as _;
    use serde_json::{Value, json};

    use super::*;

    /// What one invocation produced.
    struct Run {
        stdout: String,
        stderr: String,
        result: Result<(), DecideError>,
    }

    /// Run an invocation against in-memory streams.
    fn run(args: &[&str], stdin_text: &str, terminal: bool) -> Run {
        let cli = Cli::try_parse_from(args).expect("the test's invocation parses");
        let mut io = Io {
            stdin: Stdin::new(Cursor::new(stdin_text.as_bytes().to_vec()), terminal),
            stdout: Vec::new(),
            stderr: Vec::new(),
        };
        let result = crate::run(&cli, &mut io);
        Run {
            stdout: String::from_utf8(io.stdout).expect("stdout is text"),
            stderr: String::from_utf8(io.stderr).expect("stderr is text"),
            result,
        }
    }

    /// A request document in a temporary file, and the directory that keeps it alive.
    fn document(text: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let path = dir.path().join("questions.json");
        std::fs::write(&path, text).expect("write the document");
        (dir, path)
    }

    /// The request body a dry run printed, as a value.
    fn printed_request(outcome: &Run) -> Value {
        serde_json::from_str(outcome.stdout.trim_end()).expect("the dry run printed JSON")
    }

    #[test]
    fn a_dry_run_prints_the_request_body_and_nothing_else() {
        let outcome = run(
            &[
                "decide",
                "choice",
                "which team should handle this?",
                "--no-state",
                "--option",
                "billing=payments and invoices",
                "--option",
                "technical=bugs and outages",
                "--option",
                "sales",
                "--id",
                "department",
                "--dry-run",
            ],
            "",
            true,
        );

        assert!(outcome.result.is_ok(), "{:?}", outcome.result);
        assert!(
            outcome.stderr.is_empty(),
            "a dry run writes nothing to stderr"
        );
        assert_eq!(
            printed_request(&outcome),
            json!({
                "model": "jev-latest",
                "state": null,
                "questions": {
                    "department": {
                        "type": "choice",
                        "instructions": "which team should handle this?",
                        "criteria": {
                            "billing": "payments and invoices",
                            "technical": "bugs and outages",
                            "sales": null
                        }
                    }
                }
            })
        );
    }

    #[test]
    fn a_dry_run_needs_no_credential_and_opens_no_connection() {
        // The base URL points nowhere, which a real call would notice.
        let outcome = run(
            &[
                "decide",
                "noul",
                "is it urgent?",
                "--no-state",
                "--base-url",
                "http://127.0.0.1:1",
                "--dry-run",
            ],
            "",
            true,
        );

        assert!(outcome.result.is_ok(), "a dry run needs no credential");
        assert_eq!(
            printed_request(&outcome)
                .pointer("/questions/answer/type")
                .and_then(Value::as_str),
            Some("noul")
        );
    }

    #[test]
    fn a_dry_run_is_the_bytes_a_real_call_would_send() {
        let outcome = run(
            &[
                "decide",
                "score",
                "how much?",
                "--no-state",
                "--level",
                "a",
                "--level",
                "b",
                "--dry-run",
            ],
            "",
            true,
        );

        let value = printed_request(&outcome);
        assert_eq!(
            outcome.stdout.trim_end(),
            report::render_json(&value, false),
            "the printed body is the compact body"
        );
        assert!(outcome.stdout.ends_with('\n'), "and it ends with a newline");
    }

    #[test]
    fn pretty_indents_the_dry_run_body() {
        let outcome = run(
            &[
                "decide",
                "noul",
                "is it so?",
                "--no-state",
                "--pretty",
                "--dry-run",
            ],
            "",
            true,
        );

        assert!(
            outcome.stdout.contains("\n  \"model\""),
            "{}",
            outcome.stdout
        );
    }

    #[test]
    fn the_state_is_read_from_stdin_when_the_document_has_none() {
        let (_dir, path) =
            document(r#"{"questions":{"q":{"type":"noul","instructions":"is it so?"}}}"#);

        let outcome = run(
            &["decide", "ask", path.to_str().expect("utf-8"), "--dry-run"],
            "Help! My payouts have been failing.",
            false,
        );

        assert!(outcome.result.is_ok(), "{:?}", outcome.result);
        assert_eq!(
            printed_request(&outcome)
                .get("state")
                .and_then(Value::as_str),
            Some("Help! My payouts have been failing.")
        );
    }

    #[test]
    fn a_piped_state_and_a_state_in_the_document_are_refused_before_anything_is_printed() {
        let (_dir, path) = document(
            r#"{"state":"in the document","questions":{"q":{"type":"noul","instructions":"x"}}}"#,
        );

        // `--state-file -` names the pipe as the state, and the document names one too.
        let outcome = run(
            &[
                "decide",
                "ask",
                path.to_str().expect("utf-8"),
                "--state-file",
                "-",
                "--dry-run",
            ],
            "on stdin",
            false,
        );

        let error = outcome.result.expect_err("two sources");
        assert_eq!(error.exit_code(), 2);
        assert!(
            outcome.stdout.is_empty(),
            "a failed run leaves stdout empty"
        );
    }

    #[test]
    fn a_document_with_an_unknown_key_is_refused_before_anything_is_printed() {
        let (_dir, path) = document(r#"{"question":{"q":{"type":"noul","instructions":"x"}}}"#);

        let outcome = run(
            &[
                "decide",
                "ask",
                path.to_str().expect("utf-8"),
                "--no-state",
                "--dry-run",
            ],
            "",
            true,
        );

        let error = outcome.result.expect_err("the key is a typo");
        assert_eq!(error.exit_code(), 2);
        assert!(error.to_string().contains("\"question\""), "{error}");
        assert!(outcome.stdout.is_empty());
        assert!(
            outcome.stderr.is_empty(),
            "`run` reports through its return value, not by writing"
        );
    }

    #[test]
    fn a_misspelled_criteria_is_refused_by_name() {
        let (_dir, path) = document(
            r#"{"questions":{"department":{"type":"choice","instructions":"which?",
                "criteriaa":{"billing":"payments"}}}}"#,
        );

        let outcome = run(
            &[
                "decide",
                "ask",
                path.to_str().expect("utf-8"),
                "--no-state",
                "--dry-run",
            ],
            "",
            true,
        );

        let error = outcome
            .result
            .expect_err("a rubric-less question is not a question");
        assert!(error.to_string().contains("criteriaa"), "{error}");
        assert!(outcome.stdout.is_empty());
    }

    #[test]
    fn a_base_url_without_a_scheme_is_refused_before_a_call() {
        let outcome = run(
            &[
                "decide",
                "noul",
                "is it so?",
                "--no-state",
                "--base-url",
                "api.typesafe.ai",
            ],
            "",
            true,
        );

        let error = outcome.result.expect_err("a relative URL goes nowhere");
        assert_eq!(error.exit_code(), 2);
        assert!(outcome.stdout.is_empty());
    }

    #[test]
    fn a_value_is_refused_before_a_call_when_the_document_asks_three_questions() {
        let (_dir, path) = document(include_str!("../tests/data/request_quickstart.json"));

        // The fixture carries its own state, so no state flag is named here.
        let outcome = run(
            &["decide", "ask", path.to_str().expect("utf-8"), "--value"],
            "",
            true,
        );

        let error = outcome
            .result
            .expect_err("the value of three answers is not defined");
        assert_eq!(error.exit_code(), 2);
        assert!(error.to_string().contains("asked 3"), "{error}");
        assert!(outcome.stdout.is_empty());
    }

    #[test]
    fn the_model_flag_overrides_the_document() {
        let (_dir, path) = document(
            r#"{"model":"from-the-document","questions":{"q":{"type":"noul","instructions":"x"}}}"#,
        );
        let file = path.to_str().expect("utf-8");

        let documented = run(
            &["decide", "ask", file, "--no-state", "--dry-run"],
            "",
            true,
        );
        assert_eq!(
            printed_request(&documented)
                .get("model")
                .and_then(Value::as_str),
            Some("from-the-document")
        );

        let overridden = run(
            &[
                "decide",
                "ask",
                file,
                "--no-state",
                "--model",
                "from-the-flag",
                "--dry-run",
            ],
            "",
            true,
        );
        assert_eq!(
            printed_request(&overridden)
                .get("model")
                .and_then(Value::as_str),
            Some("from-the-flag")
        );
    }

    #[test]
    fn ask_reads_the_document_from_stdin_when_no_file_is_named() {
        let outcome = run(
            &["decide", "ask", "--no-state", "--dry-run"],
            r#"{"questions":{"q":{"type":"noul","instructions":"is it so?"}}}"#,
            false,
        );

        assert!(outcome.result.is_ok(), "{:?}", outcome.result);
        assert_eq!(
            printed_request(&outcome)
                .pointer("/questions/q/instructions")
                .and_then(Value::as_str),
            Some("is it so?")
        );
    }

    #[test]
    fn ask_with_no_file_and_a_terminal_says_to_name_one() {
        let outcome = run(&["decide", "ask", "--no-state"], "", true);

        let error = outcome.result.expect_err("there is nothing to read");
        assert_eq!(error.exit_code(), 2);
        assert!(error.to_string().contains("name a file"), "{error}");
    }

    #[test]
    fn a_question_id_that_is_empty_is_refused_by_name() {
        let outcome = run(
            &[
                "decide",
                "noul",
                "is it so?",
                "--no-state",
                "--id",
                "",
                "--dry-run",
            ],
            "",
            true,
        );

        let error = outcome.result.expect_err("an empty id cannot be addressed");
        assert!(error.to_string().contains("id cannot be empty"), "{error}");
    }

    #[test]
    fn the_shorthand_carries_its_own_flags_into_the_request() {
        let outcome = run(
            &[
                "decide",
                "noul",
                "does this message convey urgency?",
                "--no-state",
                "--yes",
                "explicitly time-sensitive",
                "--no",
                "no urgency expressed",
                "--id",
                "urgency",
                "--dry-run",
            ],
            "",
            true,
        );

        assert_eq!(
            printed_request(&outcome),
            json!({
                "model": "jev-latest",
                "state": null,
                "questions": {
                    "urgency": {
                        "type": "noul",
                        "instructions": "does this message convey urgency?",
                        "criteria": {
                            "true": "explicitly time-sensitive",
                            "false": "no urgency expressed"
                        }
                    }
                }
            })
        );
    }

    #[test]
    fn a_state_file_supplies_the_state_for_a_request_document() {
        let (dir, path) = document(r#"{"questions":{"q":{"type":"noul","instructions":"x"}}}"#);
        let state_path = dir.path().join("ticket.txt");
        std::fs::write(&state_path, "the payouts are failing").expect("write the state");

        let outcome = run(
            &[
                "decide",
                "ask",
                path.to_str().expect("utf-8"),
                "--state-file",
                state_path.to_str().expect("utf-8"),
                "--dry-run",
            ],
            "",
            true,
        );

        assert!(outcome.result.is_ok(), "{:?}", outcome.result);
        assert_eq!(
            printed_request(&outcome)
                .get("state")
                .and_then(Value::as_str),
            Some("the payouts are failing")
        );
    }
}

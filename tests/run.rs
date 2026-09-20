//! What a whole invocation does, with the response coming from a local socket.
//!
//! This is the layer the stdout contract lives at: the exact bytes, whether `--verbose`
//! changes them, and whether a failed run leaves them empty. The streams are in-memory,
//! so nothing here captures a subprocess.

// An integration test is its own crate, which `clippy.toml`'s `allow-expect-in-tests`
// cannot see: as in `src`'s unit tests, a panic here is the assertion mechanism.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::io::{Cursor, Write as _};

use clap::Parser as _;
use serde_json::Value;

use decide::{Cli, DecideError, Io, Stdin};
use support::{FakeServer, Reply};

/// The vendor's three-answer response, copied from the documentation.
const ANSWERS: &str = include_str!("data/response_answers.json");
/// The vendor's models response.
const MODELS: &str = include_str!("data/models.json");

/// What one invocation produced.
struct Outcome {
    stdout: String,
    stderr: String,
    result: Result<(), DecideError>,
}

/// Run an invocation against in-memory streams.
fn run(args: &[&str], stdin_text: &str, terminal: bool) -> Outcome {
    let cli = Cli::try_parse_from(args).expect("the test's invocation parses");
    let mut io = Io {
        stdin: Stdin::new(Cursor::new(stdin_text.as_bytes().to_vec()), terminal),
        stdout: Vec::new(),
        stderr: Vec::new(),
    };
    let result = decide::run(&cli, &mut io);
    Outcome {
        stdout: String::from_utf8(io.stdout).expect("stdout is text"),
        stderr: String::from_utf8(io.stderr).expect("stderr is text"),
        result,
    }
}

/// A credential in a file, and the guard that keeps the file alive.
///
/// A flag is the only way to point a test at a credential: setting an environment
/// variable would change every test running in this process.
fn key_file() -> (tempfile::NamedTempFile, String) {
    let mut file = tempfile::NamedTempFile::new().expect("a temporary file");
    file.write_all(b"sekrit-token\n").expect("write the key");
    let path = file.path().to_string_lossy().into_owned();
    (file, path)
}

/// A request document in a file that outlives the run.
fn document(text: &str) -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("questions.json");
    std::fs::write(&path, text).expect("write the document");
    let path = path.to_string_lossy().into_owned();
    (dir, path)
}

/// The vendor's quickstart document, which carries its own state and model.
fn quickstart() -> (tempfile::TempDir, String) {
    document(include_str!("data/request_quickstart.json"))
}

/// One noul question, for the `--value` and `--field` cases.
fn one_noul() -> (tempfile::TempDir, String) {
    document(concat!(
        r#"{"state":"Help!","questions":{"is_urgent":"#,
        r#"{"type":"noul","instructions":"Does this convey urgency?"}}}"#,
    ))
}

/// One choice question, for `--field`.
fn one_choice() -> (tempfile::TempDir, String) {
    document(
        r#"{"state":"Help!","questions":{"department":{"type":"choice","instructions":"Which team?",
            "criteria":{"billing":"payments","technical":"bugs"}}}}"#,
    )
}

/// The response to `one_choice`, whose answer hides its option behind a field.
const CHOICE_ANSWER: &str = r#"{"model":"jev-1.13.0","answers":{"department":
    {"type":"choice","choice":"technical","probabilities":{"billing":0.12,"technical":0.88},
     "confidence":0.81}},"usage":{"input_tokens":296,"output_tokens":20}}"#;

#[test]
fn the_response_is_printed_as_one_json_value_with_its_keys_sorted() {
    let server = FakeServer::start(vec![Reply::json(ANSWERS)]);
    let (_key, key) = key_file();
    let (_dir, doc) = quickstart();

    let outcome = run(
        &[
            "decide",
            "ask",
            &doc,
            "--base-url",
            &server.url(),
            "--api-key-file",
            &key,
        ],
        "",
        true,
    );

    assert!(outcome.result.is_ok(), "{:?}", outcome.result);
    assert!(
        outcome.stdout.ends_with('\n'),
        "the line ends with a newline"
    );
    let printed: Value =
        serde_json::from_str(outcome.stdout.trim_end()).expect("stdout is one JSON value");
    assert_eq!(
        printed,
        serde_json::from_str::<Value>(ANSWERS).expect("the fixture")
    );
    assert!(
        outcome.stdout.starts_with(r#"{"answers":"#),
        "the keys are sorted, so answers comes first: {}",
        outcome.stdout
    );
}

#[test]
fn the_request_the_server_received_is_the_document_that_was_written() {
    let server = FakeServer::start(vec![Reply::json(ANSWERS)]);
    let (_key, key) = key_file();
    let (_dir, doc) = quickstart();

    run(
        &[
            "decide",
            "ask",
            &doc,
            "--base-url",
            &server.url(),
            "--api-key-file",
            &key,
        ],
        "",
        true,
    );

    let seen = server.seen();
    let request = seen.first().expect("one request was seen");
    assert_eq!(request.header("authorization"), Some("Bearer sekrit-token"));
    assert_eq!(request.path, "/v1/systemone");
    let sent: Value = serde_json::from_str(&request.body).expect("the body is JSON");
    assert_eq!(
        sent.pointer("/questions/department/type")
            .and_then(Value::as_str),
        Some("choice")
    );
    assert_eq!(
        sent.get("model").and_then(Value::as_str),
        Some("jev-latest")
    );
}

#[test]
fn verbose_does_not_change_a_byte_of_stdout() {
    let server = FakeServer::start(vec![Reply::json(ANSWERS), Reply::json(ANSWERS)]);
    let (_key, key) = key_file();
    let (_dir, doc) = quickstart();
    let url = server.url();
    let base = [
        "decide",
        "ask",
        doc.as_str(),
        "--base-url",
        url.as_str(),
        "--api-key-file",
        key.as_str(),
    ];

    let quiet = run(&base, "", true);

    let mut loud_args = base.to_vec();
    loud_args.push("--verbose");
    let loud = run(&loud_args, "", true);

    assert_eq!(quiet.stdout, loud.stdout, "--verbose never touches stdout");
    assert!(
        quiet.stderr.is_empty(),
        "nothing is written without --verbose"
    );
    assert!(
        !loud.stderr.is_empty(),
        "--verbose reports progress on stderr"
    );
    assert!(loud.stderr.contains("POST"), "{}", loud.stderr);
}

#[test]
fn a_status_error_leaves_stdout_empty_and_exits_one() {
    let server = FakeServer::start(vec![Reply::status(422, r#"{"detail":"nope"}"#)]);
    let (_key, key) = key_file();
    let (_dir, doc) = quickstart();

    let outcome = run(
        &[
            "decide",
            "ask",
            &doc,
            "--base-url",
            &server.url(),
            "--api-key-file",
            &key,
        ],
        "",
        true,
    );

    let error = outcome.result.expect_err("the API refused the request");
    assert_eq!(error.exit_code(), 1);
    assert!(
        outcome.stdout.is_empty(),
        "a failed run leaves stdout empty"
    );
    assert!(error.to_string().contains("422"), "{error}");
    assert!(error.to_string().contains("nope"), "{error}");
}

#[test]
fn a_response_that_breaks_the_answer_contract_is_exit_one_with_empty_stdout() {
    let server = FakeServer::start(vec![Reply::json(
        r#"{"model":"jev-1.13.0","answers":{"is_urgent":{"type":"choice","choice":"yes"}}}"#,
    )]);
    let (_key, key) = key_file();
    let (_dir, doc) = one_noul();

    let outcome = run(
        &[
            "decide",
            "ask",
            &doc,
            "--base-url",
            &server.url(),
            "--api-key-file",
            &key,
        ],
        "",
        true,
    );

    let error = outcome
        .result
        .expect_err("the answer disagrees with the question");
    assert_eq!(error.exit_code(), 1);
    assert!(error.to_string().contains("asked for a noul"), "{error}");
    assert!(outcome.stdout.is_empty());
}

#[test]
fn a_response_that_is_not_json_is_exit_one_with_empty_stdout() {
    let server = FakeServer::start(vec![Reply::json("<html>a proxy said no</html>")]);
    let (_key, key) = key_file();
    let (_dir, doc) = one_noul();

    let outcome = run(
        &[
            "decide",
            "ask",
            &doc,
            "--base-url",
            &server.url(),
            "--api-key-file",
            &key,
        ],
        "",
        true,
    );

    let error = outcome.result.expect_err("that is not JSON");
    assert_eq!(error.exit_code(), 1);
    assert!(outcome.stdout.is_empty());
}

#[test]
fn select_reaches_a_value_anywhere_in_the_response() {
    let server = FakeServer::start(vec![
        Reply::json(ANSWERS),
        Reply::json(ANSWERS),
        Reply::json(ANSWERS),
        Reply::json(ANSWERS),
    ]);
    let (_key, key) = key_file();
    let (_dir, doc) = quickstart();
    let url = server.url();
    let base = [
        "decide",
        "ask",
        doc.as_str(),
        "--base-url",
        url.as_str(),
        "--api-key-file",
        key.as_str(),
    ];

    for (path, expected) in [
        ("model", "jev-1.13.0\n"),
        ("usage.input_tokens", "296\n"),
        ("answers.department.choice", "billing\n"),
        ("answers.frustration.legend.2", "Very angry\n"),
    ] {
        let mut args = base.to_vec();
        args.push("--select");
        args.push(path);
        let outcome = run(&args, "", true);

        assert!(outcome.result.is_ok(), "{path}: {:?}", outcome.result);
        assert_eq!(outcome.stdout, expected, "selecting {path}");
    }
}

#[test]
fn a_select_that_misses_names_the_segment_and_the_keys_available() {
    let server = FakeServer::start(vec![Reply::json(ANSWERS)]);
    let (_key, key) = key_file();
    let (_dir, doc) = quickstart();

    let outcome = run(
        &[
            "decide",
            "ask",
            &doc,
            "--base-url",
            &server.url(),
            "--api-key-file",
            &key,
            "--select",
            "answers.urgency.noul",
        ],
        "",
        true,
    );

    let error = outcome.result.expect_err("there is no urgency");
    assert_eq!(error.exit_code(), 2);
    let rendered = error.to_string();
    assert!(rendered.contains("no key \"urgency\""), "{rendered}");
    assert!(rendered.contains("\"is_urgent\""), "{rendered}");
    assert!(
        outcome.stdout.is_empty(),
        "a failed run leaves stdout empty"
    );
}

#[test]
fn value_prints_the_one_answer_without_the_script_knowing_its_name() {
    let server = FakeServer::start(vec![Reply::json(ANSWERS)]);
    let (_key, key) = key_file();
    let (_dir, doc) = one_noul();

    let outcome = run(
        &[
            "decide",
            "ask",
            &doc,
            "--base-url",
            &server.url(),
            "--api-key-file",
            &key,
            "--value",
        ],
        "",
        true,
    );

    assert!(outcome.result.is_ok(), "{:?}", outcome.result);
    assert_eq!(outcome.stdout, "0.95\n");
}

#[test]
fn value_prints_the_chosen_option_and_not_its_name() {
    let server = FakeServer::start(vec![Reply::json(CHOICE_ANSWER)]);
    let (_key, key) = key_file();
    let (_dir, doc) = one_choice();

    let outcome = run(
        &[
            "decide",
            "ask",
            &doc,
            "--base-url",
            &server.url(),
            "--api-key-file",
            &key,
            "--value",
        ],
        "",
        true,
    );

    assert_eq!(outcome.stdout, "technical\n");
}

#[test]
fn field_walks_from_the_one_answer() {
    // One reply per field the loop asks for.
    let server = FakeServer::start(vec![
        Reply::json(CHOICE_ANSWER),
        Reply::json(CHOICE_ANSWER),
        Reply::json(CHOICE_ANSWER),
    ]);
    let (_key, key) = key_file();
    let (_dir, doc) = one_choice();
    let url = server.url();
    let base = [
        "decide",
        "ask",
        doc.as_str(),
        "--base-url",
        url.as_str(),
        "--api-key-file",
        key.as_str(),
    ];

    for (path, expected) in [
        ("confidence", "0.81\n"),
        ("probabilities.technical", "0.88\n"),
        ("choice", "technical\n"),
    ] {
        let mut args = base.to_vec();
        args.push("--field");
        args.push(path);
        let outcome = run(&args, "", true);

        assert!(outcome.result.is_ok(), "{path}: {:?}", outcome.result);
        assert_eq!(outcome.stdout, expected, "field {path}");
    }
}

#[test]
fn a_misspelled_field_names_the_flag() {
    let server = FakeServer::start(vec![Reply::json(CHOICE_ANSWER)]);
    let (_key, key) = key_file();
    let (_dir, doc) = one_choice();

    let outcome = run(
        &[
            "decide",
            "ask",
            &doc,
            "--base-url",
            &server.url(),
            "--api-key-file",
            &key,
            "--field",
            "confidencee",
        ],
        "",
        true,
    );

    let error = outcome.result.expect_err("there is no confidencee");
    assert_eq!(error.exit_code(), 2);
    assert!(error.to_string().contains("--field"), "{error}");
}

#[test]
fn pretty_indents_the_response() {
    let server = FakeServer::start(vec![Reply::json(ANSWERS)]);
    let (_key, key) = key_file();
    let (_dir, doc) = quickstart();

    let outcome = run(
        &[
            "decide",
            "ask",
            &doc,
            "--base-url",
            &server.url(),
            "--api-key-file",
            &key,
            "--pretty",
        ],
        "",
        true,
    );

    assert!(
        outcome.stdout.contains("\n  \"answers\""),
        "{}",
        outcome.stdout
    );
}

#[test]
fn models_is_printed_whole_and_can_be_selected_from() {
    let server = FakeServer::start(vec![Reply::json(MODELS), Reply::json(MODELS)]);
    let (_key, key) = key_file();
    let url = server.url();
    let base = [
        "decide",
        "models",
        "--base-url",
        url.as_str(),
        "--api-key-file",
        key.as_str(),
    ];

    let whole = run(&base, "", true);
    assert!(whole.result.is_ok(), "{:?}", whole.result);
    let printed: Value = serde_json::from_str(whole.stdout.trim_end()).expect("stdout is JSON");
    assert_eq!(
        printed,
        serde_json::from_str::<Value>(MODELS).expect("the fixture")
    );

    let mut args = base.to_vec();
    args.push("--select");
    args.push("models.0.name");
    let selected = run(&args, "", true);
    assert_eq!(selected.stdout, "jev-latest\n");
}

#[test]
fn the_credential_is_never_written_to_stdout() {
    let server = FakeServer::start(vec![Reply::json(ANSWERS)]);
    let (_key, key) = key_file();
    let (_dir, doc) = quickstart();

    let outcome = run(
        &[
            "decide",
            "ask",
            &doc,
            "--base-url",
            &server.url(),
            "--api-key-file",
            &key,
            "--verbose",
        ],
        "",
        true,
    );

    assert!(!outcome.stdout.contains("sekrit-token"));
    assert!(
        !outcome.stderr.contains("sekrit-token"),
        "--verbose is not a credential leak"
    );
}

#[test]
fn a_dry_run_works_with_nothing_but_the_invocation() {
    let outcome = run(
        &["decide", "noul", "is it urgent?", "--no-state", "--dry-run"],
        "",
        true,
    );

    assert!(outcome.result.is_ok());
    assert_eq!(
        serde_json::from_str::<Value>(outcome.stdout.trim_end())
            .expect("the dry run printed JSON")
            .pointer("/questions/answer/type")
            .and_then(Value::as_str),
        Some("noul")
    );
}

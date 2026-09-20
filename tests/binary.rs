//! The compiled binary, as a child process.
//!
//! A handful of tests that pin what the library tests cannot see: the exit-code table, the
//! fact that `--help` and `--version` go to stdout and exit `0`, that a bare invocation
//! prints the help on stderr and exits `2`, and that stdout survives `--verbose` byte for
//! byte.
//!
//! Every child runs with a cleared environment and `stdin` on `/dev/null`, so an ambient
//! `TYPESAFE_*` variable in a developer's shell cannot point a test at the real API, and
//! nothing waits for input a test cannot type.

// An integration test is its own crate, which `clippy.toml`'s `allow-expect-in-tests`
// cannot see: as in `src`'s unit tests, a panic here is the assertion mechanism.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::io::Write as _;
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use support::{FakeServer, Reply};

/// The vendor's three-answer response, copied from the documentation.
const ANSWERS: &str = include_str!("data/response_answers.json");

/// Run the built binary with a cleared environment and no stdin.
fn decide(args: &[&str]) -> Output {
    decide_with(args, &[])
}

/// Run the built binary with a cleared environment plus `env`, and no stdin.
fn decide_with(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_decide"));
    command
        .args(args)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in env {
        command.env(name, value);
    }
    command.output().expect("the binary runs")
}

/// The exit code of a finished child.
fn code(output: &Output) -> i32 {
    output
        .status
        .code()
        .expect("the child was not killed by a signal")
}

/// One stream of a finished child, as text.
fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The vendor's quickstart document, in a file that outlives the test.
fn quickstart() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("questions.json");
    std::fs::write(&path, include_str!("data/request_quickstart.json")).expect("write");
    let path = path.to_string_lossy().into_owned();
    (dir, path)
}

/// The response body as the client re-serialises it, which is what stdout must carry.
fn canonical_answers() -> String {
    let value: Value = serde_json::from_str(ANSWERS).expect("the fixture is JSON");
    serde_json::to_string(&value).expect("a value serialises")
}

#[test]
fn a_completed_evaluation_exits_zero_and_prints_the_response() {
    let server = FakeServer::start(vec![Reply::json(ANSWERS)]);
    let (_dir, doc) = quickstart();

    let output = decide_with(
        &["ask", &doc, "--base-url", &server.url()],
        &[("TYPESAFE_API_KEY", "from-the-environment")],
    );

    assert_eq!(code(&output), 0, "stderr: {}", text(&output.stderr));
    assert_eq!(text(&output.stdout), format!("{}\n", canonical_answers()));
    assert!(output.stderr.is_empty(), "no progress without --verbose");
    let seen = server.seen();
    let request = seen.first().expect("one request was seen");
    assert_eq!(
        request.header("authorization"),
        Some("Bearer from-the-environment"),
        "the environment credential is what travelled"
    );
}

#[test]
fn a_missing_credential_is_exit_two_and_is_reported_before_a_connection() {
    let server = FakeServer::start(vec![Reply::json(ANSWERS)]);
    let (_dir, doc) = quickstart();

    let output = decide(&["ask", &doc, "--base-url", &server.url()]);

    assert_eq!(code(&output), 2);
    assert!(output.stdout.is_empty(), "a failed run leaves stdout empty");
    let stderr = text(&output.stderr);
    assert!(stderr.starts_with("decide: "), "{stderr}");
    assert!(stderr.contains("TYPESAFE_API_KEY"), "{stderr}");
    assert!(stderr.contains("--api-key-file"), "{stderr}");
    assert_eq!(
        server.count(),
        0,
        "no connection is opened before the check"
    );
}

#[test]
fn a_credential_in_a_file_is_used_when_the_environment_has_none() {
    let server = FakeServer::start(vec![Reply::json(ANSWERS)]);
    let (_dir, doc) = quickstart();
    let mut key = tempfile::NamedTempFile::new().expect("a temporary file");
    key.write_all(b"from-the-file\n").expect("write the key");
    let key = key.path().to_string_lossy().into_owned();

    let output = decide(&[
        "ask",
        &doc,
        "--base-url",
        &server.url(),
        "--api-key-file",
        &key,
    ]);

    assert_eq!(code(&output), 0, "stderr: {}", text(&output.stderr));
    let seen = server.seen();
    let request = seen.first().expect("one request was seen");
    assert_eq!(
        request.header("authorization"),
        Some("Bearer from-the-file"),
        "the trailing newline was stripped"
    );
}

/// Run the built binary with a home directory of our choosing, and text on stdin.
fn decide_in_home(args: &[&str], home: &std::path::Path, stdin_text: &str) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_decide"));
    command
        .args(args)
        // A cleared environment and a home of our own, so the store the child reads and
        // writes is one the test owns.
        .env_clear()
        .env("HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("the binary runs");
    {
        let mut stdin = child.stdin.take().expect("stdin is piped");
        stdin.write_all(stdin_text.as_bytes()).expect("write");
    }
    child.wait_with_output().expect("the child finishes")
}

#[test]
fn a_stored_token_is_used_when_the_environment_has_none() {
    let server = FakeServer::start(vec![Reply::json(ANSWERS)]);
    let (_dir, doc) = quickstart();
    let home = tempfile::tempdir().expect("a temporary home");

    let stored = decide_in_home(&["auth", "set"], home.path(), "from-the-store\n");

    assert_eq!(code(&stored), 0, "stderr: {}", text(&stored.stderr));
    assert!(stored.stdout.is_empty(), "a confirmation is a diagnostic");
    assert!(
        text(&stored.stderr).contains("stored the token"),
        "{}",
        text(&stored.stderr)
    );

    let called = decide_in_home(&["ask", &doc, "--base-url", &server.url()], home.path(), "");

    assert_eq!(code(&called), 0, "stderr: {}", text(&called.stderr));
    let seen = server.seen();
    let request = seen.first().expect("one request was seen");
    assert_eq!(
        request.header("authorization"),
        Some("Bearer from-the-store"),
        "the credential came from the store, with no environment variable set"
    );
}

#[test]
fn auth_status_check_exercises_the_stored_key() {
    let server = FakeServer::start(vec![Reply::json(include_str!("data/models.json"))]);
    let home = tempfile::tempdir().expect("a temporary home");
    assert_eq!(
        code(&decide_in_home(
            &["auth", "set"],
            home.path(),
            "stored-token\n"
        )),
        0
    );

    let checked = decide_in_home(
        &["auth", "status", "--check", "--base-url", &server.url()],
        home.path(),
        "",
    );

    assert_eq!(code(&checked), 0, "stderr: {}", text(&checked.stderr));
    assert!(
        text(&checked.stdout).contains("the API accepted it"),
        "{}",
        text(&checked.stdout)
    );
    let seen = server.seen();
    let request = seen.first().expect("one request was seen");
    assert_eq!(
        request.header("authorization"),
        Some("Bearer stored-token"),
        "the check used the stored credential, with no environment variable set"
    );
}

#[test]
fn auth_status_reports_where_the_credential_comes_from_without_printing_it() {
    let home = tempfile::tempdir().expect("a temporary home");

    let empty = decide_in_home(&["auth", "status"], home.path(), "");
    assert_eq!(
        code(&empty),
        0,
        "nothing configured is a fact, not a failure"
    );
    assert!(
        text(&empty.stdout).contains("no credential"),
        "{}",
        text(&empty.stdout)
    );

    let stored = decide_in_home(&["auth", "set"], home.path(), "sekrit-token\n");
    assert_eq!(code(&stored), 0);

    let status = decide_in_home(&["auth", "status"], home.path(), "");
    assert_eq!(code(&status), 0);
    let stdout = text(&status.stdout);
    assert!(stdout.contains("api-key"), "{stdout}");
    assert!(
        !stdout.contains("sekrit-token"),
        "the value is never printed"
    );
}

#[test]
fn auth_unset_forgets_the_stored_token() {
    let home = tempfile::tempdir().expect("a temporary home");

    assert_eq!(
        code(&decide_in_home(
            &["auth", "set"],
            home.path(),
            "sekrit-token\n"
        )),
        0
    );
    assert_eq!(
        code(&decide_in_home(&["auth", "unset"], home.path(), "")),
        0
    );

    let status = decide_in_home(&["auth", "status"], home.path(), "");
    assert!(
        text(&status.stdout).contains("no credential"),
        "{}",
        text(&status.stdout)
    );
}

#[test]
fn auth_set_refuses_an_empty_stdin() {
    let home = tempfile::tempdir().expect("a temporary home");

    let output = decide_in_home(&["auth", "set"], home.path(), "");

    assert_eq!(code(&output), 2);
    assert!(output.stdout.is_empty());
    assert!(
        text(&output.stderr).contains("nothing was piped in"),
        "an empty pipe says which way in was empty: {}",
        text(&output.stderr)
    );
}

#[test]
fn the_environment_still_overrides_the_stored_token() {
    let server = FakeServer::start(vec![Reply::json(ANSWERS)]);
    let (_dir, doc) = quickstart();
    let home = tempfile::tempdir().expect("a temporary home");
    assert_eq!(
        code(&decide_in_home(
            &["auth", "set"],
            home.path(),
            "from-the-store\n"
        )),
        0
    );

    let mut command = Command::new(env!("CARGO_BIN_EXE_decide"));
    command
        .args(["ask", &doc, "--base-url", &server.url()])
        .env_clear()
        .env("HOME", home.path())
        .env("TYPESAFE_API_KEY", "from-the-environment")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let called = command.output().expect("the binary runs");

    assert_eq!(code(&called), 0, "stderr: {}", text(&called.stderr));
    let seen = server.seen();
    let request = seen.first().expect("one request was seen");
    assert_eq!(
        request.header("authorization"),
        Some("Bearer from-the-environment"),
        "a variable is the caller saying what to use this time"
    );
}

#[test]
fn a_failed_evaluation_is_exit_one_with_empty_stdout() {
    let server = FakeServer::start(vec![
        Reply::status(500, "later"),
        Reply::status(500, "later"),
        Reply::status(500, "later"),
    ]);
    let (_dir, doc) = quickstart();

    let output = decide_with(
        &[
            "ask",
            &doc,
            "--base-url",
            &server.url(),
            "--backoff-ms",
            "1",
        ],
        &[("TYPESAFE_API_KEY", "k")],
    );

    assert_eq!(code(&output), 1);
    assert!(output.stdout.is_empty(), "a failed run leaves stdout empty");
    assert!(
        text(&output.stderr).contains("500"),
        "{}",
        text(&output.stderr)
    );
}

#[test]
fn a_usage_error_is_exit_two_with_empty_stdout() {
    let (_dir, doc) = quickstart();

    // The quickstart document asks three questions, so "the value" is not defined.
    let output = decide(&["ask", &doc, "--value"]);

    assert_eq!(code(&output), 2);
    assert!(output.stdout.is_empty());
    let stderr = text(&output.stderr);
    assert!(stderr.starts_with("decide: "), "{stderr}");
    assert!(stderr.contains("exactly one question"), "{stderr}");
}

#[test]
fn a_dry_run_exits_zero_and_needs_no_credential() {
    let output = decide(&[
        "choice",
        "which team?",
        "--no-state",
        "--option",
        "billing",
        "--dry-run",
    ]);

    assert_eq!(code(&output), 0, "stderr: {}", text(&output.stderr));
    assert!(output.stderr.is_empty());
    let printed: Value = serde_json::from_slice(&output.stdout).expect("the dry run printed JSON");
    assert_eq!(
        printed.get("model").and_then(Value::as_str),
        Some("jev-latest")
    );
}

#[test]
fn help_goes_to_stdout_and_exits_zero() {
    let output = decide(&["--help"]);

    assert_eq!(code(&output), 0);
    assert!(output.stderr.is_empty(), "help is not a diagnostic");
    let stdout = text(&output.stdout);
    assert!(
        stdout.starts_with("Ask Jev for a typed decision"),
        "{stdout}"
    );
    assert!(
        stdout.contains("Usage: decide [OPTIONS] <COMMAND>"),
        "{stdout}"
    );
}

#[test]
fn version_goes_to_stdout_and_exits_zero() {
    let output = decide(&["--version"]);

    assert_eq!(code(&output), 0);
    assert!(
        text(&output.stdout).starts_with("decide "),
        "{}",
        text(&output.stdout)
    );
}

#[test]
fn a_bare_invocation_prints_the_help_on_stderr_and_exits_two() {
    let output = decide(&[]);

    assert_eq!(code(&output), 2);
    assert!(output.stdout.is_empty(), "the help goes to stderr here");
    let stderr = text(&output.stderr);
    assert!(stderr.contains("Usage: decide"), "{stderr}");
    assert!(stderr.contains("ask"), "{stderr}");
}

#[test]
fn an_unrecognised_argument_is_exit_two() {
    let output = decide(&["--nonsense"]);

    assert_eq!(code(&output), 2);
    assert!(output.stdout.is_empty());
    assert!(
        text(&output.stderr).contains("--nonsense"),
        "{}",
        text(&output.stderr)
    );
}

#[test]
fn the_api_key_flag_does_not_exist() {
    let output = decide(&["models", "--api-key", "sekrit"]);

    assert_eq!(code(&output), 2);
    assert!(output.stdout.is_empty());
    assert!(
        text(&output.stderr).contains("--api-key"),
        "{}",
        text(&output.stderr)
    );
}

#[test]
fn a_flag_that_conflicts_is_exit_two() {
    let output = decide(&[
        "noul",
        "is it so?",
        "--no-state",
        "--value",
        "--select",
        "model",
    ]);

    assert_eq!(code(&output), 2);
    assert!(output.stdout.is_empty());
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("--value") && stderr.contains("--select"),
        "{stderr}"
    );
}

#[test]
fn verbose_does_not_change_a_byte_of_stdout() {
    let server = FakeServer::start(vec![Reply::json(ANSWERS), Reply::json(ANSWERS)]);
    let (_dir, doc) = quickstart();

    let quiet = decide_with(
        &["ask", &doc, "--base-url", &server.url()],
        &[("TYPESAFE_API_KEY", "k")],
    );
    let loud = decide_with(
        &["ask", &doc, "--base-url", &server.url(), "--verbose"],
        &[("TYPESAFE_API_KEY", "k")],
    );

    assert_eq!(quiet.stdout, loud.stdout, "--verbose never touches stdout");
    assert!(quiet.stderr.is_empty());
    assert!(!loud.stderr.is_empty(), "--verbose reports on stderr");
}

#[test]
fn a_select_prints_one_value_and_exits_zero() {
    let server = FakeServer::start(vec![Reply::json(ANSWERS)]);
    let (_dir, doc) = quickstart();

    let output = decide_with(
        &[
            "ask",
            &doc,
            "--base-url",
            &server.url(),
            "--select",
            "answers.department.choice",
        ],
        &[("TYPESAFE_API_KEY", "k")],
    );

    assert_eq!(code(&output), 0);
    assert_eq!(
        text(&output.stdout),
        "billing\n",
        "raw, with no JSON quoting"
    );
}

#[test]
fn models_lists_the_models_and_exits_zero() {
    let server = FakeServer::start(vec![Reply::json(include_str!("data/models.json"))]);

    let output = decide_with(
        &[
            "models",
            "--base-url",
            &server.url(),
            "--select",
            "models.0.name",
        ],
        &[("TYPESAFE_API_KEY", "k")],
    );

    assert_eq!(code(&output), 0, "stderr: {}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "jev-latest\n");
    let seen = server.seen();
    let request = seen.first().expect("one request was seen");
    assert_eq!(request.method, "GET");
    assert_eq!(request.path, "/v1/models");
}

#[test]
fn a_retried_rate_limit_still_exits_zero() {
    let server = FakeServer::start(vec![
        Reply::status(429, "slow down").with_header("Retry-After", "0"),
        Reply::json(ANSWERS),
    ]);
    let (_dir, doc) = quickstart();

    let output = decide_with(
        &[
            "ask",
            &doc,
            "--base-url",
            &server.url(),
            "--backoff-ms",
            "1",
        ],
        &[("TYPESAFE_API_KEY", "k")],
    );

    assert_eq!(
        code(&output),
        0,
        "a 429 is an expected event, not a failed run"
    );
    assert_eq!(server.count(), 2);
}

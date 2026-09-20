//! The transport, against a local socket.
//!
//! Over a real TCP connection with the real `ureq`, because the parts most likely to be
//! wrong are the ones a mock HTTP layer hides: the headers actually written, what the
//! retry loop actually does, and whether a trailing slash in the base URL produces
//! `//v1/systemone`.
//!
//! Every test asserts on what the *server* received as well as on what the client
//! returned, and every test passes a backoff of one millisecond so that observing a retry
//! costs a millisecond rather than a second.

// An integration test is its own crate, which `clippy.toml`'s `allow-expect-in-tests`
// cannot see: as in `src`'s unit tests, a panic here is the assertion mechanism.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::collections::BTreeMap;
use std::net::TcpListener;

use serde_json::Value;

use decide::client::{self, ClientConfig};
use decide::error::DecideError;
use decide::wire::{Question, Request};
use decide::{input, report, wire};
use support::{FakeServer, Reply};

/// The response body a successful call returns.
const ANSWER: &str = concat!(
    r#"{"model":"jev-1.13.0","answers":{"is_urgent":{"type":"noul","noul":0.95}},"#,
    r#""usage":{"input_tokens":296,"output_tokens":20}}"#,
);

/// A config pointed at the fake server, with the backoff turned down to nothing.
fn config(server: &FakeServer, retries: u32) -> ClientConfig {
    ClientConfig {
        base_url: server.url(),
        api_key: "sekrit-token".to_string(),
        timeout_secs: 5,
        retries,
        backoff_ms: 1,
        verbose: false,
    }
}

/// One valid request, ready to send.
fn request() -> Request {
    let mut questions = BTreeMap::new();
    questions.insert(
        "is_urgent".to_string(),
        Question::noul(Value::String("Does this convey urgency?".to_string()), None),
    );
    Request::new(
        Value::String("Help!".to_string()),
        "jev-latest".to_string(),
        questions,
    )
    .expect("the request is valid")
}

/// The body of that request, as it goes on the wire.
fn body() -> String {
    report::render_json(&request().to_value(), false)
}

#[test]
fn the_api_key_travels_in_the_authorization_header() {
    let server = FakeServer::start(vec![Reply::json(ANSWER)]);
    let mut progress = Vec::new();

    let returned = client::post_systemone(&config(&server, 2), &body(), &mut progress)
        .expect("the call succeeds");

    assert_eq!(returned, ANSWER);
    let seen = server.seen();
    let request = seen.first().expect("one request was seen");
    assert_eq!(
        request.header("authorization"),
        Some("Bearer sekrit-token"),
        "the credential is a bearer token, and it never came from argv"
    );
}

#[test]
fn the_request_line_and_content_type_are_what_the_api_documents() {
    let server = FakeServer::start(vec![Reply::json(ANSWER)]);
    let mut progress = Vec::new();

    client::post_systemone(&config(&server, 0), &body(), &mut progress).expect("the call");

    let seen = server.seen();
    let request = seen.first().expect("one request was seen");
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/v1/systemone");
    assert_eq!(request.header("content-type"), Some("application/json"));
}

#[test]
fn the_body_the_server_received_is_the_request_that_was_built() {
    let server = FakeServer::start(vec![Reply::json(ANSWER)]);
    let mut progress = Vec::new();

    client::post_systemone(&config(&server, 0), &body(), &mut progress).expect("the call");

    let seen = server.seen();
    let request = seen.first().expect("one request was seen");
    // Compared as parsed values, because what matters is the document, not the spacing.
    let sent: Value = serde_json::from_str(&request.body).expect("the body is JSON");
    assert_eq!(
        sent.get("model").and_then(Value::as_str),
        Some("jev-latest")
    );
    assert_eq!(sent.get("state").and_then(Value::as_str), Some("Help!"));
    assert_eq!(
        sent.pointer("/questions/is_urgent/instructions")
            .and_then(Value::as_str),
        Some("Does this convey urgency?")
    );
}

#[test]
fn a_trailing_slash_in_the_base_url_does_not_double_the_path() {
    let server = FakeServer::start(vec![Reply::json(ANSWER)]);
    let mut progress = Vec::new();
    let mut config = config(&server, 0);
    config.base_url = input::resolve_base_url(&format!("{}/", server.url()))
        .expect("a base URL with a scheme is accepted");

    client::post_systemone(&config, &body(), &mut progress).expect("the call");

    let seen = server.seen();
    let request = seen.first().expect("one request was seen");
    assert_eq!(
        request.path, "/v1/systemone",
        "a trailing slash in the base URL must not become //v1/systemone"
    );
}

#[test]
fn a_base_url_naming_the_endpoint_in_full_calls_the_same_two_paths() {
    let server = FakeServer::start(vec![Reply::json(ANSWER), Reply::json("{}")]);
    let mut progress = Vec::new();
    let mut config = config(&server, 0);
    // What the default is, and what a caller who pasted the documented URL would pass.
    config.base_url = input::resolve_base_url(&format!("{}/v1/systemone", server.url()))
        .expect("a base URL with a scheme is accepted");

    client::post_systemone(&config, &body(), &mut progress).expect("the call");
    client::get_models(&config, &mut progress).expect("the call");

    let seen = server.seen();
    let posted = seen.first().expect("the POST was seen");
    let listed = seen.get(1).expect("the GET was seen");
    assert_eq!(posted.path, "/v1/systemone", "no doubled path segment");
    assert_eq!(
        listed.path, "/v1/models",
        "models stays a sibling of the endpoint, not a child of it"
    );
}

#[test]
fn a_rate_limit_is_retried_after_the_retry_after_header() {
    let server = FakeServer::start(vec![
        Reply::status(429, "slow down").with_header("Retry-After", "0"),
        Reply::json(ANSWER),
    ]);
    let mut progress = Vec::new();

    let returned = client::post_systemone(&config(&server, 2), &body(), &mut progress)
        .expect("the retry wins");

    assert_eq!(returned, ANSWER);
    assert_eq!(server.count(), 2, "the 429 was retried exactly once");
}

#[test]
fn a_request_timeout_status_is_retried() {
    let server = FakeServer::start(vec![Reply::status(408, ""), Reply::json(ANSWER)]);
    let mut progress = Vec::new();

    client::post_systemone(&config(&server, 2), &body(), &mut progress).expect("the retry wins");

    assert_eq!(server.count(), 2);
}

#[test]
fn an_overloaded_is_retried() {
    let server = FakeServer::start(vec![Reply::status(529, ""), Reply::json(ANSWER)]);
    let mut progress = Vec::new();

    client::post_systemone(&config(&server, 2), &body(), &mut progress).expect("the retry wins");

    assert_eq!(server.count(), 2);
}

#[test]
fn a_server_error_is_retried() {
    let server = FakeServer::start(vec![
        Reply::status(503, "later"),
        Reply::status(500, "still not"),
        Reply::json(ANSWER),
    ]);
    let mut progress = Vec::new();

    client::post_systemone(&config(&server, 2), &body(), &mut progress).expect("the retry wins");

    assert_eq!(server.count(), 3);
}

#[test]
fn a_validation_error_is_not_retried() {
    let server = FakeServer::start(vec![
        Reply::status(422, r#"{"detail":"questions must not be empty"}"#),
        Reply::json(ANSWER),
    ]);
    let mut progress = Vec::new();

    let error = client::post_systemone(&config(&server, 2), &body(), &mut progress)
        .expect_err("a 422 is the answer");

    assert_eq!(server.count(), 1, "a validation error must not be retried");
    assert_eq!(error.exit_code(), 1);
    let rendered = error.to_string();
    assert!(rendered.contains("422"), "{rendered}");
    assert!(
        rendered.contains("questions must not be empty"),
        "{rendered}"
    );
}

#[test]
fn a_bad_request_is_not_retried() {
    let server = FakeServer::start(vec![Reply::status(400, "no"), Reply::json(ANSWER)]);
    let mut progress = Vec::new();

    client::post_systemone(&config(&server, 2), &body(), &mut progress)
        .expect_err("a 400 is fatal");

    assert_eq!(server.count(), 1);
}

#[test]
fn an_unauthorized_is_not_retried() {
    let server = FakeServer::start(vec![Reply::status(401, "bad key"), Reply::json(ANSWER)]);
    let mut progress = Vec::new();

    let error = client::post_systemone(&config(&server, 2), &body(), &mut progress)
        .expect_err("a 401 is fatal");

    assert_eq!(server.count(), 1);
    assert!(error.to_string().contains("401"), "{error}");
}

#[test]
fn no_retries_means_exactly_one_attempt() {
    let server = FakeServer::start(vec![Reply::status(500, ""), Reply::json(ANSWER)]);
    let mut progress = Vec::new();

    client::post_systemone(&config(&server, 0), &body(), &mut progress)
        .expect_err("one attempt is all there is");

    assert_eq!(server.count(), 1);
}

#[test]
fn a_rate_limit_that_never_clears_reports_the_status() {
    let server = FakeServer::start(vec![
        Reply::status(429, "slow down"),
        Reply::status(429, "slow down"),
        Reply::status(429, "slow down"),
        Reply::json(ANSWER),
    ]);
    let mut progress = Vec::new();

    let error = client::post_systemone(&config(&server, 2), &body(), &mut progress)
        .expect_err("the retries run out");

    assert_eq!(server.count(), 3, "one attempt and two retries");
    assert_eq!(error.exit_code(), 1);
    assert!(error.to_string().contains("429"), "{error}");
}

#[test]
fn a_connection_failure_reports_the_attempt_count() {
    // A port that was bound and then released, so nothing is listening on it.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a port");
    let base_url = format!("http://{}", listener.local_addr().expect("the address"));
    drop(listener);

    let mut progress = Vec::new();
    let config = ClientConfig {
        base_url,
        api_key: "sekrit-token".to_string(),
        timeout_secs: 5,
        retries: 2,
        backoff_ms: 1,
        verbose: false,
    };

    let error =
        client::post_systemone(&config, &body(), &mut progress).expect_err("nothing is listening");

    assert_eq!(error.exit_code(), 1);
    let rendered = error.to_string();
    assert!(rendered.contains("3 attempts"), "{rendered}");
}

#[test]
fn a_server_that_never_answers_times_out() {
    let server = FakeServer::start_silent();
    let mut progress = Vec::new();
    let config = ClientConfig {
        base_url: server.url(),
        api_key: "sekrit-token".to_string(),
        timeout_secs: 1,
        retries: 0,
        backoff_ms: 1,
        verbose: false,
    };

    let error = client::post_systemone(&config, &body(), &mut progress)
        .expect_err("the server never answers");

    assert_eq!(error.exit_code(), 1);
    let rendered = error.to_string();
    assert!(rendered.contains("timed out"), "{rendered}");
    assert!(rendered.contains("/v1/systemone"), "{rendered}");
}

#[test]
fn a_long_error_body_is_capped() {
    let huge = "x".repeat(20_000);
    let server = FakeServer::start(vec![Reply::status(400, &huge)]);
    let mut progress = Vec::new();

    let error = client::post_systemone(&config(&server, 0), &body(), &mut progress)
        .expect_err("a 400 is fatal");

    let DecideError::Status { body, .. } = error else {
        panic!("a status error was expected");
    };
    assert!(
        body.len() < 20_000,
        "the error body is capped, and this one was {} bytes",
        body.len()
    );
}

#[test]
fn verbose_writes_progress_to_the_writer_and_not_to_the_body() {
    // Two identical replies, because the test makes the same call twice.
    let server = FakeServer::start(vec![Reply::json(ANSWER), Reply::json(ANSWER)]);
    let mut quiet_progress = Vec::new();
    let mut loud_progress = Vec::new();

    let quiet = client::post_systemone(&config(&server, 0), &body(), &mut quiet_progress)
        .expect("the call");
    let mut loud_config = config(&server, 0);
    loud_config.verbose = true;
    let loud = client::post_systemone(&loud_config, &body(), &mut loud_progress).expect("the call");

    assert_eq!(quiet, loud, "--verbose does not change the response");
    let progress = String::from_utf8(loud_progress).expect("progress is text");
    assert!(progress.contains("POST"), "{progress}");
    assert!(progress.contains("/v1/systemone"), "{progress}");
    assert!(progress.contains("200"), "{progress}");
    assert!(progress.contains("jev-1.13.0"), "{progress}");
    assert!(progress.contains("296 in / 20 out tokens"), "{progress}");
    assert!(
        quiet_progress.is_empty(),
        "nothing is written without --verbose"
    );
}

#[test]
fn models_is_a_get_with_the_authorization_header() {
    let models = include_str!("data/models.json");
    let server = FakeServer::start(vec![Reply::json(models)]);
    let mut progress = Vec::new();

    let returned = client::get_models(&config(&server, 0), &mut progress).expect("the call");

    assert_eq!(returned, models);
    let seen = server.seen();
    let request = seen.first().expect("one request was seen");
    assert_eq!(request.method, "GET");
    assert_eq!(request.path, "/v1/models");
    assert_eq!(request.header("authorization"), Some("Bearer sekrit-token"));
    assert_eq!(request.body, "", "a GET has no body");
}

#[test]
fn a_status_error_names_the_reason_phrase() {
    let server = FakeServer::start(vec![Reply::status(422, "unprocessable")]);
    let mut progress = Vec::new();

    let error = client::post_systemone(&config(&server, 0), &body(), &mut progress)
        .expect_err("a 422 is fatal");

    assert!(
        error.to_string().contains("Unprocessable Entity"),
        "{error} should name the reason phrase the server sent"
    );
}

#[test]
fn the_body_the_client_returned_is_the_one_wire_can_read() {
    let server = FakeServer::start(vec![Reply::json(ANSWER)]);
    let mut progress = Vec::new();

    let text =
        client::post_systemone(&config(&server, 0), &body(), &mut progress).expect("the call");
    let response = wire::Response::from_slice(text.as_bytes(), &request())
        .expect("the client's bytes are a response");

    assert_eq!(response.model.as_deref(), Some("jev-1.13.0"));
}

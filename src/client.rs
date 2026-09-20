//! The transport: one blocking POST or GET, the auth header, the retry loop of
//! [D10](../docs/design.md#d10-retries-on-by-default-with-the-sdks-policy), and the
//! error-body cap.
//!
//! Blocking rather than async
//! ([D1](../docs/design.md#d1-a-blocking-http-client-and-no-async-runtime)), because the
//! API does not stream: one request produces one response body, so the whole transport is
//! one call and a hand-written retry loop.
//!
//! Nothing here prints. The caller passes the writer that progress goes to, which is how
//! `--verbose` is asserted without capturing a subprocess.

use std::io::{Read as _, Write};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use ureq::{Agent, Body, http::StatusCode};

use crate::error::DecideError;

/// The delay before the first retry doubles up to this, in milliseconds.
const MAX_BACKOFF_MS: u64 = 5000;

/// The share of each delay that jitter may subtract, as a percent.
const JITTER_PERCENT: u64 = 25;

/// The most a `Retry-After` header is believed, in milliseconds.
const MAX_RETRY_AFTER_MS: u64 = 60_000;

/// The most of an error body is quoted back to the caller, in bytes.
const MAX_ERROR_BODY: u64 = 4096;

/// Everything a call needs beyond the request body: where, with which credential, and how
/// hard to try.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// The API root, already checked for a scheme and normalised.
    pub base_url: String,
    /// The bearer credential.
    pub api_key: String,
    /// The bound on one attempt, in seconds.
    pub timeout_secs: u64,
    /// The retries after the first attempt.
    pub retries: u32,
    /// The delay before the first retry, in milliseconds.
    pub backoff_ms: u64,
    /// Whether to report progress on the writer passed with each call.
    pub verbose: bool,
}

/// POST one request body to `/v1/systemone` and return the response body.
pub fn post_systemone<W: Write>(
    config: &ClientConfig,
    body: &str,
    progress: &mut W,
) -> Result<String, DecideError> {
    let url = format!("{}/v1/systemone", config.base_url);
    call(config, Method::Post, &url, Some(body), progress)
}

/// GET `/v1/models` and return the response body.
pub fn get_models<W: Write>(
    config: &ClientConfig,
    progress: &mut W,
) -> Result<String, DecideError> {
    let url = format!("{}/v1/models", config.base_url);
    call(config, Method::Get, &url, None, progress)
}

/// The two calls this program makes.
#[derive(Debug, Clone, Copy)]
enum Method {
    /// `POST /v1/systemone`.
    Post,
    /// `GET /v1/models`.
    Get,
}

impl Method {
    /// The method as it goes on the request line.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Post => "POST",
            Self::Get => "GET",
        }
    }
}

/// One attempt loop: try, and retry what the policy says to retry, bounded by `--retries`.
fn call<W: Write>(
    config: &ClientConfig,
    method: Method,
    url: &str,
    body: Option<&str>,
    progress: &mut W,
) -> Result<String, DecideError> {
    let agent: Agent = Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(config.timeout_secs)))
        // The status is read here rather than turned into a `ureq` error, because the
        // error body and the `Retry-After` header both matter to the decision.
        .http_status_as_error(false)
        .build()
        .into();

    let allowed = config.retries.saturating_add(1);
    if config.verbose {
        let _ = writeln!(progress, "{} {url}", method.as_str());
    }

    let mut attempt: u32 = 1;
    loop {
        match attempt_once(&agent, method, url, body, config) {
            Ok((status, text)) => {
                if config.verbose {
                    let _ = writeln!(progress, "{}", outcome(status, &text));
                }
                return Ok(text);
            }
            Err(CallError::Fatal(error)) => return Err(error),
            Err(CallError::Retryable(transient)) => {
                if attempt >= allowed {
                    return Err(transient.into_error(url, config.timeout_secs, attempt));
                }
                let delay = transient.delay_ms(config.backoff_ms, attempt);
                if config.verbose {
                    let _ = writeln!(
                        progress,
                        "{}; retrying in {delay} ms (attempt {} of {allowed})",
                        transient.describe(),
                        attempt.saturating_add(1)
                    );
                }
                std::thread::sleep(Duration::from_millis(delay));
                attempt = attempt.saturating_add(1);
            }
        }
    }
}

/// Make one attempt, and classify what came back.
fn attempt_once(
    agent: &Agent,
    method: Method,
    url: &str,
    body: Option<&str>,
    config: &ClientConfig,
) -> Result<(u16, String), CallError> {
    let authorization = format!("Bearer {}", config.api_key);
    let response = match method {
        Method::Post => agent
            .post(url)
            .header("Content-Type", "application/json")
            .header("Authorization", authorization)
            .send(body.unwrap_or_default()),
        Method::Get => agent.get(url).header("Authorization", authorization).call(),
    };

    let mut response = match response {
        Ok(response) => response,
        Err(error) => return Err(transient(error)),
    };

    let status = response.status().as_u16();
    let reason = reason_phrase(status);

    if (200..300).contains(&status) {
        return match read_body(response.body_mut(), u64::MAX) {
            Ok((text, _)) => Ok((status, text)),
            Err(error) => Err(transient(error)),
        };
    }

    let retry_after_ms = parse_retry_after(response.headers());
    // An error body that cannot be read is not worth failing over: the status is the
    // message, and the body is a bonus.
    let (text, _) = read_body(response.body_mut(), MAX_ERROR_BODY).unwrap_or_default();

    if is_retryable_status(status) {
        Err(CallError::Retryable(Transient::Status {
            status,
            reason,
            body: text,
            retry_after_ms,
        }))
    } else {
        Err(CallError::Fatal(DecideError::Status {
            status,
            reason,
            body: non_empty(text),
        }))
    }
}

/// Why an attempt did not produce a body, and whether it is worth another attempt.
#[derive(Debug)]
enum CallError {
    /// The policy says to try again.
    Retryable(Transient),
    /// The policy says this is the answer.
    Fatal(DecideError),
}

/// A failure the retry policy may repeat.
#[derive(Debug)]
enum Transient {
    /// A status the policy retries: `408`, `429`, and `5xx`.
    Status {
        /// The status code.
        status: u16,
        /// The reason phrase, or a stand-in.
        reason: String,
        /// The error body, capped.
        body: String,
        /// A `Retry-After`, if the server sent one.
        retry_after_ms: Option<u64>,
    },
    /// The connection failed before a status arrived.
    Transport {
        /// The transport's own complaint.
        reason: String,
    },
    /// The attempt hit `--timeout`.
    Timeout,
}

impl Transient {
    /// The words for this failure, for a `--verbose` retry line.
    fn describe(&self) -> String {
        match self {
            Self::Status { status, reason, .. } => format!("{status} {reason}"),
            Self::Transport { reason } => format!("a transport error: {reason}"),
            Self::Timeout => "the attempt timed out".to_string(),
        }
    }

    /// How long to wait before the next attempt.
    fn delay_ms(&self, base_ms: u64, attempt: u32) -> u64 {
        match self {
            Self::Status {
                retry_after_ms: Some(delay),
                ..
            } => *delay,
            _ => backoff_ms(base_ms, attempt),
        }
    }

    /// The error this failure becomes once the retries are exhausted.
    fn into_error(self, url: &str, timeout_secs: u64, attempts: u32) -> DecideError {
        match self {
            Self::Status {
                status,
                reason,
                body,
                ..
            } => DecideError::Status {
                status,
                reason,
                body: non_empty(body),
            },
            Self::Transport { reason } => DecideError::Transport { reason, attempts },
            Self::Timeout => DecideError::Timeout {
                url: url.to_string(),
                seconds: timeout_secs,
            },
        }
    }
}

/// Whether a status is one the policy retries: `408`, `429`, and `5xx`.
const fn is_retryable_status(status: u16) -> bool {
    status == 408 || status == 429 || status >= 500
}

/// Classify a transport failure: every one of them is retried.
fn transient(error: ureq::Error) -> CallError {
    match error {
        ureq::Error::Timeout(_) => CallError::Retryable(Transient::Timeout),
        other => CallError::Retryable(Transient::Transport {
            reason: other.to_string(),
        }),
    }
}

/// Read a body, up to `cap` bytes, and say whether there was more.
fn read_body(body: &mut Body, cap: u64) -> Result<(String, bool), ureq::Error> {
    let reader = body.with_config().reader();
    let mut bytes = Vec::new();
    reader.take(cap.saturating_add(1)).read_to_end(&mut bytes)?;
    let too_long = cap != u64::MAX && u64::try_from(bytes.len()).unwrap_or(u64::MAX) > cap;
    if too_long {
        bytes.truncate(usize::try_from(cap).unwrap_or(usize::MAX));
    }
    // A body that is not UTF-8 is surfaced rather than refused: the status is the
    // message, and the bytes are evidence.
    Ok((String::from_utf8_lossy(&bytes).into_owned(), too_long))
}

/// The `Retry-After` or `retry-after-ms` header, in milliseconds, capped.
fn parse_retry_after(headers: &ureq::http::HeaderMap) -> Option<u64> {
    if let Some(millis) = header_millis(headers, "retry-after-ms") {
        return Some(millis.min(MAX_RETRY_AFTER_MS));
    }
    let seconds = headers
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())?;
    Some(seconds.saturating_mul(1000).min(MAX_RETRY_AFTER_MS))
}

/// One numeric header, in milliseconds.
fn header_millis(headers: &ureq::http::HeaderMap, name: &str) -> Option<u64> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
}

/// The delay before retrying after `attempt` attempts have failed: the base doubled
/// `attempt - 1` times, capped, with up to 25% subtracted as jitter.
fn backoff_ms(base_ms: u64, attempt: u32) -> u64 {
    let mut delay = base_ms;
    let mut step: u32 = 1;
    while step < attempt {
        delay = delay.saturating_mul(2).min(MAX_BACKOFF_MS);
        step = step.saturating_add(1);
    }
    let jitter = delay.saturating_mul(JITTER_PERCENT).saturating_div(100);
    let cut = if jitter == 0 {
        0
    } else {
        entropy().checked_rem(jitter.saturating_add(1)).unwrap_or(0)
    };
    delay.saturating_sub(cut)
}

/// An unpredictable number, drawn from the clock.
///
/// The jitter only has to be unpredictable, not unbiased, so a clock is enough — and it is
/// the reason `rand` is not a dependency.
fn entropy() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            elapsed.as_secs().wrapping_mul(1_000_000_007) ^ u64::from(elapsed.subsec_nanos())
        })
}

/// The reason phrase for a status, or a stand-in when the code is not a standard one.
fn reason_phrase(status: u16) -> String {
    StatusCode::from_u16(status)
        .ok()
        .and_then(|code| code.canonical_reason())
        .unwrap_or("Unknown")
        .to_string()
}

/// An empty body reads better as a statement than as nothing at all.
fn non_empty(body: String) -> String {
    if body.trim().is_empty() {
        "(empty body)".to_string()
    } else {
        body
    }
}

/// The `--verbose` outcome line: the status, and whatever the body named.
fn outcome(status: u16, body: &str) -> String {
    let reason = reason_phrase(status);
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return format!("{status} {reason}");
    };

    let mut extra = Vec::new();
    if let Some(model) = value.get("model").and_then(Value::as_str) {
        extra.push(format!("model {model}"));
    }
    let usage = value.get("usage");
    let input = usage
        .and_then(|usage| usage.get("input_tokens"))
        .and_then(Value::as_u64);
    let output = usage
        .and_then(|usage| usage.get("output_tokens"))
        .and_then(Value::as_u64);
    if let (Some(input), Some(output)) = (input, output) {
        extra.push(format!("{input} in / {output} out tokens"));
    }

    if extra.is_empty() {
        format!("{status} {reason}")
    } else {
        format!("{status} {reason} ({})", extra.join(", "))
    }
}

#[cfg(test)]
mod tests {
    // `Write`, for the temporary key file, comes in through `super`.
    use super::*;

    #[test]
    fn the_backoff_doubles_and_is_capped() {
        // The first delay is the knob, minus up to a quarter of jitter.
        let first = backoff_ms(500, 1);
        assert!((375..=500).contains(&first), "the first delay was {first}");

        // The fifth attempt would be 8000 ms undoubled, so it is capped at 5000.
        let capped = backoff_ms(500, 5);
        assert!(
            (3750..=5000).contains(&capped),
            "the capped delay was {capped}"
        );

        // A base above the cap is honoured as given: the knob is the knob.
        assert!(backoff_ms(9000, 1) <= 9000);

        assert_eq!(backoff_ms(0, 3), 0, "no delay stays no delay");
    }

    #[test]
    fn only_the_documented_statuses_are_retried() {
        for status in [408, 429, 500, 503, 529, 599] {
            assert!(is_retryable_status(status), "{status} is retried");
        }
        for status in [400, 401, 403, 404, 422, 200, 201] {
            assert!(!is_retryable_status(status), "{status} is not retried");
        }
    }

    #[test]
    fn an_error_body_that_is_empty_reads_as_a_statement() {
        assert_eq!(non_empty("  \n".to_string()), "(empty body)");
        assert_eq!(
            non_empty("{\"detail\":\"no\"}".to_string()),
            "{\"detail\":\"no\"}"
        );
    }

    #[test]
    fn the_verbose_outcome_line_names_the_model_and_the_tokens() {
        let body = r#"{"model":"jev-1.13.0","usage":{"input_tokens":296,"output_tokens":20}}"#;

        let line = outcome(200, body);

        assert!(line.starts_with("200 OK"), "{line}");
        assert!(line.contains("model jev-1.13.0"), "{line}");
        assert!(line.contains("296 in / 20 out tokens"), "{line}");
    }

    #[test]
    fn the_verbose_outcome_line_survives_a_body_that_is_not_json() {
        assert_eq!(outcome(200, "not json"), "200 OK");
    }
}

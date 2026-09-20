# Testing and verification

The claim this page makes is narrow and checkable: **every behaviour in
[`cli.md`](cli.md) is asserted by a test, in both directions, and the suite never touches
the network.** A test that only shows the happy path is treated as incomplete.

Nothing here is aspirational. Each section says what the test asserts and *why that
assertion is the product* rather than a proxy for it.

## The gates

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --all-features
cargo test --doc
cargo build --release
```

All five must be clean. `--all-features` is on every gate, and this crate has no optional
features, so the flag is a statement of intent: the build has one shape, and it is the one
that is tested.

`cargo nextest run --profile ci` adds the retry-and-fail-fast behaviour that CI wants
([`.config/nextest.toml`](design.md#toolchain-lints-and-the-test-runner)). The default
profile does **not** retry, because a test that passes on a retry is a test to look at.

The suite needs no `TYPESAFE_API_KEY`, no network, and no fixture directory that a fresh
clone lacks. That is a property worth keeping: a test suite that cannot run on a plane is
a test suite that stops running.

### The one test that needs the API

`tests/live.rs` carries a single `#[ignore]`d test — a real `noul` call against
`api.typesafe.ai` — and it runs only when asked:

```sh
TYPESAFE_API_KEY=… cargo nextest run --run-ignored ignored-only
```

It exists because a fake server proves the client is consistent with *our understanding*
of the API, not with the API. It is ignored rather than required because the rest of the
transcript's claims must be checkable by anyone with a clone.

## Layers

Four layers, each with a different reason to exist. The counts below are what the
implementation is expected to produce, not a target to pad towards.

### 1. Unit tests, no I/O (`wire.rs`, `input.rs`, `report.rs`, `error.rs`)

Fast, precise, and where most of the behaviour lives. The state resolution rules are
testable without a terminal because `input.rs` takes a `Stdin` that carries both a reader
*and* an `is_terminal` answer — the fact that a TTY and a pipe take different paths is
exactly the kind of thing that is otherwise only verifiable by hand.

### 2. Argument parsing (`cli.rs`), through `Cli::try_parse_from`

`try_parse_from` rather than the spawned binary, so a refusal is an assertion on an error
value instead of a subprocess. Every conflict and every arity rule is checked here.

`--help` and `--version` are *not* tested here: clap signals them through an error kind
that is not a failure, and asserting on that would be asserting on clap's internals. They
are checked through the built binary instead.

### 3. Transport and the wire (`client.rs`), against a local socket

Over a real TCP socket with the real `ureq`, because the parts most likely to be wrong are
the ones a mock HTTP layer hides: the headers actually written, the retry loop's actual
timing behaviour, what happens to an error body, and whether a trailing slash in a base URL
produces `//v1/systemone`.

### 4. The binary (`main.rs`), through `env!("CARGO_BIN_EXE_decide")`

A handful of tests that run the compiled program as a child process. They exist to pin the
things the library tests cannot see: the exit-code table, the fact that `--help` goes to
stdout and exits `0`, that stdout survives `--verbose` byte for byte, and that the stored
credential works end to end — a child with a home directory of the test's choosing, and no
environment variable, still sends the `Authorization` header.

`tests/help.rs` is the same layer: it runs the binary and compares `--help` with the block
in [`cli.md`](cli.md#help-text), which is the only way to hold the interface to the page.

## The fake server

`tests/support/` holds a small HTTP/1.1 server built on `std::net::TcpListener` — no mock
framework, and no HTTP server dependency in the manifest. It binds `127.0.0.1:0`, so it
takes any free port, and it publishes its `SocketAddr` once it is listening, so no test
races the bind.

```rust
/// One canned answer, in order. A test that scripts two answers is a test
/// that observes a retry.
struct Reply { status: u16, headers: Vec<(String, String)>, body: String }

/// A server that answers the first N requests from `replies` and records every
/// request it saw.
struct FakeServer { /* listener, handle, seen: Arc<Mutex<Vec<Request>>> */ }

impl FakeServer {
    fn start(replies: Vec<Reply>) -> Self;
    fn addr(&self) -> SocketAddr;
    fn url(&self) -> String;            // http://127.0.0.1:PORT
    fn seen(&self) -> Vec<Request>;     // method, path, headers, body
}

struct Request { method: String, path: String, headers: BTreeMap<String, String>, body: String }
```

Each connection reads the request line and headers, reads `Content-Length` bytes of body,
records the request, writes one canned response with `Connection: close`, and closes —
which is what makes the next attempt a new connection, and the retry observable as a second
entry in `seen()`. A server started with no replies left answers `500`, so a client that
retries when it should not fails loudly instead of hanging.

Two properties of this design matter and are chosen deliberately:

- **The assertion is on the request the server received**, not only on the response the
  client parsed. "The body contained the criteria I gave" and "the client said it sent
  them" are different claims, and only the first is the product working.
- **`--base-url` points at the fake server**, always explicit rather than left to
  `TYPESAFE_BASE_URL`, so an ambient variable in a developer's shell cannot redirect a test
  at the real API. Every subprocess test also runs with the `TYPESAFE_*` variables removed
  from its environment, for the same reason in reverse.

## Fixtures that nobody here wrote

`tests/data/` holds the request and response bodies copied verbatim from the
documentation — the quickstart's three-question example, and the `api.md` examples for
each answer type. They are replayed through the real client and asserted against, so the
parser is held to a shape this repository did not invent.

This is the cheapest defence there is against the most likely failure in a hand-written
client: a field name that is spelled the way we remember it rather than the way the API
spells it. `input_tokens`, `release_date`, and `legend` are all names that a plausible
guess could have got wrong, and a fixture that came from the vendor's own page is the only
thing that catches it.

Each fixture is also checked in the other direction: a response with an extra field the
documentation does not mention must still parse
([D8](design.md#d8-the-response-is-parsed-tolerantly-the-document-strictly)).

## What each claim's two directions are

Every row is a test that exists with both halves.

| The claim | The case that works | The case that fails |
|---|---|---|
| A document becomes a request | The quickstart document and the shorthand both serialise to the expected JSON | A document with `criteriaa`, a missing `questions`, an unknown `type`, an unparseable file |
| Every documented limit is enforced | 255 options, 10 levels, 2 levels, one question | 256 options, 11 levels, 1 level, zero questions |
| The state comes from one place | Each of the five sources alone | Two sources named; none named with a TTY; empty text; `--state-json` with `--no-state` |
| The client speaks the API | The request line, the `Authorization` header, the content type, and the body the server received | A base URL with no scheme; a base URL with a trailing slash (which must not become `//v1/…`) |
| Retries happen | `408`, `429`, `529`, `500` each retried, `Retry-After` honoured, success on the second attempt | `400` and `422` not retried; `--retries 0` gives exactly one attempt; exhaustion reports the count |
| The response becomes an answer | All three answer types from the vendor's fixtures | A missing answer for a question; an answer whose type disagrees with its question; a body that is not JSON |
| stdout is the response | The exact bytes, compact and pretty | A failed run leaves stdout empty; `--verbose` does not change stdout |
| A selected value is a usable value | `--select` into an object, an array, a number, a bool; `--value`; `--field confidence` | A path that misses names the segment and the available keys; `--value` on a three-question request is refused; `--value` and `--field` together are refused |
| A credential is found without an environment variable | Each of the three sources alone, in that order; a stored token used by a real call through the built binary | No source at all; a named file that cannot be read; `auth set` from a terminal or with an empty stdin; `auth status` never printing the value |
| The help is the interface | The block in [`cli.md`](cli.md#help-text), compared with what the binary prints; every flag present in `--help` and absent where it could not act | A description column that moved; `--api-key` parsed as a flag; an evaluation flag accepted by `models` or `auth` |

## The units, named

Test names state the behaviour, the way [`design.md`](design.md) states the reason. A
sample, because the names *are* the documentation of the suite:

```
wire::a_choice_without_options_is_refused;
wire::two_hundred_and_fifty_five_options_are_accepted_and_two_hundred_and_fifty_six_are_not;
wire::a_score_needs_at_least_two_levels;
wire::an_unknown_key_inside_a_question_names_the_question;
wire::an_extra_field_in_a_response_is_ignored;
wire::an_answer_whose_type_disagrees_with_its_question_is_refused;

input::a_piped_state_and_a_state_in_the_document_are_refused_together;
input::stdin_is_read_when_nothing_else_supplies_a_state;
input::a_terminal_with_no_state_named_is_a_usage_error;
input::whitespace_only_state_is_refused_because_a_pipeline_delivered_nothing;
input::no_state_sends_null;

cli::a_bare_invocation_prints_the_help;
cli::the_extraction_flags_conflict;

client::the_api_key_travels_in_the_authorization_header;
client::a_trailing_slash_in_the_base_url_does_not_double_the_path;
client::a_rate_limit_is_retried_after_the_retry_after_header;
client::a_validation_error_is_not_retried;
client::an_overloaded_is_retried;
client::a_server_that_never_answers_times_out;

config::the_store_supplies_the_credential_when_nothing_else_does;
config::a_named_file_that_cannot_be_read_does_not_fall_through_to_the_store;
config::the_store_is_readable_only_by_its_owner;
config::a_credential_is_read_from_a_pipe_and_not_from_a_terminal;
input::both_spellings_of_the_endpoint_reduce_to_the_same_root;
cli::auth_takes_no_credential_flag_either;

report::a_string_value_is_printed_without_quotes;
report::a_missing_path_names_the_segment_and_the_keys_available;
report::an_answer_keyed_with_a_dot_is_reachable_by_value_but_not_by_select;

error::a_usage_error_and_an_evaluation_failure_have_different_exit_codes;
```

## Timing

The retry backoff is the only thing in the suite that waits, and it is bounded by the knob
rather than by the default: the tests that observe a retry pass `--backoff-ms 1`. The
timeout test passes `--timeout 1` against a server that accepts and never replies. Nothing
sleeps to synchronise: the fake server publishes its address when it is listening, and
every wait is on a socket read that the server's close ends.

`slow-timeout` in the nextest profile is a guard, not a budget: 60 s per test means a
test that hangs is reported as a failure with its name attached rather than as a suite that
never finishes.

## Bugs this shape is meant to catch

This repository has no bug list. What it has is the list of the failures the *design* says
are likely, which is what the suite is built to make impossible:

- **A credential in the transcript.** Nothing reads `--api-key`, so nothing can leak one;
  a test asserts the flag is unknown.
- **A silent typo in a question.** A misspelled `criteria` is refused by name, so the
  answer that "came back fine" out of a rubric-less question cannot be read as a result.
- **An answer attributed to the wrong question.** Ids are checked to match their answers'
  types, so a renamed question is a hard error rather than a plausible number.
- **A partial answer on stdout.** Every write happens after the work is done, and the
  failure tests assert stdout is empty rather than asserting an exit code alone.
- **A retry that never ends.** The policy has three bounds — attempts, per-attempt timeout,
  and the total — and the tests pin the ones that are observable.

A test that is added because it was convenient, and not because it pins one of these, is
the kind of test that slows a suite down without making it stronger.

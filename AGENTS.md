# AGENTS.md

## What this is

`decide` is a small command-line client for [TypeSafe](https://docs.typesafe.ai)'s Jev
model. It takes a state and a set of typed questions — from flags, or from a JSON document
— and prints the structured answers as JSON for a shell script to branch on.

Its two callers shape everything: a shell script a human wrote, and a language model
driving a bash tool. The second one wrote the command once, cannot read this source, and
sees only what it printed. That is why `--help` is specified as interface rather than
described as documentation, why every mistake is refused locally and by name, and why the
output contract is strict.

**Status: implemented.** `src/` and `tests/` are here and the five gates below are green.
[`docs/`](docs) is the specification the code is held to: where a page and the code
disagree, one of them is a bug, and the fix is to decide which and say so.

## Repository layout

```
README.md            what it is, how to build it, and the two shapes of a call
AGENTS.md            this file
Cargo.toml           the manifest, the lints, and the release profile
Cargo.lock           committed, because this is a binary rather than a library
rust-toolchain.toml  the pinned toolchain
clippy.toml          the panic-family settings the lint cannot infer by itself
.config/nextest.toml the test runner's profiles
src/                 the implementation, module by module
tests/               the integration tests, the fake server, and the fixtures
docs/cli.md          the interface contract, including the help text verbatim
docs/design.md       the decisions, with the alternatives that were rejected
docs/api.md          the Jev API as this tool depends on it, with sources
docs/testing.md      what is asserted, in both directions, and how
docs/roadmap.md      what is in v1, what is out, and what would change the shape
```

The `src/` modules are the ones mapped in
[`docs/design.md`](docs/design.md#the-module-map); `Cargo.toml`, `rust-toolchain.toml`,
`clippy.toml`, and `.config/nextest.toml` are reproduced verbatim in
[`docs/design.md`](docs/design.md#toolchain-lints-and-the-test-runner).

## Where things are

One file per concern, and a test file per layer. Most changes touch one of each, so the
rightmost column is the one to read first.

| `src/` | Owns | Pinned by |
|---|---|---|
| `main.rs` | the process: `Io` from the real streams, `Cli::parse()`, `run()`, an `ExitCode` | `tests/binary.rs` |
| `lib.rs` | the crate docs and its runnable example, `Io`, `run()`, the glue between modules | every test |
| `cli.rs` | the clap types and the help text; the only module that knows about `argv` | `cli::tests`, `tests/help.rs` |
| `wire.rs` | the request and response shapes, the limit constants, validation | `wire::tests`, `tests/run.rs` |
| `input.rs` | `Stdin`, the state rules, reading a document, the flags-to-question builder, the base URL | `input::tests` |
| `config.rs` | the stored credential: its path, its file, the prompt that reads one, and which of the three sources supplies it | `config::tests`, `tests/binary.rs` |
| `client.rs` | the agent, the auth header, one attempt, the retry loop, the error-body cap | `client::tests`, `tests/transport.rs` |
| `report.rs` | path selection and rendering; no I/O at all | `report::tests`, `tests/run.rs` |
| `error.rs` | `DecideError`, its `Display`, and its exit code | `error::tests` |

`tests/support/mod.rs` is the fake server, shared by the three files that need a socket.
`tests/data/` holds the vendor's request and response bodies, plus a response with fields
the documentation does not mention. `tests/help.rs` compares `decide --help` with the block
in [`docs/cli.md`](docs/cli.md#help-text). `tests/live.rs` holds the one `#[ignore]`d test
that reaches the real API.

## Toolchain and setup

- Latest stable Rust, pinned in `rust-toolchain.toml` (1.98) with `rustfmt` and `clippy`.
- [`cargo-nextest`](https://nexte.st) runs the suite; install it before you need it.
- **No `TYPESAFE_API_KEY` is needed** to build, lint, test, or use `--dry-run`. The suite
  binds sockets on `127.0.0.1` and never reaches the network; the single test that calls
  the real API is `#[ignore]`d.
- The dependency set is the six in
  [`docs/design.md`](docs/design.md#the-dependency-set), plus `tempfile` for the tests.
  **`Cargo.lock` is committed**, because this is a binary: the versions that passed the last
  gate are the versions a fresh clone builds.

## Commands

```sh
# The five gates. All must pass, and all are expected to be clean.
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --all-features
cargo test --doc
cargo build --release

# Run a subset while iterating.
cargo nextest run wire::          # one module
cargo nextest run --profile ci    # retry and fail fast, as CI does

# The one test that needs the API.
TYPESAFE_API_KEY=… cargo nextest run --run-ignored ignored-only
```

As of this writing the suite is **203 passing, one `#[ignore]`d**, plus one doctest, and it
should never shrink. `cargo nextest run --all-features` prints the number; if a change makes
it smaller, the change is not finished.

## Running the binary

```sh
export TYPESAFE_API_KEY=…                                   # never a flag; see D5
cargo run -- noul "does this message convey urgency?" \
    --state-file ticket.txt --value
cargo run -- ask questions.json < ticket.txt
cargo run -- models --select models.0.name

# This one needs no credential and opens no connection: it prints the request.
cargo run -- choice "which team?" --no-state \
    --option billing --option technical --dry-run
```

`--dry-run` is the way to exercise the request builder by hand: it needs no credential, no
network, and prints exactly the bytes that would be sent — which also makes it the quickest
way to check a quoting problem in a shell.

## Conventions you must follow

Tiger Style, enforced by `clippy -D warnings` rather than aspired to. The full lint set is
in [`docs/design.md`](docs/design.md#toolchain-lints-and-the-test-runner).

- **`unsafe` is forbidden**, by a crate-level attribute *and* the manifest lint — the
  manifest alone does not cover doctests.
- **No `panic!`, `unwrap`, `expect`, `todo!`, `unimplemented!`, or `dbg!` in production
  code.** `assert!` is the sanctioned way to state an invariant, and it stays in release
  builds. `clippy.toml` allows the panic family in tests, where a panic is the assertion
  mechanism.
- **Arithmetic is explicit about overflow** — `checked_*` and `saturating_*`
  (`arithmetic_side_effects` and `integer_division` are denied). Retry counts and delays
  are the arithmetic here, and a wrapped delay is a negative sleep.
- **No recursion, and no panicking index arithmetic.** `indexing_slicing` is denied, so
  the `--select` path walk uses `.get()` and returns a `Result` — which it needs anyway.
- **70 lines per function, 100 columns per line.**
- **Errors are `Result`**, one enum per crate, with `Display` that names the thing that is
  wrong: the question id, the flag, the path, or the status.
- **`missing_docs` warns.** Every public item has a doc comment; `# Errors` sections are
  conventional on fallible functions.
- **No printing from anywhere.** All I/O is injected
  ([D14](docs/design.md#d14-all-io-is-injected-so-nothing-in-the-crate-prints)), so
  `print_stdout` and `print_stderr` are denied in the library *and* the binary — there is
  no crate-level exemption here, and adding one would be the first sign that the I/O
  boundary had been breached.
- **An integration test crate is not covered by `clippy.toml`.** `allow-expect-in-tests`
  only reaches code in a `#[cfg(test)]` module, and each file in `tests/` is its own crate,
  so every one of them opens with a named
  `#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]` and a comment saying
  why — the same distinction the setting exists to draw.
- When a lint cannot be satisfied, `allow` it **by name, at the narrowest site, with a
  comment saying why.**

## Testing conventions

- Unit tests live in `#[cfg(test)] mod tests`; integration tests live in `tests/`.
- **Name tests for the behaviour, not the function**:
  `a_trailing_slash_in_the_base_url_does_not_double_the_path`, not `test_base_url`.
- **Test both directions.** Every claim needs the case that works *and* the case that
  fails. A happy-path-only test is incomplete, and the negative cases are the point:
  a misspelled `criteria`, two state sources, a `400` that must not be retried, a body
  that is not JSON, a path that is not there.
- **Assert on what the server received**, not only on what the client parsed. A test that
  checks the client's own view of the request it sent proves the client is self-consistent.
- **Prefer the vendor's fixtures.** `tests/data/` holds request and response bodies copied
  from the documentation, so the parser is held to a shape nobody here wrote.
- The fake server is hand-rolled on `std::net::TcpListener` in `tests/support/`; there is
  no mock framework, and no HTTP server dependency in the manifest.
- **Nothing in the suite touches the network or sleeps to synchronise.** Waits are on
  socket reads the server ends. The only real delay is the retry backoff, and the tests
  that observe a retry pass `--backoff-ms 1`.
- Doctests are part of the gates: `cargo test --doc` must pass, and the crate-level
  example in `lib.rs` is expected to be runnable.
- **Never set an environment variable in a test.** The test process is shared and `set_var`
  is `unsafe` in edition 2024, which is forbidden here besides. The `run`-level tests point
  at a credential with `--api-key-file`; the subprocess tests control the child's whole
  environment with `env_clear()` and always pass `--base-url` rather than leaving
  `TYPESAFE_BASE_URL` to chance. That is what keeps an ambient variable in a developer's
  shell from aiming a test at the real API.
- **The help text is compared with the document**, in `tests/help.rs`, which reads the block
  out of `docs/cli.md` and compares it with what the built binary prints. One normalisation
  is applied — see the gotcha below — so a flag that loses its description, its value name,
  or its `[env: …]` note fails the suite.

## Invariants that are enforced by tests

These are the properties the suite exists for. Breaking one is a defect, not a preference.

1. **stdout carries the response and nothing else.** `--verbose` does not change a byte of
   it, and a failed run leaves it empty.
2. **The exit code separates the caller's mistake from the world's**: `0` completed, `1`
   the evaluation failed, `2` the invocation is wrong.
3. **The state comes from exactly one place.** Two *named* sources is an error that names
   both, and `--state-file -` is a named source like any other; a pipe is used only when
   nothing else is named. See the gotcha below for why that line is where it is.
4. **Every documented limit is checked before the request**, and every message names the
   question it is about.
5. **The document is strict, the response is tolerant.** An unknown key in a document is
   refused; an unknown field in a response is ignored.
6. **An answer matches its question.** A missing answer, or one whose `type` disagrees with
   its question, is a hard error.
7. **Retries are bounded and correctly scoped**: `408`, `429`, `5xx` and transport failures
   retry; `4xx` validation does not; `--retries 0` means one attempt.
8. **The credential never comes from `argv`.**
9. **`--dry-run` prints the request and opens no connection**, and needs no credential.

Each of these is asserted in both directions. The table under
[Where things are](#where-things-are) says which file holds which; the stdout and exit-code
claims live in `tests/run.rs` and `tests/binary.rs`.

## Critical gotchas

- **The three extraction flags are not one flag.** `--select` is a path from the response
  root and applies to every subcommand; `--value` is the single answer's own value; and
  `--field` is a path from that answer. The last two need a request that asked exactly one
  question, and all three print a string *raw*, with no JSON quoting. They conflict with
  one another, and all of them conflict with `--dry-run`.
- **clap exits the process on a usage error and on `--help`.** That is wanted (exit `2`
  and exit `0` respectively, with help on stdout), and it means the usage tests use
  `Cli::try_parse_from` while the help and version tests go through the built binary.
- **Declare a cross-set conflict on the local flag, not the global one.** `--select` is
  global and `--value`, `--field`, and `--dry-run` are not, and a `conflicts_with` declared
  on a local flag naming a global id is the arrangement clap resolves reliably in every
  subcommand that has both.
- **clap does not own the count rules.** `--option` and `--level` are deliberately *not*
  `required`, because `decide`'s own message ("a choice needs at least one option") is
  better than clap's, and because the 2..=10 level bound is the API's knowledge, not
  clap's.
- **`--base-url` needs a scheme and may end in a slash.** Both cases are validated or
  normalised, because `api.typesafe.ai` silently becomes a relative URL and
  `http://host/` + `/v1/systemone` silently becomes a double slash.
- **A question id may contain a dot, and a dotted path cannot address it.** `--value`
  sidesteps the problem, `--select` does not. Documented in
  [`docs/roadmap.md`](docs/roadmap.md#later-if-it-earns-it) rather than papered over.
- **serde_json sorts keys**, which is where the deterministic output comes from. When a
  test asserts on a body, compare parsed values, not strings.
- **A pipe is not a competing state source.** stdin supplies the state only when nothing
  else does, so a document that carries its own `state` — the documented shape of `ask` —
  still works when stdin is not a terminal, which is the case for `/dev/null` under cron,
  CI, and every subprocess. The conflict the documentation calls out is enforced where the
  caller was *explicit*, including `--state-file -` against a document state. `input.rs`
  records the reasoning and `input::tests` pins both halves.
- **`--value` and `--field` are refused before the request**, not when the response is read:
  "the value" of three answers is not defined, and exit `2` promises that nothing was
  attempted. A `--select` miss is the one refusal that necessarily lands after the response,
  because a path cannot be known missing until a body exists.
- **The help block's option column is one space narrower than the page's.** clap computes
  that column from the longest flag; the block in `docs/cli.md` was aligned by hand one
  further right. Do not "fix" either side — `tests/help.rs` normalises that one column and
  compares every other byte, which is what keeps the flag list honest.
- **`Stdin` carries `is_terminal` next to the reader.** A terminal and a pipe take different
  paths through the state rules, and that is the branch a unit test otherwise cannot reach.
- **`client.rs` writes progress to the writer it is handed**, never to a stream, because
  `print_stderr` is denied everywhere and `--verbose` has to be assertable in a unit test.
- **An error body is capped** at the constant in `client.rs`, read through a bounded reader,
  so a server that answers with a novel cannot become an allocation.
- **The credential has three sources, and the order is a decision.** `TYPESAFE_API_KEY`,
  then a named file, then the store `decide auth set` writes. The environment is first so a
  script can override what a developer stored without unsetting anything; a *named* file
  that cannot be read is an error rather than a fall through, because the caller said where
  the key is. `config.rs` holds all of it.
- **`auth set` prompts when it has a terminal, and reads a pipe when it does not.** The
  prompt is the default because the pipe is the one that leaks: `printf %s "$TOKEN" | decide
  auth set` puts the token in the shell's history, which is the same class of mistake as
  `argv`. The reading is behind the `config::Secret` trait so the interactive branch is a
  unit test — the real implementation needs a terminal, and the suite must never borrow the
  developer's.
- **The terminal handling is `rpassword`'s contract, not ours.** It clears
  `ECHO`/`ECHONL`/`ICANON`/`ISIG`, restores the terminal in a `Drop` guard, and re-raises
  SIGINT *after* restoring. Verified through a real pty once (token not echoed, Ctrl-C exits
  by SIGINT with nothing stored); the suite does not assert it, because a pseudo-terminal is
  one thing `std` does not have and a pty crate for one assertion is not worth the manifest.
- **`auth status` dials; `--no-check` is the local mode.** The call is `GET /v1/models`
  because it needs the same token and spends no tokens; it refuses a `200` that is not the
  API's answer, and it lets a refused key travel as the ordinary `401` status path so the
  exit code needs no new case. With no credential it ends in `NoCredentialConfigured`
  (exit `2`), which is the missing-key error plus the path the store would use.
- **The one file `decide` writes is the credential, not a config file.** It is a token in a
  file — not parsed, not merged, mode 0600 — and adding a second key to it is the point at
  which [D13](docs/design.md#d13-one-credential-store-and-still-no-configuration-file)'s
  objection becomes true again.
- **`--base-url` accepts the root and the endpoint in full.** `input::resolve_base_url`
  strips a trailing `/v1/systemone` before anything else looks at it, so the documented URL
  can be pasted in and `GET /v1/models` stays a sibling of the `POST`. Only that exact
  suffix is recognised, so a proxy path of your own is left alone.

## How to make common changes

**A new flag.** Add it in `cli.rs` with its help text, decide whether it is a knob
(overrides) or meaning (conflicts), implement it where its effect belongs, and add both
directions to the tests. If it changes what goes on the wire, update
[`docs/api.md`](docs/api.md); if it changes the interface, update
[`docs/cli.md`](docs/cli.md) and the help text.

**A new question type or primitive.** `Question` gains a variant, the shorthand gains a
subcommand, the answer validation gains a case, and `docs/design.md` records what the
system says about it. The three matches are exhaustive, so the compiler lists the work —
which is the reason the enum is closed.

**A documented limit changes.** Change the constant in `wire.rs`, the table in
[`docs/api.md`](docs/api.md#limits), and the test at the boundary in both directions.
A limit is never changed in one place only.

**The credential changes.** It is `config.rs` — the path, the file, the prompt, and the
order in
`config::resolve` — plus the table in [`docs/cli.md`](docs/cli.md#where-the-credential-comes-from),
[D5](docs/design.md#d5-the-credential-never-comes-from-argv) and
[D13](docs/design.md#d13-one-credential-store-and-still-no-configuration-file), and the
tests in both directions: what wins, what is refused, and what is never printed.

**The retry policy changes.** Change it in `client.rs`, the table in
[`docs/api.md`](docs/api.md#retries), the flag defaults in [`docs/cli.md`](docs/cli.md), and
the tests that pin what retries and what does not.

**The help text changes.** It is specified in [`docs/cli.md`](docs/cli.md#help-text);
change the definition in `cli.rs` and the page together, then run `decide --help` and read
it as the model that has never seen this repository would. `tests/help.rs` fails until the
two agree.

Whatever changes, run the five gates. `tests/help.rs` fails on a flag that loses its help
text, `tests/transport.rs` fails on a header or a retry that moves, and `tests/run.rs` fails
on a byte of stdout that is not what it was.

## Working agreement for agents

- **Read the relevant `docs/` page before editing.** The docs here are the specification,
  not a summary of the code: if the code and a page disagree, that is a bug in one of them,
  and the fix is to decide which and say so.
- **Match the surrounding voice.** Comments explain *why*, often at length, and a comment
  that describes behaviour you changed must change with it.
- **Run every gate before calling a change done.** Do not leave the lint status or the test
  count worse than you found it.
- **Do not weaken a test to make a change pass.** A refusal from the suite means the change
  and the specification disagree; decide which is wrong and record the answer in the change
  itself, in a comment on the code or the test.
- **Prefer editing files over rewriting them**, and keep diffs focused.
- **Do not add a dependency** without a line in
  [`docs/design.md`](docs/design.md#the-dependency-set) saying what it is for and what the
  smaller alternative was. The manifest is small on purpose.

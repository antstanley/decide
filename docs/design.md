# Design

**`decide` asks Jev a typed question and prints the answer as JSON, so a shell script can
branch on it.**

```console
$ decide noul "does this message convey urgency?" --state-file ticket.txt --value
0.95
```

That is the whole product. Everything below is either a reason for a choice or the
consequences of one.

## The two callers

Two kinds of program run this binary, and the second one shapes the interface more than
the first.

**A shell script a human wrote.** It wants a value on stdout and a meaningful exit code.
It was written by someone who can read the source, re-run the command, and fix it.

**A language model driving a bash tool.** It wrote the command once, at speed, with no
ability to read `decide`'s source, and it sees only what the command printed. It is
usually the *first* caller of a new command, and it is the one that will get the flags
wrong. Three consequences follow, and they are design constraints rather than
aspirations:

- **`--help` is part of the interface, not documentation about it.** It has to carry the
  shape of a correct call, in examples, because it may be the only thing the caller reads.
  The help text is specified verbatim in [`cli.md`](cli.md#help-text), and the examples
  come before the flag list.
- **A wrong invocation must be refused locally, by name.** A caller that has to make a
  network round trip to discover that `criteria` was misspelled learns slowly and pays
  tokens to do it. Every check that can be made before the request is made before the
  request.
- **Errors must name the thing that is wrong.** "invalid request" is useless to a caller
  that cannot read the code; "question `department`: a choice needs at least one
  `--option`" is actionable without any other context.

## Goals

1. **Faithful.** Anything the API can express, the CLI can send, and everything the API
   returns is printed. The CLI is a transport with opinions about *how* you ask, not
   about *what* you may ask.
2. **Composable.** stdout carries JSON and nothing else; diagnostics go to stderr; the
   exit code distinguishes "the invocation is wrong" from "the evaluation failed".
3. **Fast to call correctly, from a one-liner or a script.** The two shapes in the
   synopsis exist because those are the two real uses: one question asked once, and a
   stable set of questions asked of many states.
4. **Small.** One binary, six dependencies, no runtime, nothing to install beside it,
   and one file it writes for you — the credential, and nothing else. It should be
   auditable in one sitting.

## Non-goals

- **A gateway, proxy, or retry daemon.** One invocation makes one request. Batching
  states, caching, and concurrency are the caller's business; the API already batches
  *questions*.
- **A decision framework.** It does not threshold, weight, or combine answers. The
  documentation is emphatic that thresholds belong to the caller and should be reviewable
  in the caller's code, and a threshold hidden inside a CLI is exactly the opposite.
- **An SDK.** It is a program. Library bindings for Rust already exist in spirit as the
  HTTP API, and the official SDKs cover Python and JavaScript.
- **A model client for anything but Jev.** The endpoint is one POST and one GET, both
  documented in [`api.md`](api.md). Nothing provider-neutral is needed to speak them.

## The interface in one page

```
decide [GLOBAL] ask    [FILE]     [EVAL]
decide [GLOBAL] noul   <INSTRUCTION> [EVAL] --id NAME [--yes DESC] [--no DESC]
decide [GLOBAL] choice <INSTRUCTION> [EVAL] --id NAME --option NAME[=DESC]…
decide [GLOBAL] score  <INSTRUCTION> [EVAL] --id NAME --level DESC…
decide [GLOBAL] models
```

`GLOBAL` — accepted by every subcommand:
`--base-url URL` · `--api-key-file PATH` · `--timeout SECS` · `--retries N` ·
`--backoff-ms MS` · `--select PATH` · `--pretty` · `--verbose` · `-h/--help` · `-V/--version`

`EVAL` — accepted by `ask`, `noul`, `choice`, and `score`:
`--state TEXT` · `--state-file PATH` · `--state-json` · `--no-state` · `--model NAME` ·
`--dry-run` · `--value` · `--field PATH`

The full contract — every flag, every rule, every exit code — is [`cli.md`](cli.md).
The dependency set, the module map, and the toolchain are further down this page.

## Decisions

Each of these was a fork in the road. They are written down because a reader comparing
`decide` with `curl` on one side and the official SDKs on the other will otherwise assume
the differences are accidents.

### D1. A blocking HTTP client, and no async runtime

`ureq` rather than `reqwest` + `tokio`.

The API does not stream ([`api.md`](api.md#the-endpoint)). One request produces one
response body. The async machinery that a streaming client exists to serve — a runtime, a
`LocalSet`, a select over partial bodies — would be dead weight bought with a dependency
tree an order of magnitude larger and a class of bugs (`cannot start a runtime from within
a runtime`) that this program has no reason to be able to have.

The cost is real and small: `ureq`'s retry support is not built in, so the retry loop in
D10 is written by hand. It is about forty lines and it has to be tested either way.

*Rejected:* `reqwest` + `tokio` for parity with the sibling project this one was inspired
by. Parity of dependency set buys nothing here; the two programs do not share a line of
code.

### D2. One request per invocation

The documentation's most repeated piece of advice is to batch: every question that shares
a state goes in one call, because questions are evaluated in parallel and cost only their
own tokens. The `ask` subcommand exists to make that easy, and its help text leads with
it.

The advice is about *questions*, though, not *states*. Asking the same questions of forty
documents is forty requests; the shell already expresses that as a `for` loop or `xargs`,
and a concurrency policy inside `decide` would be a second, worse scheduler — one with no
knowledge of the caller's rate limit, ordering needs, or cost ceiling.

*Rejected:* `--each <glob>` (one request per file, results concatenated). It puts a
scheduler in the tool and a new output shape on stdout, for something `xargs -n1 -P4`
already does. See [`roadmap.md`](roadmap.md#out-of-scope-and-why).

### D3. stdout is the API's response and nothing else

On success, stdout carries one JSON value — the response body, exactly as the API sent
it, or one value selected out of it. It is written once, entire, after the work is
finished; a failed run leaves stdout empty rather than partial. Everything else — progress,
retry notices, errors — goes to stderr.

This is what makes `decide … > answers.json` and `x=$(decide … --value)` safe without a
single filter in between, which is the difference between a tool a script can use and a
tool a script has to parse.

### D4. Compact JSON by default; `--pretty` opts in

The default output is one line, keys sorted, no indentation. `--pretty` indents it.

The tempting alternative — pretty on a terminal, compact in a pipe — is refused because it
makes stdout depend on something the script does not control. A command whose output
changes shape depending on where it was run is a command that behaves differently in
`cargo test` than in the terminal it was written in, and the failure mode is a parser that
worked yesterday. The format is a flag, so it is a property of the invocation.

Keys come out sorted, always. The reason is mechanical and worth stating, because it is a
property of the code rather than of the struct declarations: every value that leaves the
program — the response, a value selected out of it, and the request body that is sent and
that `--dry-run` prints — passes through `serde_json::Value` before it is written or sent,
and `serde_json`'s default map is a `BTreeMap`. The body on the wire and the body in a dry
run are therefore the same bytes, and neither depends on the order the fields happen to be
declared in.

That is a feature rather than a side effect. The API's key order is not part of its
contract, so nothing is lost by sorting; and a sorted body is byte-stable across runs,
which is what makes the output comparable, diffable, and testable without a canonicalising
comparison.

### D5. The credential never comes from `argv`

`TYPESAFE_API_KEY`, a file named by `--api-key-file` or `TYPESAFE_API_KEY_FILE`, or the
store that `decide auth set` writes. There is no `--api-key`.

`argv` is readable by every process on the machine (`ps`, `/proc/<pid>/cmdline`), and it is
written to shell history and to the transcripts of the agents this tool is built to be
called by. A flag that puts a credential there is a flag that will be used, because it is
convenient. The SDKs read the environment for the same reason; `decide` reads what they
read. It is also why `decide auth set` reads the token from **stdin**: a command that takes
a secret as an argument is a command that puts it in `ps`, which would be this decision
broken by the command that exists to honour it.

`TYPESAFE_API_KEY_FILE` earns its place for the same reason in reverse: a container or a CI
runner mounts a secret as a file, and the alternative is a wrapper script that exists only
to move a file's contents into the environment.

The store earns its place for a third version of the same reason. The environment is not a
home for a credential on a developer's machine: an `export` in a shell profile is inherited
by every process that shell ever starts, and it is the first thing an agent's transcript
captures. One file, readable only by its owner, is the smallest thing that fixes both — and
the environment still wins over it, so a script or a CI runner overrides what is stored
without having to unset anything. The precedence is in
[`cli.md`](cli.md#where-the-credential-comes-from); the file itself is
[D13](#d13-one-credential-store-and-still-no-configuration-file).

`decide auth set` prompts when a terminal is watching and reads a pipe when one is not. The
prompt came second, and the reason it came second is worth recording: the pipe was the
original instruction, and
it was wrong. `printf %s "$TOKEN" | decide auth set` writes the token into the shell's
history, which is the same class of leak as `argv` and one this tool has no business
teaching. The earlier objection — that turning echo off needs a terminal-handling
dependency — turned out to be a two-crate dependency (`rpassword`, and `libc`, which was
already in the tree) that also does the part a hand-rolled `stty -echo` gets wrong: it
restores the terminal in a `Drop` guard and re-raises SIGINT *after* restoring, so an
interrupted prompt does not leave a shell with no echo.

*Rejected:* `--api-key`, for the reason above. *Rejected:* the `stty -echo` idiom, which
needs no dependency at all: it shells out, it is unix-only, and Ctrl-C at the prompt kills
the process with echo still off, which is a worse bug than the one it fixes.
*Rejected:* an interactive prompt with echo left on, which is not a credential store but a
way of printing the credential.

### D6. Two forms for the questions: flags for one, a document for many

A question's `instructions` and `criteria` may be strings, objects, or arrays
([`api.md`](api.md#question-types)). A shell flag syntax for an arbitrarily nested JSON
value is either a mini-language nobody will remember or a lossy one, and a lossy one is
worse: it silently changes the question.

So the line is drawn where the shell stops being able to carry the value:

- **`ask` takes a document** — a file or stdin, in the API's own request shape. It carries
  everything: many questions, structured instructions, structured criteria, a state, a
  model. It is the complete form, and it is the reason no feature is "missing" from the
  flags.
- **`noul`, `choice`, and `score` build one question from flags**, for the string-typed
  common case: one instruction, a list of options or levels, optionally some criteria. The
  flags carry exactly the shapes a shell can carry, and no more.

Because every flag has an exact document equivalent, the flags are a shorthand and never a
second implementation: `input.rs` builds the same `Request` either way, and one validator
checks it.

*Rejected:* flags for everything, including `--option-json '{"a": {…}}'`. It reads as
complete while getting the common case wrong (quoting a nested object inside a shell
argument is the hard part, not the type), and it doubles the surface to document.

### D7. The state comes from exactly one place

| Source | Supplies |
|---|---|
| `--state TEXT` | the text itself, or JSON with `--state-json` |
| `--state-file PATH` | that file's contents (`-` means stdin) |
| stdin | used when no other source is given and stdin is not a terminal |
| `--no-state` | an explicit `null` |
| the document's `state` (in `ask`) | whatever JSON value is there, including `null` |

At most one. Two is a usage error that names both, because a state given twice is a
mistake with no defensible winner: the document's sample state silently replacing a piped
document is exactly the kind of quiet wrongness that costs an afternoon.

If none is given and stdin *is* a terminal, that is a usage error naming `--state`,
`--state-file`, and `--no-state` — not a hang. If the text read from a file or stdin has
no non-whitespace character, that is a usage error too: the classic cause is a pipeline
that delivered nothing, and asking Jev a question about an empty string is never what was
meant.

`--no-state` exists because `state` may be `null`. A question can be self-contained
("is this statement true?"), and a CLI that cannot say so cannot express every request the
API accepts — which would break Goal 1.

*Rejected:* inferring the state from stdin whenever stdin is not a terminal, even when the
document has one. Convenient, and it makes `cat doc.txt | decide ask questions.json` mean
two different things depending on whether `questions.json` happens to carry a state.

### D8. The response is parsed tolerantly; the document strictly

Two opposite rules, because the two JSON values have different authors.

**The request document is parsed strictly.** An unrecognised key anywhere — a top-level
typo like `"question"` for `"questions"`, or `"criteriaa"` inside a question — is refused
with the key and its location named. The caller wrote this document and is in the same
room as the fix. A tolerance here buys nothing and costs the one error that matters most:
a misspelled `criteria` silently turns a carefully-written rubric into a question with no
rubric at all, and the answer still comes back, looking fine.

**The response is parsed tolerantly.** Unknown fields are ignored. TypeSafe owns this
value, and the deployment a caller is talking to can gain a field at any time; a client
that refuses to read a response because it grew a key is a client that breaks on someone
else's release schedule. The fields `decide` actually consumes are still validated — an
answer whose `type` disagrees with its question, or a question with no answer, is a
contract violation and a hard error — but the mere presence of something new is not.

*Rejected:* strictness in both directions. It converts every server-side addition into a
client outage. *Rejected:* tolerance in both directions, for the reason above.

### D9. The limits are enforced before the call

The documented limits ([`api.md`](api.md#limits)) that are checkable without a tokenizer
are checked locally and refused with exit code 2: at least one question, at least two and
at most ten score levels, at most 255 choice options, non-empty ids, non-empty
instructions, and a state that is a string, object, or array rather than a number or a
boolean.

The point is the error the caller gets. The API would refuse the same request with a
`422`, which costs a round trip, a rate-limit slot, and — for the second caller — a turn
of an agent's budget to learn something the local process already knew. The check is also
*free*: it happens while the request is being assembled anyway.

The token budget is deliberately not checked. It needs the model's own tokenizer, which is
not published, and a guess would be a warning that fires on correct requests and misses
incorrect ones.

*Rejected:* sending whatever the caller asked for and relaying the `422`. It is less code,
and it makes the tool slower and more expensive for its heaviest caller.

### D10. Retries on by default, with the SDK's policy

`decide` retries `408`, `429`, and `5xx`, plus transport failures, with the documented
policy: two retries, 500 ms doubling to a 5 s cap, 25% jitter, `Retry-After` honoured up
to 60 s ([`api.md`](api.md#retries)).

It does so because the caller's alternative is a retry loop in shell, and because the API's
own rate limits are documented as adjusting dynamically — a `429` is an expected event, not
an incident. A cron job that fails because of one `429` is a cron job that needs a wrapper
script, and a tool that needs a wrapper script is a tool that has not finished.

`--retries 0` turns it off, and `--timeout` bounds one attempt, so the worst case is
`(1 + retries) × timeout` and is a number the caller can compute. Both are documented in
the flag's help rather than left to be discovered.

*Rejected:* retrying nothing (the caller's problem) and retrying indefinitely (a hung
cron job). *Rejected:* an environment variable for the policy. The environment is for
things that differ between machines — credentials and endpoints; how hard to try is a
property of the invocation.

### D11. The exit code says whether the command worked, not what the answer was

| Code | Meaning |
|---|---|
| `0` | The evaluation completed. Printed on stdout. |
| `1` | The evaluation was attempted and did not complete: transport, HTTP error status, undecodable response, or a response that broke the answer contract. |
| `2` | The invocation is wrong: a flag, a document, a path, or the environment. Nothing was attempted. |

Anything else means the process itself died, which is a bug in `decide`.

An exit code that encoded the answer — `1` for "no", say — was considered and rejected for
three reasons. An exit code is one value, and "yes", "no", and "the call failed" need
three; reusing `1` for both "no" and "the network is down" makes the common case (`set -e`,
`&&` chains) abort a script that had a perfectly good answer. The answer is data, and it
already has a channel. And a threshold is policy, which the documentation insists belongs
in the caller's code where it can be reviewed — `decide` shipping one would be shipping a
default nobody chose.

### D12. `--dry-run` prints the request and makes no call

It prints the exact bytes that would be sent, to stdout, and exits 0. It needs no
credential.

For the second caller this is the review step: a model that has assembled a five-question
request can look at what it is about to ask before spending tokens on it, and a script
author can debug a quoting problem without a key. It also makes the request-building half
of the program testable with no server at all, which is where most of the validation
behaviour is tested.

`--dry-run` is defined on the four evaluation subcommands and not on `models`, because a
GET has no body to show. Its output is the *re-serialised* request, not the document as it
was read: it shows what would go on the wire, including the resolved model and the
canonical key order.

### D13. One credential store, and still no configuration file

Every setting is a flag or one of the documented environment variables, with one exception:
the API token, which `decide auth set` writes to `$XDG_CONFIG_HOME/decide/api-key`, or to
`~/.config/decide/api-key` when that variable is unset.

A configuration file is a second place a setting can come from, and therefore a second
place to look when a script behaves differently on a colleague's machine. The settings here
are few, they differ per invocation rather than per person, and the *script* is already the
file that records them — so none of them is in a file. The credential is the exception
because it is not a setting: it is a secret with one correct value, and both of its other
homes are worse. `argv` is shown in `ps` and kept in shell history
([D5](#d5-the-credential-never-comes-from-argv)), and a variable exported from a shell
profile is inherited by every process that shell starts, including ones with no business
holding it.

The file is a token in a file rather than a format: one setting, not parsed, not merged
with anything, and readable only by its owner — created that way, and narrowed if it was
already there with wider permissions. That is what keeps this from being
the configuration file the rest of this decision refuses — a *second* key in it would be
the point at which the objection bites, and the point at which this decision needs
revisiting. `TYPESAFE_LOG_LEVEL` is still unused for the original reason: it configures a
library's logger, and a program with one flag that means "say what you are doing on stderr"
does not need a second, differently-behaved way to ask.

*Rejected:* a config file for the defaults a user always passes. It would save typing in an
interactive session and add a hidden input to every script — and this tool's callers are
scripts. *Rejected:* a keychain or credential helper (`security find-generic-password` on
macOS, libsecret elsewhere). It puts the secret somewhere better, at the cost of a
dependency and a per-platform implementation of something one file does portably.
*Rejected:* a `.env` file in the working directory, which is a hidden input that depends on
where the command was run from.

### D14. All I/O is injected, so nothing in the crate prints

The library never touches a stream. `run` receives a struct holding stdin, stdout, and
stderr as `Read`/`Write` values, so every behaviour that matters — what lands on stdout,
what lands on stderr, whether stdin was read, whether a partial answer can be seen — is
asserted in a unit test with in-memory buffers rather than captured from a subprocess.

`entry()` is then about thirty lines — build the struct from the real streams, parse with
clap, run, report, return an exit code. Because nothing outside `entry` touches a stream, and
`entry` touches them only through `Io`, the `print_stdout` and `print_stderr` denials hold for
the whole crate, including the binary — there is no crate here that needs the exemption.

### D15. Safe Rust, no panics, no recursion

`unsafe` is forbidden by a crate-level attribute and by the manifest lint, because the
manifest alone does not cover doctests. Nothing in this program needs it: the whole
surface is a JSON value, a socket, and a file.

There is no `unwrap`, `expect`, `panic!`, `todo!`, or `unimplemented!` in production code;
`clippy.toml` allows the panic family in tests, where a panic is the assertion mechanism.
Arithmetic uses `checked_*`/`saturating_*` (`arithmetic_side_effects` is denied), and
`indexing_slicing` is denied as well, so the path walk in `report.rs` uses `.get()` and
returns a `Result` — which is what `--select` needs anyway, since a path that is not there
is a caller error to report, not a crash.

### D16. Three extraction flags, split by how far the path reaches

The response is a nested JSON value and the caller usually wants one leaf of it. Three
flags cover that, and the division between them is the thing worth stating:

- **`--select PATH`** is a dotted path from the response root
  (`--select usage.input_tokens`, `--select answers.department.choice`,
  `--select models.0.name`). It works on every subcommand, because every subcommand has a
  response.
- **`--value`** prints the single answer's own value (`noul`, `choice`, or `score`). It
  applies when the request asked exactly one question — `ask` included, since a document
  with one question is still one question.
- **`--field PATH`** prints a dotted path from that one answer
  (`--field confidence`, `--field probabilities.billing`). Its rules are `--value`'s, one
  level down.

The scripting case is the reason the last two exist at all. `decide choice … --value` puts
the chosen option on stdout without the script knowing that the answer is called `answer`,
that a choice answer hides its option in a field also called `choice`, or that the response
wraps it in `answers`. Those facts belong in the tool, which is the thing that knows them.

A string prints raw in all three, with no JSON quoting, so `$(…)` yields the option name
itself rather than a quoted JSON string. A number prints as JSON prints it, which is why
the documentation says to compare it with `awk`, `bc`, or `jq -e`: rounding an answer on
the caller's behalf would be changing it.

*Rejected:* one flag with an optional value (`--value [FIELD]`). Every way of spelling an
optional value in an argument parser is worse than a second flag: either the value is
greedy, so `--value <the instruction>` swallows the instruction it was standing next to, or
the space-separated form is forbidden and it becomes `--value=confidence`, which is a rule
a caller has to be told rather than shown. Two flags — one bare and one that takes a value —
have no such case, and the help text lists them side by side.

*Rejected:* making `--value` and `--field` global, for the symmetry with `--select`. They
need an answer and `models` has none, so a global placement would mean accepting a flag
whose only possible outcome is a refusal. A flag a subcommand cannot honour is better absent
from that subcommand's parser, where the error names the subcommand instead of a rule.

### D17. A model is a knob; a state and a question are meaning

`--model` overrides a `model` in the document, silently. Every other double-specification
— two state sources, questions from two places — is an error.

The asymmetry is the point. A model name is operational: it differs between a developer's
machine and CI, an alias moves when a release ships, and a caller that pins a version for a
week pinning it *on the command line* is doing the right thing. A state or a question is
what the answer is about; two versions of it do not have a precedence, they have a
disagreement, and the only safe response is to stop.

The same rule covers `--base-url`, `--timeout`, `--retries`, `--backoff-ms`, and
`--api-key-file`: knobs override, meaning conflicts.

### D18. `auth status` is local; `--check` is the one that calls

`decide auth status` answers "where does the credential come from" without touching the
network. That is the command a caller runs when a call has *already* failed, and a
diagnostic that needs the thing it is diagnosing is not much of a diagnostic.

`--check` adds the other half of the question — whether the API accepts the key — as an
opt-in, because it changes both what the command needs (a network, a timeout, a retry
policy) and what it can do (fail in ways that have nothing to do with the credential).

The call is `GET /v1/models`. It needs the same bearer token, and unlike an evaluation it
spends no tokens, which is what makes it cheap enough to run after every `auth set`. A key
the API refuses is left as the `401` the rest of the program already reports, so the exit
code, the reason phrase, and the body are the ones a caller has read before, and
`decide auth status --check || exit 1` is the whole script. A `200` that is not the API's
answer — a proxy, a captive portal — is not an acceptance, because the check parses the
body.

*Rejected:* checking on every `auth status`. It would make a local command depend on a
network and a clock, and it would break the property the suite is built on: that `status` is
a question about this machine. *Rejected:* checking with the evaluation endpoint, which
answers the same question for the price of a state, a question, and the tokens the two cost.
*Rejected:* a separate `auth check` action, which reads as two questions where there is one.

## The module map

```
src/
  main.rs      the binary: build Io from the process streams, Cli::parse(),
               run(), return an ExitCode. Nothing else. ~30 lines.
  lib.rs       the crate doc comment (with a runnable example), run(), the
               module declarations and the public re-exports.
  cli.rs       the clap types — Cli, Command, EvalArgs — and the help text.
               The only module that knows about argv.
  wire.rs      the API vocabulary: Request, Model, Question (noul | choice |
               score), Criteria, Response, Answer, Usage, ModelsResponse; the
               limit constants; validation. The only module that knows the
               documented shapes.
  input.rs     where the parts of a request come from: the state resolution
               rules of D7, the document parse, the flags-to-question builder of
               D6. Holds Stdin, so the terminal branch is testable.
  config.rs    the one file decide writes: where the credential is stored, how
               it is read and written, which of the three sources supplies it,
               and the stdin rule for `auth set`.
  client.rs    the transport: the ureq agent, the auth header, one attempt, the
               retry loop of D10, status handling, and the error-body cap.
  report.rs    what comes back: the Outcome of a run, path selection, and
               rendering to a String per D3, D4, and D16. No I/O.
  error.rs     DecideError, one enum with the usage/evaluation split of D11,
               its Display, and its exit code.
```

The seams are the ones worth testing across: `wire` ↔ `input` is "a document becomes a
request", `client` ↔ `wire` is "a response body becomes an answer", and `report` ↔ `wire`
is "an answer becomes a line". Each boundary is a place a test can sit with no socket in
the middle.

## The dependency set

Six, and every one of them is load-bearing.

| Crate | Version | Why it is here | Why nothing else is |
|---|---|---|---|
| `clap` | 4.6 | Flags, subcommands, help text, `env` for the three documented variables, `wrap_help` so the examples re-wrap to the terminal. | Derive-based, and the alternative is writing an argument parser, which is where a CLI's bugs live. |
| `serde` | 1.0 | The request and response shapes, with the tagged question enum. | — |
| `serde_json` | 1.0 | The wire format, the state value, path selection. | Sorted maps by default, which is D4's determinism for free. |
| `ureq` | 3.4 | One blocking POST and one GET, TLS via rustls (its default), gzip. | 3.4 as of 2026-09-20. The `json` feature is added for `send_json`; the defaults already carry rustls and gzip. |
| `rpassword` | 7.5 | Reading a secret at a terminal without echoing it, for `decide auth set`. | Clears ECHO/ECHONL/ICANON/ISIG, restores the terminal in a `Drop` guard, and re-raises SIGINT after restoring. Its two crates are this one and `rtoolbox`; `libc`, which it needs, was already in the tree. The smaller alternative is `stty -echo` as a subprocess, which is rejected above. |
| `thiserror` | 2.0 | The error enum's `Display` and `From` impls. | Hand-writing `Display` for ~15 variants is more code with more ways to be inconsistent. |

Dev-dependency: `tempfile` 3.27, for the file and stdin cases in the tests. The fake HTTP
server is written by hand against `std::net::TcpListener`
([`testing.md`](testing.md#the-fake-server)), so no mock framework is needed.

Deliberately absent: `tokio` and `reqwest` (D1), `tracing` (D13), `anyhow` (an error enum
is the house style, and this program has a small, closed set of failures), `rand` (the
jitter draws its entropy from the clock — the value only has to be unpredictable, not
unbiased), and `indexmap` (D4).

## Toolchain, lints, and the test runner

These files are the first implementation step. They are reproduced here in full so that
step is a copy rather than a design decision made at the keyboard.

Latest stable Rust, pinned so a reader cannot get a different answer:

```toml
# rust-toolchain.toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
profile = "minimal"
```

```toml
# Cargo.toml
[package]
name = "decide"
version = "0.1.0"
edition = "2024"
rust-version = "1.98"
license = "MIT"
description = "Ask Jev for a typed decision, and get JSON a shell script can branch on."
readme = "README.md"
keywords = ["cli", "ai", "llm", "shell", "json"]
categories = ["command-line-utilities"]

[dependencies]
clap = { version = "4.6", features = ["derive", "env", "wrap_help"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
thiserror = "2.0"
ureq = { version = "3.4", features = ["json"] }

[dev-dependencies]
tempfile = "3.27"

# `src/lib.rs` and `src/main.rs` share the package name, so the binary's
# `use decide::…` resolves to the library beside it, and no `[[bin]]` table is
# needed. Integration tests reach the library and, through
# `env!("CARGO_BIN_EXE_decide")`, the built binary.

[profile.release]
lto = "thin"
codegen-units = 1
strip = "debuginfo"
# A panic here is a bug in a program that forbids panics in production code, and
# an abort keeps the binary small. It also means a bug's exit code is 134, which
# is outside the documented set of 0, 1, and 2 — deliberately, because "the
# process died" is not a result.
panic = "abort"

[profile.test]
opt-level = 1
```

```toml
# Cargo.toml, continued
[lints.rust]
unsafe_code = "forbid"          # and #![forbid(unsafe_code)] in lib.rs, which
                                # is what covers the doctests
missing_docs = "warn"
unreachable_pub = "warn"
unused_qualifications = "warn"
rust_2018_idioms = { level = "warn", priority = -1 }
non_ascii_idents = "warn"
single_use_lifetimes = "warn"
trivial_casts = "warn"
trivial_numeric_casts = "warn"
macro_use_extern_crate = "warn"
meta_variable_misuse = "warn"
missing_abi = "warn"
elided_lifetimes_in_paths = "warn"

[lints.clippy]
# A lint *group* at the same priority as an individual entry wins, which is why
# the groups are pinned to -1 and every override below can take effect.
all = { level = "warn", priority = -1 }
pedantic = { level = "warn", priority = -1 }
nursery = { level = "warn", priority = -1 }
cargo = { level = "warn", priority = -1 }
missing_const_for_fn = "allow"      # fires on constructors that cannot be const
multiple_crate_versions = "allow"   # nothing here can pin a transitive major
cargo_common_metadata = "allow"     # the manifest above carries the metadata
unwrap_used = "deny"
expect_used = "deny"
panic = "deny"
todo = "deny"
unimplemented = "deny"
dbg_macro = "deny"
exit = "deny"                       # `main` returns an ExitCode
arithmetic_side_effects = "deny"    # retry counts and delays use checked/saturating
integer_division = "deny"
indexing_slicing = "deny"           # the path walk uses .get(), not [i]
print_stdout = "deny"               # all I/O is injected (D14), so these hold
print_stderr = "deny"               # everywhere, including the binary
module_name_repetitions = "allow"
missing_errors_doc = "allow"
missing_panics_doc = "allow"
must_use_candidate = "allow"
struct_excessive_bools = "allow"
too_many_lines = "warn"
cognitive_complexity = "warn"
cast_possible_truncation = "warn"
```

```toml
# clippy.toml
# Tiger Style keeps `assert!` in production and permits the panic family in tests,
# where a panic *is* the assertion mechanism. These three settings state the
# distinction the lint cannot see by itself.
allow-panic-in-tests = true
allow-unwrap-in-tests = true
allow-expect-in-tests = true
# A `dbg!` left in a test is still a debugging artefact with no place in a
# committed test suite.
allow-dbg-in-tests = false
too-many-arguments-threshold = 6
type-complexity-threshold = 200
```

```toml
# .config/nextest.toml
[store]
dir = "target/nextest"

[profile.default]
retries = 0
fail-fast = false
test-threads = "num-cpus"
slow-timeout = { period = "60s", terminate-after = 3 }
leak-timeout = "100ms"

# Retry and fail fast in CI: a test that passes on a retry is a test to look at,
# not a test to trust.
[profile.ci]
retries = 2
fail-fast = true
slow-timeout = { period = "30s", terminate-after = 2 }
```

```gitignore
# .gitignore
/target/
.env
*.local.toml
.DS_Store
*.swp
.idea/
.vscode/
```

The gates, all of which must be clean:

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --all-features
cargo test --doc
cargo build --release
```

## Failure modes

What each thing looks like from outside, which is the only view the second caller has.

| Cause | Exit | stdout | stderr |
|---|---|---|---|
| The evaluation succeeded | `0` | the response, or the selected value | empty, or the `--verbose` lines |
| `--dry-run` | `0` | the request body | empty |
| The credential is missing or unreadable | `2` | empty | names `TYPESAFE_API_KEY` / `TYPESAFE_API_KEY_FILE` |
| A flag conflict or a missing required flag | `2` | empty | clap's usage message, naming the flags |
| The document is malformed, or has an unknown key | `2` | empty | the key, its location, and what was expected |
| The document breaks a documented limit | `2` | empty | the question id, the limit, and the count given |
| The state came from nowhere, or from two places | `2` | empty | both sources by name, or the flags that would supply one |
| A `--select`, `--value`, or `--field` path is not in the response | `2` | empty | the path, the segment that failed, and the keys available there |
| The connection failed, after the retries | `1` | empty | the transport error, and the number of attempts |
| The API answered `401`, `422`, `429`, or `529` | `1` | empty | the status, the reason phrase, and the error body |
| The response was not JSON, or broke the answer contract | `1` | empty | what was expected and what arrived |
| The attempt timed out | `1` | empty | the timeout, and the URL |

Every row writes nothing to stdout. That is the property that makes the output stream safe
to consume without checking the exit code first — and D3 is the reason it holds.

## Implementation order

Each step is independently reviewable and leaves the gates green.

1. `Cargo.toml`, `rust-toolchain.toml`, `clippy.toml`, `.config/nextest.toml`,
   `.gitignore`. No source yet; this is the constraints made executable.
2. `error.rs` and `wire.rs` with their unit tests: the shapes, the limits, the strict
   document parse, the tolerant response parse.
3. `cli.rs` with the parsing tests, in both directions.
4. `input.rs`: state resolution (with `Stdin`) and the two question sources.
5. `report.rs`: selection and rendering.
6. `client.rs` against the fake server, including the retry and timeout cases.
7. `main.rs`, `run`, and the subprocess tests that pin the exit-code table.
8. The recorded fixtures, the doctests, and [`testing.md`](testing.md) verified by running
   every gate for real.

## Open questions for review

These are the choices I would most like a second opinion on before they become code.

1. **The names.** `decide` for the binary, `ask` for the document subcommand,
   `--select`/`--value`/`--field` for the three extraction forms, and
   `--option`/`--level`/`--yes`/`--no` for the shorthand criteria.
2. **`ask [FILE]` as a positional** rather than `--request FILE`. A positional is shorter
   and `ask -` reads naturally, at the cost of being less greppable in a script.
3. **Noul criteria in the shorthand** (`--yes`/`--no`). They are a real improvement to a
   yes/no judgment, and they are the only criteria the shorthand carries that are not a
   list. The alternative is to leave them to the document.
4. **Strict document parsing** (D8). It is the right default for a document the caller
   wrote — but if TypeSafe adds a request field, `decide` refuses a document the API would
   have accepted, until it is updated.
5. **`models` in v1.** It is ten lines and it lets a caller discover the model ids, at the
   cost of a fifth of the surface. The alternative is to leave it to `curl`.
6. **No `--api-key`** (D5). It is defensible and it will be the first thing a user tries.
   The help text has to say why.

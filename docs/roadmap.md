# Scope

What v1 is, what it deliberately is not, and what would have to be true for the second list
to change. The reasoning behind the boundaries is in [`design.md`](design.md); this page is
the checklist.

## In v1

Everything here is specified in [`cli.md`](cli.md) and asserted by a test
([`testing.md`](testing.md)). Nothing on this list is "nice to have".

| # | Thing | Why it is in |
|---|---|---|
| 1 | `ask [FILE]`, with `-` and stdin | The batching form. The documentation's strongest advice is to put every question that shares a state in one call, and this is the only form that can express structured instructions and criteria. |
| 2 | `noul` / `choice` / `score` from flags | The one-liner. A shell script should not have to write a file to ask one question. |
| 3 | `models` | Lets a caller discover the aliases without `curl`, and it is one GET. |
| 4 | `--state`, `--state-file`, stdin, `--no-state`, `--state-json` | The state is the input; being unable to say where it comes from is being unable to use the tool in a pipeline. `--no-state` exists because the API accepts `null`. |
| 5 | `--select`, `--value`, and `--field` | The difference between a tool a script pipes into `jq` and a tool a script can use directly. |
| 6 | `--dry-run` | The review step for an agent caller, and it makes the request builder testable with no server. |
| 7 | `--pretty`, `--verbose` | One for a human reading, one for a human debugging. Neither touches stdout's contents. |
| 8 | Local validation of every documented limit | Turns a paid round trip and a `422` into an instant, named error. |
| 9 | Retries with the SDK's policy, and `--retries`/`--backoff-ms`/`--timeout` | A `429` in a cron job should not be a failed run, and the caller must be able to bound the wait. |
| 10 | The exit-code contract: `0`, `1`, `2` | The only thing a shell script can branch on without parsing. |
| 11 | `decide auth set`/`unset`/`status`, and the one file it writes | A credential should not have to live in an environment variable that leaks into every child process, and it must not live in `argv`. |

## Out of scope, and why

Each of these was considered on its merits and rejected for a reason that is a property of
the tool, not a shortage of time. If the reason stops holding, the item comes back.

| Not in v1 | Reason |
|---|---|
| **A non-zero exit code that encodes the answer** (e.g. `1` for "no") | An exit code is one value and "yes", "no", and "the call failed" are three. Overloading `1` makes `set -e` and `&&` abort a script that had a valid answer, and it puts a threshold inside the tool — which the documentation says belongs in the caller's code where it can be reviewed. See [D11](design.md#d11-the-exit-code-says-whether-the-command-worked-not-what-the-answer-was). |
| **`--threshold` / `--combine` / weighted scoring** | Policy. The documentation is explicit that thresholds must be evaluated on the caller's data and consequences, and that the questions and thresholds are the two things a human should review. A default inside `decide` is a default nobody chose. |
| **`--each <glob>`, batch over many states** | The API batches *questions*; batching *states* is a scheduler, and the shell already has one (`for`, `xargs -P`). A concurrency policy inside `decide` would be a second, worse one, with no knowledge of the caller's rate limit or cost ceiling. |
| **A configuration file** | A second place a setting comes from, and so a second place to look when two machines disagree. Every *setting* here is per-invocation, and the script is already the file that records it; the one file `decide` writes holds the credential, which is a secret rather than a setting. A second key in that file would be this rejection broken. See [D13](design.md#d13-one-credential-store-and-still-no-configuration-file). |
| **`--api-key`** | A credential in `argv` is readable by every process on the machine and is kept in shell history and in agent transcripts. See [D5](design.md#d5-the-credential-never-comes-from-argv). |
| **Streaming output** | The API does not stream ([`api.md`](api.md#the-endpoint)). There is nothing to stream. |
| **Structured criteria through the flags** (`--option-json`, `--instructions-json`) | The document already carries them faithfully. A flag that holds a nested JSON object is a quoting problem disguised as a feature, and a lossy version of it silently changes the question. See [D6](design.md#d6-two-forms-for-the-questions-flags-for-one-a-document-for-many). |
| **A token-budget check against the 64k/32k limits** | It needs the model's own tokenizer, which is not published. An approximation would warn on correct requests and miss incorrect ones, which is worse than silence. Revisit if TypeSafe publishes a tokenizer or a `usage`-style dry run. |
| **Caching, or a session log** | The tool is stateless by design. Anything that remembers an answer creates a second source of truth about what was asked, and the caller's own files are a better one. |
| **Shell completions** | A real convenience, and it belongs in the packaging step (`clap_complete` at build time) rather than in the runtime. Not needed by either caller in the first version. |
| **A library API for Rust callers** | The crate *is* a library with a `run` entry point, so a Rust caller can use it — but it is not designed or documented as one, and pretending otherwise would freeze an internal shape. The HTTP API is the interface for a Rust program. |

## Later, if it earns it

In rough order of how likely each is to be worth its surface.

1. **A `--select` escape for question ids containing a dot.** Dotted paths can address
   `answers.is_urgent.noul` but not an id that itself contains a dot. `--value` and
   `--field` sidestep it, so the gap is narrow; a bracket form (`answers.[my.id].noul`) is
   the fix if anyone hits it.
2. **`decide ask --check`**, validating a document and reporting every problem at once
   instead of stopping at the first. An agent that has written a five-question document
   would learn faster from four errors than from one, four times.
3. **A `--format table` for `models`.** JSON is the right default for a script; a human
   running `decide models` in a terminal is reading a wall of braces.
4. **Per-question `--id` prefixes for a batch built from flags** — several questions on one
   command line. It is the shape [D6](design.md#d6-two-forms-for-the-questions-flags-for-one-a-document-for-many)
   rejected, and the objection holds until the flag list carries structured criteria, which
   is the thing that makes a document necessary.
5. **A property test for the path parser.** The path grammar is small and enumerable, and
   the table of cases in [`testing.md`](testing.md) is currently complete; a `proptest`
   dependency would buy coverage the table already has.

## What would change the shape

Recorded because each would invalidate a decision rather than add a feature.

- **The API gaining streaming**, or a batch endpoint that takes many states. That would
  make [D1](design.md#d1-a-blocking-http-client-and-no-async-runtime) and
  [D2](design.md#d2-one-request-per-invocation) worth revisiting together.
- **A published tokenizer**, or a documented token estimate. That would allow the one
  documented limit that is currently unchecked to be checked.
- **A documented error-body schema.** The error path currently surfaces the body verbatim
  because its shape is unspecified; a schema would let the `422` name the offending field
  the way the local validator already does.
- **A second primitive, or a new question type.** The `Question` enum would gain a variant,
  the shorthand would gain a subcommand, and the answer validation would gain a case —
  all three are closed matches, so the compiler lists the work.

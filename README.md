# decide

**Ask Jev for a typed decision, and get JSON a shell script can branch on.**

> **Status: implemented.** The five gates in [`AGENTS.md`](AGENTS.md#commands) are green and
> the suite runs with no API key. The examples below are the behaviour
> [`docs/cli.md`](docs/cli.md) specifies and `tests/` asserts.

Jev is [TypeSafe](https://docs.typesafe.ai)'s System One model. You give it a state and a
set of typed questions; it gives back typed answers and probability distributions, in one
request, with no text to parse. `decide` is the smallest useful way to call it from a
script.

```console
$ decide noul "does this message convey urgency?" --state-file ticket.txt --value
0.95

$ decide ask questions.json --state-file ticket.txt --select answers.department.choice
technical
```

## Install

```sh
cargo build --release          # target/release/decide
```

Latest stable Rust, pinned in [`rust-toolchain.toml`](rust-toolchain.toml). There is no
runtime beside the binary, no configuration file, and nothing to install first.

The credential comes from the environment, never from a flag — `argv` is readable by every
process on the machine, and it is kept in shell history and in agent transcripts:

```sh
export TYPESAFE_API_KEY=…                         # or, for a mounted secret:
export TYPESAFE_API_KEY_FILE=/run/secrets/jev     # the file is read only if the
                                                  # variable above is unset or empty
```

`TYPESAFE_BASE_URL` overrides the API root, and `TYPESAFE_DEFAULT_MODEL` names the model
when neither `--model` nor the request document does.

## The two shapes

**One question, from the command line**, when the answer is a string, an option, or a
level:

```console
$ decide choice "which team should handle this?" --state-file ticket.txt --id department \
      --option "billing=payments and invoices" \
      --option "technical=bugs and outages" \
      --option sales --value
technical
```

**Many questions, from a document**, which is what the API is built for — every question
is evaluated in parallel against the same state, and adding one costs only its own tokens:

```console
$ cat questions.json
{
  "questions": {
    "is_urgent":   { "type": "noul",   "instructions": "Does this convey urgency?" },
    "department":  { "type": "choice", "instructions": "Which team should handle this?",
                     "criteria": { "billing": "payments", "technical": "bugs", "sales": "pricing" } },
    "frustration": { "type": "score",  "instructions": "How frustrated is the customer?",
                     "criteria": ["calm", "frustrated but civil", "very angry"] }
  }
}

$ cat ticket.txt | decide ask questions.json --pretty
{
  "answers": {
    "department":  { "choice": "technical", "confidence": 0.78,
                     "probabilities": { "billing": 0.15, "sales": 0.0, "technical": 0.85 },
                     "type": "choice" },
    "frustration": { "confidence": 1.0, "legend": { "0": "calm", "1": "frustrated but civil", "2": "very angry" },
                     "probabilities": { "0": 0.0, "1": 1.0, "2": 0.0 }, "score": 1.0, "type": "score" },
    "is_urgent":   { "noul": 1.0, "type": "noul" }
  },
  "model": "jev-1.13.0",
  "usage": { "input_tokens": 392, "output_tokens": 65 }
}
```

The document is the API's own request body, so one written for `curl` works unchanged, and
one written here can be sent with `curl -d @…`. It is parsed **strictly**: a misspelled
`criteria` is refused by name, before anything is sent, because a rubric that quietly
became no rubric still comes back with an answer.

## The contract

- **stdout is the response and nothing else.** One JSON value, written once, after the work
  is done. A failed run leaves stdout empty.
- **stderr is progress and errors.** `--verbose` is the only thing that adds to it, and it
  never changes a byte of stdout.
- **The exit code says whether the command worked, not what the answer was.** `0` completed,
  `1` the evaluation failed, `2` the invocation is wrong. The answer does not encode a
  threshold, because the threshold is policy and belongs in your script.
- **Nothing is sent that a local check could have refused.** Every documented limit, every
  misspelled key, and every conflicting flag is caught before a connection is opened, and
  the error names the question and the flag it is about.
- **A `429` is retried**, with the same policy the official SDKs use — `408`, `429`, and
  `5xx`, two retries, 500 ms doubling to 5 s, `Retry-After` honoured — and bounded by
  `--retries` and `--timeout`, so the worst case is `(1 + retries) × timeout`.
- **`--dry-run` prints the request and calls nothing**, so a request can be reviewed before
  it is paid for. It needs no credential.

### Reading one value

| Flag | Prints |
|---|---|
| `--select PATH` | a dotted path from the response root — `--select usage.input_tokens`, `--select models.0.name`. Works on every subcommand. |
| `--value` | the single answer's own value, for a request that asked exactly one question |
| `--field PATH` | a dotted path from that answer — `--field confidence`, `--field probabilities.billing`, `--field legend.2` |

A string prints raw, with no JSON quoting, so `$(…)` yields the value itself. A number
prints as JSON prints it, so `1.0` stays `1.0` and compares with `awk`, `bc`, or `jq -e` —
rounding an answer would be changing it.

```sh
p=$(decide noul "does this message convey urgency?" --state-file ticket.txt --value)
if awk -v p="$p" 'BEGIN { exit !(p > 0.8) }'; then
  page_oncall
fi
```

## Where the state comes from

Exactly one of these supplies it, and two named sources are an error that names both:

| Source | Supplies |
|---|---|
| `--state TEXT` | `TEXT` as a string — or as JSON with `--state-json` |
| `--state-file PATH` | the contents of `PATH` (`-` means stdin) |
| stdin | used when nothing else is named and stdin is not a terminal |
| `--no-state` | an explicit `null`, for a self-contained question |
| the `state` key of a document passed to `ask` | whatever JSON value is there, including `null` |

A text state with no non-whitespace character is refused — that is what a pipeline that
delivered nothing looks like — and `--state-json` refuses a number or a boolean by name,
because the API accepts a string, an object, or an array.

## What it does not do

- **No threshold, weighting, or combining of answers.** A threshold hidden inside a CLI is
  a default nobody chose; it belongs in your script, where a reviewer can find it.
- **No batching over states.** The API batches *questions*; forty documents are forty
  invocations, and `xargs -P4` is already the scheduler for that.
- **No configuration file.** Every setting is a flag, and the script is the file that
  records how it is called.
- **No `--api-key`.** See the credential section above.
- **No streaming.** The API does not stream, so there is nothing to stream.

[`docs/roadmap.md`](docs/roadmap.md) has the full list, each with the reason it is out and
what would have to change for it to come back.

## Why not `curl` and `jq`

`curl -H "Authorization: Bearer $TYPESAFE_API_KEY" -d @questions.json … | jq -r …` is a
perfectly good pipeline, and it is what most people will reach for first. `decide` is for
the second week, when the pipeline is in a cron job: it refuses a misspelled `criteria` by
name instead of getting an answer to a question nobody asked, it validates the documented
limits before spending a request, it retries a rate limit without a shell loop, and it puts
one value on stdout with no quoting to strip. It also gives an agent a `--help` that
describes a correct call, which a `curl` invocation does not.

## Development

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --all-features        # no API key and no network are needed
cargo test --doc
cargo build --release
```

All five must pass. The suite binds sockets on `127.0.0.1` and answers them from a
hand-rolled server, so the real client is exercised against real bytes without reaching the
network; the fixtures in [`tests/data/`](tests/data) are the request and response bodies
copied from the documentation.

One test does call the real API, and it is ignored by default:

```sh
TYPESAFE_API_KEY=… cargo nextest run --run-ignored ignored-only
```

It is the only check that the client agrees with the API itself rather than with our
reading of the documentation, which is why it is kept and why it is not required.

## Documentation

| Page | What it holds |
|---|---|
| [`docs/cli.md`](docs/cli.md) | Every flag, every rule, every exit code, and the help text |
| [`docs/design.md`](docs/design.md) | The decisions, each with the alternative that was rejected |
| [`docs/api.md`](docs/api.md) | The Jev API as this tool depends on it, with sources |
| [`docs/testing.md`](docs/testing.md) | What is asserted, in both directions, and how |
| [`docs/roadmap.md`](docs/roadmap.md) | What is in v1, what is out, and what would change the shape |
| [`AGENTS.md`](AGENTS.md) | How to work on this repository: layout, gates, and conventions |

## Licence

MIT.

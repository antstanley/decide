# The command line

This is the contract an implementer follows and a caller relies on. It is written so that
`decide --help` and this page cannot disagree: [Help text](#help-text) below is the text
`cli.rs` must produce.

The decisions behind these rules are in [`design.md`](design.md); the API facts they rest
on are in [`api.md`](api.md).

## Synopsis

```
decide [GLOBAL] ask    [FILE]     [EVAL]
decide [GLOBAL] noul   <INSTRUCTION> [EVAL] --id NAME [--yes DESC] [--no DESC]
decide [GLOBAL] choice <INSTRUCTION> [EVAL] --id NAME --option NAME[=DESC]…
decide [GLOBAL] score  <INSTRUCTION> [EVAL] --id NAME --level DESC…
decide [GLOBAL] models
decide [GLOBAL] auth   set | unset | status
```

A bare `decide`, or `decide` with an unrecognised argument, prints the help on stderr and
exits `2`. There is no default subcommand: guessing which of "ask one question" and "read
a file" was meant is a guess with a wrong answer half the time.

`[GLOBAL]` and `[EVAL]` are two named flag sets, and `[EVAL]` may be interleaved with the
subcommand's own flags in any order. Both are defined under [Flags](#flags).

## Where the state comes from

`state` may be a string, a JSON object, an array, or `null`. Exactly one of these supplies
it:

| Source | Supplies |
|---|---|
| `--state TEXT` | `TEXT` as a string — or as JSON with `--state-json` |
| `--state-file PATH` | the contents of `PATH` (`-` means stdin) |
| stdin | used when no other source is named and stdin is not a terminal |
| `--no-state` | an explicit `null` |
| the `state` key of a document passed to `ask` | whatever JSON value is there, including `null` |

Rules, all of which are enforced before anything is sent:

- **Naming two sources is a usage error** that names both. This includes stdin: a piped
  document and a `state` in the request document are two sources.
- **Naming none, with a terminal on stdin, is a usage error** naming `--state`,
  `--state-file`, and `--no-state`. It is never a wait.
- **A text state with no non-whitespace character is a usage error.** This is the
  "the pipeline delivered nothing" case. `--no-state` is how to ask for no state at all.
- `--state-json` says the state is JSON; it may accompany `--state`, `--state-file`, or
  stdin. It is meaningless with `--no-state` or with a document that carries a `state`,
  and both are usage errors.
- With `--state-json`, the value must be that JSON — a string, an object, or an array. A
  number or a boolean is refused by name ("the state is a number; the API accepts a string,
  an object, or an array"), because the API accepts neither.
- A file that cannot be read is a usage error naming the path and the OS error.
- A document sent to `ask` **without** a `state` key falls through to this table, so
  `decide ask questions.json --state-file ticket.txt` and `cat ticket.txt | decide ask
  questions.json` both work with the same reusable question file.

## The request document

`ask` reads one JSON object. It is the API's own request shape
([`api.md`](api.md#the-request-body)), so a document written for `curl` works here
unchanged, and one written here can be sent with `curl -d @…`.

```json
{
  "state": "Help! My payouts have been failing for 3 days.",
  "model": "jev-latest",
  "questions": {
    "is_urgent": {
      "type": "noul",
      "instructions": "Does this convey urgency?"
    },
    "department": {
      "type": "choice",
      "instructions": "Which team should handle this?",
      "criteria": {
        "billing": "Payments, invoicing, refunds",
        "technical": "Bugs, outages, integrations"
      }
    },
    "frustration": {
      "type": "score",
      "instructions": "How frustrated is the customer?",
      "criteria": ["Calm", "Frustrated", "Very angry"]
    }
  }
}
```

| Key | Required | Notes |
|---|---|---|
| `questions` | yes | A non-empty object. Each value is one of the three question types. |
| `state` | no | A string, object, array, or `null`. Omitting it falls through to the table above. |
| `model` | no | Overridden by `--model`, which is a knob (see [D17](design.md#d17-a-model-is-a-knob-a-state-and-a-question-are-meaning)). |

**The document is parsed strictly.** A key that is not in this table — at the top level or
inside a question — is refused, with the key and its location named. The reason is in
[D8](design.md#d8-the-response-is-parsed-tolerantly-the-document-strictly): a misspelled
`criteria` turns a rubric into no rubric, and the answer still comes back.

Validation, all of it local, all of it exit `2`:

| Rule | Where it comes from |
|---|---|
| At least one question. | The API's SDK types a request's questions as non-empty. |
| A `choice` has at least one option; at most 255. | [`api.md`](api.md#limits) |
| A `score` has at least two levels and at most ten. | [`api.md`](api.md#limits) |
| `type` is one of `noul`, `choice`, `score`. | [`api.md`](api.md#question-types) |
| A noul's `criteria` has only the keys `true` and `false`. | [`api.md`](api.md#question-types) |
| Question ids are non-empty. | A `""` id cannot be addressed by `--select`. |
| A string instruction is non-empty. | A question with no instruction asks nothing. |

Every message names the question id it is about — `question "department": a choice has at
most 255 options (256 were given)` — because a document with a dozen questions otherwise
gives a caller no way to find the one that is wrong.

## Output

### The default

The response body, as one JSON value, on stdout, followed by a newline. `--pretty` indents
it instead. Keys are sorted in everything this program writes or sends — the response, a
selected value, and a request body — so `--dry-run` prints byte-for-byte what a real call
would send. The output is the same whether or not stdout is a terminal
([D4](design.md#d4-compact-json-by-default---pretty-opts-in)).

Nothing else is ever written to stdout, and on failure stdout stays empty
([D3](design.md#d3-stdout-is-the-apis-response-and-nothing-else)).

```console
$ decide noul "does this message convey urgency?" --state-file ticket.txt
{"answers":{"answer":{"noul":0.95,"type":"noul"}},"model":"jev-1.13.0","usage":{"input_tokens":296,"output_tokens":20}}
```

### `--select PATH` — one value, from anywhere in the response

A dotted path from the response root, resolved against the response of whichever
subcommand ran — so it reaches `models` as well as `answers`. A segment walks into an
object as a key or into an array as an index: on an object a numeric segment *is* the key,
which is how `--field legend.2` finds the level numbered `2`, and on an array it is the
index, which is how `--select models.0.name` finds the first model. An out-of-range index
does not resolve, and neither does a path through anything that is not an object or an
array.

```console
$ decide --select model noul "…" --state-file t.txt
jev-1.13.0
$ decide --select usage.input_tokens noul "…" --state-file t.txt
296
$ decide models --select models.0.name
jev-latest
```

- A **string** is printed raw: no quotes, no escaping, then a newline. The reason to select
  one value is to use it, and `$(…)` on a quoted string would carry the quotes.
- A **number** is printed as JSON prints it, so `1.0` stays `1.0` rather than becoming `1`.
  Comparing it in shell needs `awk`, `bc`, or `jq -e`; `decide` does not round on the
  caller's behalf, because rounding an answer is changing it.
- **`true`, `false`, and `null`** are printed literally.
- An **object or array** is printed as JSON, indented if `--pretty` is given.
- A path that does not resolve is exit `2`: the path, the segment that failed, and the keys
  available at that point are named. `--select answers.urgency.noul` when the answer is
  keyed `is_urgent` prints the ids it did find.

### `--value` and `--field PATH` — the one answer, without knowing its name

Both apply when the request asked **exactly one question**, and both are refused when it
asked any other number, naming the count and suggesting `--select` — "the value" of three
answers is not defined.

- **`--value`** prints that answer's own value: `noul`, `choice`, or `score`, whichever the
  primitive has. It is the flag a script uses, and it is why the answer's id and field
  name never appear in the script.
- **`--field PATH`** prints a dotted path from the answer, for the rest of it:
  `--field confidence` and `--field probabilities.billing` on a choice, `--field legend.2`
  on a score.
- The rules for what a scalar looks like are `--select`'s.

They are two flags rather than one flag with an optional value, because an optional value
in an argument parser is either greedy — `--value something` swallowing the next token —
or forced into `--value=something` form. A bare flag and a valued flag say the same thing
without either problem.

```console
$ decide choice "which team should handle this?" --state-file ticket.txt \
      --option billing="invoices, refunds" --option technical="bugs, outages" --value
technical

$ decide choice "…" --state-file ticket.txt --option billing --option technical \
      --field confidence
0.78
```

`--value`, `--field`, and `--select` conflict with one another — each asks for a different
thing to be printed — and all three conflict with `--dry-run`, which prints a request
rather than a response.

### `--dry-run`

Prints the request body that would be sent — re-serialised, so the resolved model and the
canonical key order are what you see, and byte-identical to what a real call would put on
the wire — writes nothing to stderr, needs no credential, and exits `0`.

```console
$ decide choice "which team should handle this?" --no-state \
      --option billing="invoices" --option technical="bugs" --dry-run
{"model":"jev-latest","questions":{"answer":{"criteria":{"billing":"invoices","technical":"bugs"},"instructions":"which team should handle this?","type":"choice"}},"state":null}
```

### Errors on stderr

Every error is one message on stderr, prefixed with `decide: `. It names the thing that is
wrong — the flag, the question id, the key, the path, the status, or the path on disk — so
that a caller which cannot read this source can still act on it. Nothing is wrapped, no
backtrace is printed, and the usage text is *not* dumped after a message from `decide`
itself; the usage text belongs to a caller who got the invocation wrong, and that caller
gets it from the argument parser.

## Subcommands

### `ask [FILE]`

Evaluates the request document in `FILE`, or on stdin when `FILE` is `-` or absent.

With no `FILE` and a terminal on stdin, this is a usage error that says to name a file or
pipe one in. This is the batching form the API recommends, and it is the only form that
can express structured instructions or criteria
([D6](design.md#d6-two-forms-for-the-questions-flags-for-one-a-document-for-many)).

`--state`, `--state-file`, `--state-json`, `--no-state`, and `--model` may accompany it
and fill in what the document leaves out; a `state` the document already carries is a
conflict, while `--model` simply wins.

### `noul <INSTRUCTION>`

Asks a yes/no question and prints the probability that the answer is yes, from 0 (no) to 1
(yes). A noul has no `confidence` ([`api.md`](api.md#the-response-body)) — the probability
*is* the answer, so a confidence-gated rule over one has to threshold this number itself.

| Flag | Meaning |
|---|---|
| `--id NAME` | The question id. Default `answer`. |
| `--yes DESC` | What a yes means. Sent as `criteria.true`. |
| `--no DESC` | What a no means. Sent as `criteria.false`. |

```console
$ decide noul "does this message convey urgency?" --state-file ticket.txt \
      --yes "explicitly time-sensitive" --no "no urgency expressed" --value
0.95
```

### `choice <INSTRUCTION>`

Picks one option from a set.

| Flag | Meaning |
|---|---|
| `--id NAME` | The question id. Default `answer`. |
| `--option NAME[=DESC]` | An option, repeatable, at least one. |

`--option` splits on the **first** `=`, so a description may contain one. `--option other`
sends `null` as that option's criteria — the API's way of saying "this option needs no
extra detail" — and `--option other=` sends an empty string, which is not the same thing.
Two options with the same name are a usage error: they are keys in a map, and a JSON
object cannot hold the same key twice without one of them being silently lost. `--option`
is not `required` in the argument parser's sense, so that the refusal a caller sees is
`decide`'s, which counts what was given:

```console
$ decide choice "which team should handle this?" --no-state
decide: question "answer": a choice needs at least one option (none were given)
$ decide choice "which team should handle this?" --no-state \
      --option billing=payments --option billing=invoices
decide: the option "billing" was given twice; a choice's options are keys, and a key
        cannot be repeated
```

The happy path, with a description for two options and none for the third:

```console
$ decide choice "which team should handle this?" --state-file ticket.txt --id department \
      --option "billing=payments and invoices" \
      --option "technical=bugs and outages" \
      --option sales --value
technical
```

### `score <INSTRUCTION>`

Rates the state along ordered levels.

| Flag | Meaning |
|---|---|
| `--id NAME` | The question id. Default `answer`. |
| `--level DESC` | A level, repeatable, in ascending order. Two to ten. |

The order of `--level` is the order of the levels; there is no other way to say which end
is which. `--level` is repeatable and the *count* is validated before the request — the
floor and the ceiling are both the API's knowledge, so neither is left to the argument
parser ([`design.md`](design.md#d9-the-limits-are-enforced-before-the-call)):

```console
$ decide score "how frustrated is the customer?" --state-file ticket.txt
decide: question "answer": a score needs at least 2 levels (0 were given)
$ decide score "how frustrated is the customer?" --state-file ticket.txt \
      --level a --level b --level c --level d --level e \
      --level f --level g --level h --level i --level j --level k
decide: question "answer": a score has at most 10 levels (11 were given)
```

A three-level score, printing the value:

```console
$ decide score "how frustrated is the customer?" --state-file ticket.txt --id frustration \
      --level calm --level "frustrated but civil" --level "very angry" --value
1.05
```

### `models`

`GET {base_url}/v1/models`, printing the response ([`api.md`](api.md#models)). It takes the
global flags and none of the evaluation flags — there is no model to choose, no state to
send, and no body for `--dry-run` to show.

```console
$ decide models --select models.0.name
jev-latest
```

### `auth`

Keeps the credential, so that a shell profile does not have to hold it. It takes the global
flags and none of the evaluation flags: there is no state to send and no body to show.

| Action | Does |
|---|---|
| `auth set` | asks for a token when a person is running it, reads a pipe when a script is, and stores it readable only by its owner |
| `auth status` | names the source that supplies the credential — never the value — and asks the API whether it accepts the key |
| `auth status --no-check` | names the source and stops there: no network, no valid key needed |
| `auth unset` | removes the stored token |

```console
$ decide auth set
TYPESAFE API token (it will not be echoed): 
stored the token in "/home/you/.config/decide/api-key"

$ printf %s "$TOKEN" | decide auth set          # a script, which has no terminal
stored the token in "/home/you/.config/decide/api-key"

$ decide auth status
TYPESAFE_API_KEY: the API accepted the key

$ decide auth status --no-check          # the local half of the question
TYPESAFE_API_KEY
```

The token is typed at a prompt, which does not echo it, or read from a **pipe** when stdin
is not a terminal — so a script or a CI runner can still supply one, and an interactive
call leaves nothing in the shell's history. It is never a flag, for the reason there is no
`--api-key` ([D5](design.md#d5-the-credential-never-comes-from-argv)): `argv` is readable by
every process on the machine. The prompt is written to stderr, and the newline that a
terminal would have echoed after the return key is written there too, so the next line of
output starts where it should.

`set` and `unset` confirm on stderr and write nothing to stdout, because they are actions;
`status` prints its one line on stdout, because it is a question.

`status` asks both halves of it: where the credential comes from, and whether the API
accepts it. The second half is one `GET /v1/models` with the credential that was found —
the same bearer token, and unlike an evaluation it spends no tokens, so it is cheap enough
to run after every `auth set`. `--timeout`, `--retries`, and `--backoff-ms` apply to it as
they do to any call, and `--verbose` shows it.

The verdict goes on stdout after the source, and the exit code is the call's:

| Outcome | Code | Where |
|---|---|---|
| the API accepted the key | `0` | stdout: the source, then `the API accepted the key` |
| the API refused it | `1` | stderr: the `401`, its reason phrase, and its body, as any other call reports one |
| the call could not be made | `1` | stderr: the transport failure and the attempt count, or the status that came back — including a `200` that is not the API's answer |
| no credential is configured | `2` | stderr: the three ways to supply one, and the path the store would use |

so the script is the one a caller already knows:

```sh
decide auth status || exit 1
```

`--no-check` drops the call and answers the local half only. It needs no network and no
valid key, which is the question to ask when the API itself is what is in doubt — and the
one `status` asked before it learned to dial. An unreadable `--api-key-file` is still
reported as the error it is rather than as a missing credential.

## Flags

The set is split by one question: does the flag apply to *every* subcommand's response?
`--select` does, so it is global. `--value` and `--field` need an answer, and `--dry-run`
needs a request body; `models` has neither, so they are defined only where they mean
something. A flag that could not do anything is not accepted and quietly ignored — it is
not accepted at all, which is the same rule the state follows.

### Global

| Flag | Default | Environment | Notes |
|---|---|---|---|
| `--base-url URL` | `https://api.typesafe.ai/v1/systemone` | `TYPESAFE_BASE_URL` | Must begin with `http://` or `https://`. Both the API root and the full `/v1/systemone` endpoint are accepted — the default is the endpoint, because that is the URL the API documentation shows — and either reduces to the root, so `/v1/systemone` is never appended twice and `GET /v1/models` stays a sibling of the `POST`. A trailing `/` is trimmed. |
| `--api-key-file PATH` | — | `TYPESAFE_API_KEY_FILE` | The key, read from a file. One trailing newline is stripped, so a file written by `echo` works. |
| `--timeout SECONDS` | `60` | — | Bounds **one attempt**. Must be at least 1; `0` is refused rather than meaning "forever". The worst case for a call is `(1 + --retries) × --timeout`. |
| `--retries N` | `2` | — | At most 10. `--retries 0` makes the call a single attempt. |
| `--backoff-ms MS` | `500` | — | The delay before the first retry; it doubles per attempt, is capped at 5 s, carries up to 25% jitter, and yields to a `Retry-After` header of up to 60 s. |
| `--select PATH` | — | — | A path from the response root, for any subcommand. See above. |
| `--pretty` | compact | — | Indents the JSON wherever it is printed. |
| `--verbose` | quiet | — | Progress on stderr: the method and URL, each retry with its reason and delay, and the outcome with its status, model, and token usage. Never on stdout. |

### Evaluation flags

Accepted by `ask`, `noul`, `choice`, and `score`, and by none of `models`:

| Flag | Notes |
|---|---|
| `--state TEXT`, `--state-file PATH`, `--state-json`, `--no-state` | See [Where the state comes from](#where-the-state-comes-from). |
| `--model NAME` | Overrides a `model` in the document. Default: `TYPESAFE_DEFAULT_MODEL`, then `jev-latest`. |
| `--dry-run` | Print the request body, make no call, need no credential. |
| `--value` | Print the one answer's own value. |
| `--field PATH` | Print a dotted path from the one answer. |

`--model` is here rather than with the global knobs because it is a field of the body that
is sent, and `models` sends no body to name a model in.

## Environment

| Variable | Used for |
|---|---|
| `TYPESAFE_API_KEY` | The credential. Required for every subcommand except `--dry-run` and `--dry-run`'s siblings in `auth`. |
| `TYPESAFE_API_KEY_FILE` | A file holding the credential, for a secret mounted where `argv` and `env` cannot carry one. It is read only when `TYPESAFE_API_KEY` is unset or empty, so a key in the environment is never a silent fallback for a file that cannot be read. |
| `TYPESAFE_BASE_URL` | The API root, or the `/v1/systemone` endpoint in full. Overridden by `--base-url`. |
| `TYPESAFE_DEFAULT_MODEL` | The model when neither `--model` nor the document names one. Default `jev-latest`. |

### Where the credential comes from

Three sources, in this order, and the first one that supplies a key wins:

| Order | Source |
|---|---|
| 1 | `TYPESAFE_API_KEY` |
| 2 | `--api-key-file PATH`, or `TYPESAFE_API_KEY_FILE` |
| 3 | the store `decide auth set` writes: `$XDG_CONFIG_HOME/decide/api-key`, or `~/.config/decide/api-key` |

The environment is first so that a script, a CI runner, or a colleague's debugging session
can override what is stored without having to unset anything first. The named file is
second because naming a file is a decision, and the store is a *default* rather than a
decision. A named file that cannot be read is an error rather than a silent fall through to
the store: the caller said where the key is, and it is not there.

There is no `--api-key`
([D5](design.md#d5-the-credential-never-comes-from-argv)). A missing
credential is exit `2` and is reported before any connection is opened, so a script learns
about it in milliseconds rather than after a timeout.

`TYPESAFE_LOG_LEVEL` is deliberately not read
([D13](design.md#d13-one-credential-store-and-still-no-configuration-file)).

## Exit codes

| Code | Meaning |
|---|---|
| `0` | The evaluation completed and its result is on stdout. The *answer* is not encoded here. |
| `1` | The evaluation was attempted and did not complete. |
| `2` | The invocation is wrong, and nothing was attempted. |

`--help` and `--version` exit `0` and print to stdout. Any other code — `134` from the
release profile's `panic = "abort"` in particular — means the process died, which is a bug
in `decide` and not a result to branch on.

## Help text

`cli.rs` must produce this, from `about`, `long_about`, and `after_help` rather than from a
literal string, so that the flag list stays generated from the definitions:

```text
Ask Jev for a typed decision, and get JSON a shell script can branch on.

Usage: decide [OPTIONS] <COMMAND>

Commands:
  ask     Evaluate the request document in a file, or on stdin
  noul    Ask a yes/no question; print the probability that the answer is yes
  choice  Ask for one option out of a set you define
  score   Ask for a position along levels you define
  models  List the models this account can send
  auth    Store the API token, so that an environment variable is not needed
  help    Print this message or the help of the given subcommand(s)

Options:
      --base-url <URL>        The API endpoint, or the root it lives under [env: TYPESAFE_BASE_URL]
                             [default: https://api.typesafe.ai/v1/systemone]
      --api-key-file <PATH>   Read the API key from a file [env: TYPESAFE_API_KEY_FILE]
      --timeout <SECONDS>     Give up on one attempt after this long [default: 60]
      --retries <N>           Retry a failed attempt this many times, at most 10 [default: 2]
      --backoff-ms <MS>       Delay before the first retry; doubles up to 5000 [default: 500]
      --select <PATH>         Print one value from the response instead of the response
      --pretty                Indent the JSON output
      --verbose               Report progress on stderr
  -h, --help                  Print help
  -V, --version               Print version

Examples:
  Ask several questions about one document, in a single call:
      decide ask questions.json --state-file ticket.txt

  The same, with the state piped in:
      cat ticket.txt | decide ask questions.json

  Ask one question, and print just its value:
      decide noul "does this message convey urgency?" --state-file ticket.txt --value

  Choose an option, and print the option's name:
      decide choice "which team should handle this?" --state-file ticket.txt \
        --option "billing=payments and invoices" \
        --option "technical=bugs and outages" \
        --id department --value

  Print the request that would be sent, without calling the API:
      decide score "how frustrated is the customer?" --state-file ticket.txt \
        --level calm --level "frustrated but civil" --level "very angry" --dry-run

  Ask about a document that arrived on stdin, with no state in the questions file:
      cat ticket.txt | decide ask questions.json --select answers.department.choice

  Store the API token; it is typed at a prompt, and never echoed:
      decide auth set

The API key is read from TYPESAFE_API_KEY, from a file named by --api-key-file or
TYPESAFE_API_KEY_FILE, or from the store that `decide auth set` writes. It is never read
from a flag: a flag is visible to every process on the machine, and it is kept in the
shell's history.

stdout carries the response and nothing else. Progress and errors go to stderr.
Exit codes: 0 the evaluation completed, 1 it failed, 2 the invocation is wrong.
```

Every subcommand also has a `long_about` with one worked example. The evaluation flags and
each primitive's own flags are listed in the *subcommand's* help rather than here —
`decide noul --help` shows `--state`, `--model`, `--value`, `--field`, `--dry-run`, `--yes`,
and `--no` — so a caller that has found the subcommand it wants never has to go back to the
top level for a flag it needs. clap generates that list from the definitions, which is what
keeps it from drifting away from the flags that exist.

## Worked examples

The three things a caller actually does.

**Ask several questions in one call, then read the answers.** One request, one state, three
judgments, evaluated in parallel:

```sh
decide ask questions.json --state-file ticket.txt > answers.json
urgency=$(jq -r '.answers.is_urgent.noul' answers.json)
dept=$(jq -r '.answers.department.choice' answers.json)
```

**Branch on a yes/no without `jq`.** A noul is a probability, so the comparison is a
float one — and the threshold lives here, in the script, where a reviewer can find it:

```sh
p=$(decide noul "does this message convey urgency?" --state-file ticket.txt --value)
if awk -v p="$p" 'BEGIN { exit !(p > 0.8) }'; then
  page_oncall
fi
```

**Gate on confidence as well as the answer.** `confidence` answers "how sure", which is a
different question from "what" — so it is asked in the same request and read separately:

```sh
decide ask questions.json --state-file ticket.txt > answers.json
if jq -e '.answers.department.confidence < 0.6' answers.json >/dev/null; then
  route_to_human answers.json      # low confidence: a person decides
else
  route_to "$(jq -r '.answers.department.choice' answers.json)"
fi
```

Every one of these is a plain pipeline, because stdout is the response and nothing else.

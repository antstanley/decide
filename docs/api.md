# The Jev API, as `decide` depends on it

This page is **the facts**, not the choices. Every claim here is traceable to
`https://docs.typesafe.ai` — the pages listed under [Provenance](#provenance) — and the
implementation is held to this page. What `decide` *does* about these facts is
[`design.md`](design.md); what its own interface looks like is [`cli.md`](cli.md).

Nothing here is inferred. Where the documentation is silent, this page says so rather
than filling the gap, because the gaps are exactly where an implementation is free to
choose — and a choice needs to be visible to be reviewed.

## The endpoint

```http
POST {base_url}/v1/systemone
Authorization: Bearer <API_KEY>
Content-Type: application/json
```

- `base_url` defaults to `https://api.typesafe.ai`. The SDKs read it from
  `TYPESAFE_BASE_URL`. `decide` accepts either that root or the endpoint in full
  (`https://api.typesafe.ai/v1/systemone`) in the same setting, and reduces both to the
  root the two calls are built from.
- The path is `/v1/systemone`. There is no other evaluation endpoint.
- There is **no streaming**. One request produces one complete response body. This is
  the single most load-bearing fact for `decide`: it means an ordinary blocking HTTP
  call is the whole transport, and no async runtime is needed to be faithful.
- Models are listed by `GET {base_url}/v1/models`.

## The request body

| Field | Type | Required | Meaning |
|---|---|---|---|
| `state` | string \| object \| array \| null | yes | The content to evaluate. One state per request. |
| `model` | string | yes | The model or alias that handles the call. |
| `questions` | map<string, Question> | yes | Named typed questions. The keys are the caller's; answers come back under the same keys. |

```json
{
  "state": "Help! My payouts have been failing for 3 days.",
  "model": "jev-latest",
  "questions": {
    "is_urgent": {
      "type": "noul",
      "instructions": "Does this convey urgency?"
    }
  }
}
```

Question ids are **not sent to the model** and are not used in inference. They are a
naming convention between the caller and the response, so an id may be any string the
caller likes.

`state` may be `null` — the JavaScript SDK types it as text, an object, an array, or
`null` "to evaluate". A question that is self-contained ("is this statement true?")
does not need a state.

### Question types

All three share `type` and `instructions`. Each adds its own `criteria`.

| `type` | Goal | `criteria` | Limit |
|---|---|---|---|
| `noul` | Is the statement true? | optional `{ "true": …, "false": … }` | — |
| `choice` | Pick one option from a set | required `map<string, string\|object\|array\|null>` | at most 255 options |
| `score` | Rate along ordered levels | required `array<string\|object\|array>` | at least 2, at most 10 levels |

```json
{
  "urgency":      { "type": "noul",   "instructions": "Does this convey urgency?",
                    "criteria": { "true": "Explicitly time-sensitive",
                                  "false": "No urgency expressed" } },
  "department":   { "type": "choice", "instructions": "Which team should handle this?",
                    "criteria": { "billing": "Payments, invoicing, refunds",
                                  "technical": "Bugs, outages, integrations",
                                  "sales": "Pricing, upgrades, new accounts" } },
  "frustration":  { "type": "score",  "instructions": "How frustrated is the customer?",
                    "criteria": ["Calm", "Frustrated", "Very angry"] }
}
```

`instructions` may be a string, an object, or an array. Structured instructions carry
the question in one field and any data it refers to in others, referring to that data by
name in backticks:

```json
"instructions": {
  "potential_duplicate": { "name": "John Smith", "location": "Oakland, California" },
  "question": "Is the resume for the same person as `potential_duplicate`?"
}
```

`criteria` values may likewise be an object or an array wherever they are described as
`string | object | array` above. A `choice` option with `null` criteria means "this
option needs no extra detail".

The documentation does **not** state a maximum number of questions per request. The
limit that does bind is the context budget (below).

## The response body

| Field | Type | Meaning |
|---|---|---|
| `model` | string | The **versioned** id that answered — `jev-1.13.0`, not `jev-latest`. |
| `answers` | map<string, Answer> | One answer per question, under the ids that were sent. |
| `usage` | `{ input_tokens: integer, output_tokens: integer }` | Token accounting for the request. |

Answer shapes, one variant per question type:

```json
{
  "model": "jev-1.13.0",
  "answers": {
    "is_urgent":   { "type": "noul",   "noul": 0.95 },
    "department":  { "type": "choice", "choice": "billing",
                     "probabilities": { "billing": 0.88, "technical": 0.12, "sales": 0.0 },
                     "confidence": 0.81 },
    "frustration": { "type": "score",  "score": 1.05,
                     "legend": { "0": "Calm", "1": "Frustrated", "2": "Very angry" },
                     "probabilities": { "0": 0.0, "1": 0.95, "2": 0.05 },
                     "confidence": 0.92 }
  },
  "usage": { "input_tokens": 296, "output_tokens": 20 }
}
```

Points that matter to a caller:

- **`noul` has no `confidence`.** Only `choice` and `score` do. A confidence-gated rule
  over a yes/no judgment has to threshold the `noul` value itself, or ask a `choice`
  question instead.
- **`choice.probabilities` keys are the options the caller defined**; they sum to 1.
  The chosen option is the highest-probability one.
- **`score.score` is probability-weighted and may land between levels** (the example
  above is 1.05 with a legend of 0..2). `score.legend` maps the string level numbers
  back to the descriptions the caller supplied. `score.probabilities` is keyed by
  level number **as a string**.
- **`model` reports the versioned id.** An alias in the request resolves to a version
  in the response.

## Errors

| Status | Meaning |
|---|---|
| `401 Unauthorized` | Missing or invalid API key. |
| `422 Unprocessable Entity` | The body failed validation; the body details the offending field. |
| `429 Too Many Requests` | Rate limit exceeded. |
| `529 Overloaded` | TypeSafe is temporarily overloaded. |

The documentation specifies the *statuses* and their meanings but **not the JSON shape
of the error body**. A caller therefore cannot model it field by field; it can only
surface it. `decide` reports the status and the body verbatim, capped in length (see
[`design.md`](design.md#d8-the-response-is-parsed-tolerantly-the-document-strictly)).

## Retries

The documentation's instruction is "retry with exponential backoff instead of retrying
immediately" for `429` and `529`. The SDKs implement a concrete policy, and the
JavaScript reference documents it field by field:

| Setting | Default |
|---|---|
| Retry these statuses | `408`, `429`, `500`–`599` |
| Maximum retries after the first attempt | 2 |
| First backoff delay | 500 ms |
| Backoff growth | doubles, capped at 5 s |
| Jitter | 25% of each delay, subtracted |
| `Retry-After` / `retry-after-ms` | honoured, up to 60 s |
| Connection and timeout failures | retried |

This is the policy `decide` mirrors, because a shell script has no retry loop of its own
and a `429` at 02:00 in a cron job should not be a failed run.

## Models

```http
GET {base_url}/v1/models
Authorization: Bearer <API_KEY>
```

```json
{ "models": [ { "name": "jev-latest", "description": "…", "release_date": "…" } ] }
```

The list contains the aliases. Versioned ids such as `jev-1.13.0` are accepted by the
`model` field whether or not they appear in the list.

| Alias | Points to | Meaning |
|---|---|---|
| `jev-latest` | `jev-1.13.0` | The most recent stable release. The SDKs' default. |
| `jev-preview` | `jev-1.13.0` | The most recent release, official or not. |

An alias moves when a new release ships, so answers can change without a change on the
caller's side. Pinning a versioned id is how a caller controls that.

## Limits

| Limit | Value |
|---|---|
| Context per request | 64k tokens: `state` plus all questions combined |
| Per-question context | 32k tokens: `state` plus the single longest question |
| `choice` options | at most 255 |
| `score` levels | at least 2, at most 10 |
| Input | Text only. No image, audio, or video. |
| Language | English is the primary training language; other languages are accepted with lower accuracy. |
| Rate limits | 250,000 tokens/second, 1,200 requests/minute — **adjusting dynamically**, so a `429` is expected rather than exceptional. |

`decide` cannot check the token budget: that needs a tokenizer for the model's own
vocabulary, and the documentation does not publish one. Everything else in this table is
checked locally (see [`design.md`](design.md#d9-the-limits-are-enforced-before-the-call)).

## Environment variables

These are the names the official SDKs use, and `decide` adopts them rather than inventing
its own:

| Variable | Used for | Default |
|---|---|---|
| `TYPESAFE_API_KEY` | The credential. | none — required |
| `TYPESAFE_BASE_URL` | The API root, or the endpoint in full. | `https://api.typesafe.ai` |
| `TYPESAFE_DEFAULT_MODEL` | The model when the request does not name one. | `jev-latest` |
| `TYPESAFE_LOG_LEVEL` | The SDKs' own logging. | not used by `decide` |

`TYPESAFE_LOG_LEVEL` is deliberately unused: it configures a *library's* logger, and
`decide` is a program whose diagnostics are a flag (`--verbose`) written to stderr. Two
ways to ask for the same thing, one of which the program cannot honour faithfully, is
worse than one.

## Provenance

Fetched on 2026-09-20, in preference to reading the SDKs' source:

- `https://docs.typesafe.ai/introduction.md`
- `https://docs.typesafe.ai/introduction/quickstart.md`
- `https://docs.typesafe.ai/api.md` — the HTTP reference, the source of the request and
  response shapes and the error table
- `https://docs.typesafe.ai/models.md` — aliases, limits, data handling
- `https://docs.typesafe.ai/concepts/state.md` — what a state may be
- `https://docs.typesafe.ai/primitives.md`, `…/primitives/choice.md`,
  `…/primitives/score.md`, `…/primitives/noul.md`, `…/primitives/advanced.md` —
  question structure and the option and level limits
- `https://docs.typesafe.ai/confidence.md` — what `confidence` is and is not
- `https://docs.typesafe.ai/sdk/javascript/api/interfaces/TypeSafeClientConfig.md`,
  `…/RetryPolicy.md`, `…/SystemOneRequest.md`, `…/SystemOneResult.md`,
  `…/variables/ENV.md` — the environment variable names and the concrete retry policy
- `https://docs.typesafe.ai/agent-skill.md` and the TypeSafe
  [agent skill](https://github.com/typesafe-ai/skills) — how the vendor expects an agent
  to call this API, which is one of `decide`'s two callers

The two example response bodies in this page are the documentation's own, reproduced so
they can be used verbatim as test fixtures
([`testing.md`](testing.md#fixtures-that-nobody-here-wrote)).

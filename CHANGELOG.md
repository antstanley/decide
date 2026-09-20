# Changelog

Every release is prepared from the change files in `.changeset/`, and this file is what they
produce — see [`docs/release.md`](docs/release.md).

## [0.1.0] - 2026-09-20

### Added

- `decide ask`, `noul`, `choice`, and `score`: one state and a set of typed questions, with
  the answer as JSON on stdout and nothing else.
- `decide models`, and `--select`, `--value`, and `--field` to print one value out of a
  response without reaching for `jq`.
- Local validation of every documented limit before a request is made, an error naming the
  question, the flag, or the path it is about.
- Retries with the SDK's policy, bounded by `--retries`, `--timeout`, and `--backoff-ms`.
- `--dry-run`, which prints the request and calls nothing.
- `decide auth set`, which asks for the token without echoing it, and `decide auth status`,
  which says where the credential comes from and whether the API accepts it.

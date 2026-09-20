# Changesets

A changeset is a note about a change, written when the change is made rather than when it is
released, so that the release notes are not reconstructed from memory months later. The
format is the one [Knope's `changesets`
crate](https://github.com/knope-dev/changesets) parses, which is a Rust implementation of
[the original idea](https://github.com/changesets/changesets).

## Adding one

Create a file in this directory named after the change — `token-store.md`, not
`fixed-a-bug.md` — and write:

```markdown
---
decide: minor
---

`decide auth set` stores the token, so an environment variable is not needed.
```

The rules, from the crate's own definition, are few and strict:

1. The first line is `---` on its own.
2. Then one `package: change type` pair per line. The package is `decide` — the only one in
   this repository — and the type is `major`, `minor`, or `patch`. Any other type is treated
   as `patch` for versioning, which is what the crate does.
3. Then a second `---` on its own.
4. Then the summary, in Markdown. It becomes the changelog entry, verbatim.

The highest type across all pending changes decides the bump. `scripts/changesets.sh` is
what applies them; [`docs/release.md`](../docs/release.md) is the process around it.

A change file is deleted by the release that consumes it, so what is in this directory is
always the set of changes that have not been released yet.

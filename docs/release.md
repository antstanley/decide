# Releasing

A release is a change like any other: it is reviewed, and it says what changed and why. The
mechanism is [changesets](https://github.com/knope-dev/changesets) — a change file written
when the change is made, rather than release notes reconstructed from memory afterwards.

## Making a change

Add a file to `.changeset/` in the same pull request as the change:

```markdown
---
decide: minor
---

`decide auth set` stores the token, so an environment variable is not needed.
```

The package is always `decide`, the type is `major`, `minor`, or `patch`, and the summary is
what the changelog will say, verbatim. [`.changeset/README.md`](../.changeset/README.md) has
the four rules in full. A change that ships without one ships without release notes.

## What the workflow does

`.github/workflows/release.yml` runs on every push to `main`, and which half of it runs is
decided by the tree rather than by a label:

| What is in the tree | What happens |
|---|---|
| change files in `.changeset/` | they are applied — `Cargo.toml`, `Cargo.lock`, and `CHANGELOG.md` — and a `release: vX.Y.Z` pull request is opened |
| none, and the version has a release | nothing |
| none, and the version has no release | the five targets are built, the tag is created, and a GitHub release is published with the archives, their checksums, and `install.sh` |

So the loop is: land a change with its change file → review the release pull request it
produces → merge that → the binaries appear. Merging the release pull request is the moment
the version is frozen, which is why it is a pull request.

## The targets

| Asset | Built on |
|---|---|
| `decide-aarch64-apple-darwin.tar.gz` | `macos-14` |
| `decide-aarch64-unknown-linux-musl.tar.gz` | `ubuntu-24.04-arm` |
| `decide-x86_64-unknown-linux-musl.tar.gz` | `ubuntu-latest` |
| `decide-aarch64-pc-windows-msvc.tar.gz` | `windows-11-arm` |
| `decide-x86_64-pc-windows-msvc.tar.gz` | `windows-latest` |

Each is built natively — `rustup target add` and `cargo build --release --locked --target` —
so nothing in the matrix crosses a compiler. The Linux builds are `musl`, which needs no
glibc and no system library beyond the kernel: that is what makes one archive run on every
distribution, and it is why the installer can promise a build rather than a range of them.

The asset name carries no version and the directory inside the archive does. That is
deliberate: it lets the installer fetch
`releases/latest/download/decide-<target>.tar.gz` without asking the API what the latest
release is — no rate limit, and no JSON parsed in shell — while an extracted copy still says
which version it is.

## Doing it by hand

The workflow is a convenience over these three commands, which need nothing but a shell:

```sh
scripts/changesets.sh apply                 # bump, changelog, consume the change files
git commit -am "release: v$(scripts/changesets.sh current)"
scripts/package.sh aarch64-apple-darwin     # dist/decide-<target>.tar.gz and its checksum
gh release create v0.2.0 --title v0.2.0 \
    --notes-file <(scripts/changesets.sh notes 0.2.0) dist/* install.sh
```

## The installer

`install.sh` is served from the repository root and attached to every release, so a pinned
version can install itself without running a script from a branch that has moved on. It
detects the host, downloads, and verifies the SHA-256 **before** anything is installed; a
missing checksum is a refusal, not a shrug. It refuses platforms it has no build for, and it
never writes to stdout — progress belongs on stderr, where a `curl | sh` caller can read it.

#!/bin/sh
# Versioning from the changesets convention (see .changeset/README.md).
#
#   changesets.sh pending      the change files that have not been released, one per line
#   changesets.sh next         the version those changes would produce
#   changesets.sh apply        consume them: bump Cargo.toml and Cargo.lock, write the
#                              changelog, delete the files, and print the new version
#   changesets.sh current      the version the manifest names now
#   changesets.sh notes V      print the changelog section for V, for a release body
#
# Exit codes: 0 the question was answered (including "nothing pending"), 1 nothing to do
# where something was required, 2 a change file is malformed or names another package.
#
# `apply` is meant to run once per batch, on a clean tree: it rewrites the version, so a
# second run without new change files would bump again. The release workflow checks
# `pending` first for exactly that reason.
#
# It is deliberately a shell script and not a dependency: the format is four rules long, and
# a release nobody can cut without a toolchain is a release nobody can cut at 3am.
#
# Two shell hazards shaped this file, both found by running it rather than reading it:
#
#   1. bash 3.2 — `/bin/sh` on macOS, and on GitHub's macOS runners — cannot parse a `case`
#      inside a command substitution. `$(f)` where f contains the case parses; `$(case …)`
#      does not. So every `case` here lives in a function, and every capture is `$(f)`.
#   2. `exit` inside a command substitution ends the *subshell*, not the script. A function
#      whose errors are meant to stop everything is therefore called once, at the top level,
#      as a validation gate — never only from inside a capture.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

package=decide
changes_dir=.changeset
manifest=Cargo.toml
changelog=CHANGELOG.md

fail() {
    echo "changesets: $*" >&2
    exit 2
}

# The change files waiting to be released: `.md`, minus this directory's own README.
pending_files() {
    for file in "$changes_dir"/*.md; do
        [ -e "$file" ] || continue
        case $(basename "$file") in
            README.md) continue ;;
        esac
        printf '%s\n' "$file"
    done
}

# The change types one file declares for this package, one per line.
types_in() {
    file=$1
    [ "$(sed -n '1p' "$file")" = '---' ] ||
        fail "$file: the first line of a change file must be ---"
    # Diagnostics go to stderr: the caller captures this function's stdout.
    awk -v file="$file" -v package="$package" '
        NR == 1 { next }
        /^---$/ { closed = 1; exit }
        {
            colon = index($0, ":")
            if (colon == 0) {
                printf "changesets: %s: line %d is not a \"package: type\" pair\n", file, NR > "/dev/stderr"
                bad = 1
                exit 1
            }
            name = substr($0, 1, colon - 1)
            type = substr($0, colon + 1)
            gsub(/^[ \t]+|[ \t]+$/, "", name)
            gsub(/^[ \t]+|[ \t]+$/, "", type)
            if (name != package) {
                printf "changesets: %s: \"%s\" is not a package here, and the only one is \"%s\"\n", file, name, package > "/dev/stderr"
                bad = 1
                exit 1
            }
            if (type == "") {
                printf "changesets: %s: \"%s\" has no change type\n", file, name > "/dev/stderr"
                bad = 1
                exit 1
            }
            print type
        }
        END {
            if (!closed && !bad) {
                printf "changesets: %s: the frontmatter is not closed with ---\n", file > "/dev/stderr"
                exit 1
            }
        }
    ' "$file" ||
        exit 2
}

# Refuse the whole changeset before anything is computed or written.
validate() {
    for file in $(pending_files); do
        types_in "$file" >/dev/null || exit 2
    done
}

# The type a declared type means for versioning: anything unknown is a patch.
effective_type() {
    case $1 in
        major) echo major ;;
        minor) echo minor ;;
        *) echo patch ;;
    esac
}

# A change type as a changelog heading.
heading_of() {
    case $1 in
        major) echo Major ;;
        minor) echo Minor ;;
        *) echo Patch ;;
    esac
}

# The bump the whole changeset asks for: the highest type any file declares.
pending_bump() {
    bump='patch'
    for file in $(pending_files); do
        types=$(types_in "$file") || exit 2
        for type in $types; do
            case $type in
                major)
                    echo major
                    return 0
                    ;;
                minor) bump='minor' ;;
            esac
        done
    done
    echo "$bump"
}

current_version() {
    sed -n 's/^version = "\(.*\)"$/\1/p' "$manifest" | head -1
}

# The next version: the current one with the bump applied.
next_version() {
    [ -n "$(pending_files)" ] || return 1
    current=$(current_version)
    major=${current%%.*}
    rest=${current#*.}
    minor=${rest%%.*}
    patch=${rest#*.}
    case $current in
        *[!0-9.]* | *..* | .* | *.) fail "$manifest: \"$current\" is not a version this script understands" ;;
    esac
    [ -n "$major" ] && [ -n "$minor" ] && [ -n "$patch" ] ||
        fail "$manifest: \"$current\" is not a version this script understands"
    case $(pending_bump) in
        major) printf '%s.0.0\n' "$((major + 1))" ;;
        minor) printf '%s.%s.0\n' "$major" "$((minor + 1))" ;;
        *) printf '%s.%s.%s\n' "$major" "$minor" "$((patch + 1))" ;;
    esac
}

# The summaries of one type's changes, as changelog list items.
summaries_of() {
    wanted=$1
    for file in $(pending_files); do
        types=$(types_in "$file") || exit 2
        for type in $types; do
            [ "$(effective_type "$type")" = "$wanted" ] || continue
            awk 'NR == 1 { next } /^---$/ { body = 1; next } body { print }' "$file" |
                sed -e 's/[[:space:]]*$//' -e '/./,$!d' |
                sed -e '1s/^/- /' -e '2,$s/^/  /' |
                sed -e 's/[[:space:]]*$//'
        done
    done
}

# The changelog entry: one section per type, each change's summary as a list item.
changelog_entry() {
    version=$1
    printf '## [%s] - %s\n\n' "$version" "$(date -u +%Y-%m-%d)"
    for wanted in major minor patch; do
        entries=$(summaries_of "$wanted")
        [ -n "$entries" ] || continue
        printf '### %s\n\n%s\n\n' "$(heading_of "$wanted")" "$entries"
    done
}

# The existing changelog without its title, so a new entry goes under the title.
changelog_body() {
    awk 'NR == 1 && $0 == "# Changelog" { next } NR == 2 && $0 == "" { next } { print }' "$changelog"
}

# Everything is written to temporary files and moved into place, so a failure anywhere in
# the computation cannot leave the manifest and the changelog disagreeing.
apply_changeset() {
    version=$(next_version) || { echo "changesets: nothing to release" >&2; exit 1; }
    current=$(current_version)

    sed "s/^version = \"$current\"$/version = \"$version\"/" "$manifest" >"$manifest.new" ||
        fail "$manifest: could not be rewritten"
    {
        printf '# Changelog\n\n'
        changelog_entry "$version"
        changelog_body
    } >"$changelog.new"

    mv "$manifest.new" "$manifest"
    mv "$changelog.new" "$changelog"

    # The lock is refreshed before the change files go, so a failure here leaves the
    # changeset intact for a retry.
    cargo update --workspace --quiet ||
        fail "could not refresh Cargo.lock; the manifest is bumped, so check it by hand"
    for file in $(pending_files); do rm -f "$file"; done
    echo "$version"
}

case ${1:-} in
    pending)
        validate
        pending_files
        ;;
    next)
        validate
        next_version || {
            echo "changesets: nothing to release" >&2
            exit 1
        }
        ;;
    apply)
        validate
        apply_changeset
        ;;
    current)
        current_version
        ;;
    notes)
        [ $# -eq 2 ] || fail "usage: changesets.sh notes VERSION"
        awk -v version="$2" '
            $0 ~ "^## \\[" version "\\]" { found = 1 }
            found && NR > 1 && $0 ~ "^## \\[" && $0 !~ "\\[" version "\\]" { exit }
            found { print }
        ' "$changelog"
        ;;
    *)
        fail "usage: changesets.sh pending|next|current|apply|notes VERSION"
        ;;
esac

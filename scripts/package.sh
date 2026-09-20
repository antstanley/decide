#!/bin/sh
# Build one target and lay out the release asset for it.
#
#   scripts/package.sh <rustup-target>
#
# Produces, in `dist/`:
#   decide-<target>.tar.gz          the archive, holding decide-<version>-<target>/decide
#   decide-<target>.tar.gz.sha256   its checksum, in `sha256sum -c` form
#
# One archive format for every target, Windows included: `tar` ships with Windows 10+, one
# format is one code path, and one code path is the one that gets tested.
#
# The asset name carries no version, and the directory inside it does. That is deliberate:
# it lets `https://github.com/…/releases/latest/download/decide-<target>.tar.gz` resolve
# without asking the API what the latest release is, which is a rate limit and a JSON parser
# an installer should not need — while the extracted copy still says which version it is.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

[ $# -eq 1 ] || {
    echo "usage: package.sh <rustup-target>" >&2
    exit 2
}

target=$1
version=$(scripts/changesets.sh current)
stage_name="decide-$version-$target"
stage="dist/$stage_name"
binary=target/$target/release/decide

case $target in
    *windows*) binary=$binary.exe ;;
esac

echo "packaging decide $version for $target" >&2
cargo build --release --locked --target "$target"

rm -rf "$stage"
mkdir -p "$stage"
cp "$binary" "$stage/"
cp LICENSE "$stage/"
cp README.md "$stage/"

rm -f "dist/decide-$target.tar.gz" "dist/decide-$target.tar.gz.sha256" \
    "dist/decide-$target.zip" "dist/decide-$target.zip.sha256"

tar -czf "dist/decide-$target.tar.gz" -C dist "$stage_name"
asset="dist/decide-$target.tar.gz"

if command -v sha256sum >/dev/null 2>&1; then
    (cd dist && sha256sum "$(basename "$asset")" >"$(basename "$asset").sha256")
else
    (cd dist && shasum -a 256 "$(basename "$asset")" >"$(basename "$asset").sha256")
fi

rm -rf "$stage"
echo "$asset" >&2
ls -l "$asset" "$asset.sha256" >&2

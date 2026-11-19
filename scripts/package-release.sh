#!/usr/bin/env bash
set -euo pipefail

usage() {
  printf 'Usage: bash scripts/package-release.sh <rust-target> <vMAJOR.MINOR.PATCH>\n' >&2
  exit 2
}

[[ $# -eq 2 ]] || usage
target=$1
version=$2
[[ $target =~ ^[a-zA-Z0-9_]+(-[a-zA-Z0-9_]+)+$ ]] || usage
[[ $version =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || usage

cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
binary=fanqie
if [[ $target == *-windows-* ]]; then
  binary=fanqie.exe
fi
source_binary="target/$target/release/$binary"
if [[ ! -f $source_binary ]]; then
  printf 'Missing %s; run cargo build --locked --release --target %s --target-dir target first.\n' \
    "$source_binary" "$target" >&2
  exit 1
fi

name="fanqie-$version-$target"
output=target/release-assets
mkdir -p -- "$output"
stage=$(mktemp -d "$output/.package.XXXXXXXX")
trap 'rm -rf -- "$stage"' EXIT
mkdir -- "$stage/$name"
cp -p -- "$source_binary" "$stage/$name/$binary"
cp -p -- README.md LICENSE "$stage/$name/"
cp -R -- licenses "$stage/$name/licenses"
tar -czf "$stage/$name.tar.gz" -C "$stage" "$name"
mv -f -- "$stage/$name.tar.gz" "$output/$name.tar.gz"
printf '%s\n' "$output/$name.tar.gz"

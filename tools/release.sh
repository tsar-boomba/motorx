#!/bin/sh
set -e

USAGE="Usage: \n\t./tools/release.sh <patch|minor|major>"

if [[ "$1" != "patch" && "$1" != "minor" && "$1" != "major" ]]; then
	echo "$USAGE"
	exit 1
fi

cargo fmt --check
cargo build -p motorx
cargo build -p motorx-core
cargo nextest run --workspace
cargo release --workspace --allow-branch main --all-features --tag-name 'v{{version}}' -v -x $1

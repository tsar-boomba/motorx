#!/bin/sh
cargo release --workspace --allow-branch main --all-features --tag-name 'v{{version}}' -v -x $1

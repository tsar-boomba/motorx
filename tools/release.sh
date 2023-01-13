#!/bin/sh
cargo release --workspace --allow-branch main --tag-name 'v{{version}}' --no-publish -v -x $1

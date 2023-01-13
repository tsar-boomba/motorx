#!/bin/sh
cargo release --allow-branch main --tag-name 'v{{version}}' --no-publish -v -x $1

#!/bin/sh
RUSTFLAGS='--cfg reqwest_unstable' cargo test -p motorx-core -F tls,h3,prometheus
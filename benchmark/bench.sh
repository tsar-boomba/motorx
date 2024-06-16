#!/bin/bash

# $1 -> Path to benchmark (not required)

cargo build --release
cd echo-server && cargo build --release && cd ..
./echo-server/target/release/echo-server 127.0.0.1:2999 &
./target/release/motorx &
sleep 1
PROXY_RPS=`oha -z 10s -j --no-tui http://127.0.0.1:4000$1 | jq '.rps.mean'`

# kill proxy process
kill -15 %2

RAW_RPS=`oha -z 10s -j --no-tui http://127.0.0.1:2999 | jq '.rps.mean'`

# kill echo process
kill -15 %1

echo "Requests/sec proxy: $PROXY_RPS"
echo "Requests/sec raw  : $RAW_RPS"

#!/bin/bash

# Requires `docker` & `jq` to be in $PATH
# runs motorx in docker and stores the requests per second

./benchmark/start_or_run.sh motorx-bench
MOTORX_RPS=`oha -z 10s -j --no-tui http://127.0.0.1:80 | jq '.rps.mean'`
echo "Stopping motorx container..."
docker stop motorx-bench

./benchmark/start_or_run.sh nginx-bench
NGINX_RPS=`oha -z 10s -j --no-tui http://127.0.0.1:80 | jq '.rps.mean'`
echo "Stopping nginx container..."
docker stop nginx-bench

echo -e "\nMotorx requests/sec: $MOTORX_RPS"
echo "Nginx requests/sec: $NGINX_RPS"

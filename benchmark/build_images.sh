#!/bin/bash
# Should be run from root of the project
docker build . -f ./benchmark/nginx.dockerfile -t nginx-bench &
docker build . -f ./benchmark/motorx.dockerfile -t motorx-bench &
wait
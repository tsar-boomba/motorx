#!/bin/bash
"$@" &
./echo-server 127.0.0.1:3000 &
wait -n
exit $?

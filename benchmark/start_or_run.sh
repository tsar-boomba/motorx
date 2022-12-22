#!/bin/bash

# Searches for $1 in containers, if found, starts it
# otherwise will try and use `docker run` to start a detached container with that name

echo "Searching for $1"

docker container ls -a | grep $1

if [ $? = "0" ]
then
	docker start $1
	echo "Started existing $1"
else
	docker run -d -p 80:80 --name $1 $1
	echo "Started new container $1"
fi
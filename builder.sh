#!/bin/bash

# generate a docker image to compile for EL7

docker build -t rust:el7 -f Dockerfile.el7

#!/bin/bash
echo "Running on host!"
echo "User: $(whoami)"
echo "PWD: $PWD"
# Keep running so locald doesn't restart it immediately
sleep 10

#!/bin/sh
trap 'echo "Caught SIGINT"; exit 0' INT
trap 'echo "Caught SIGTERM"; exit 0' TERM

echo "Waiting for signal..."
while true; do
    sleep 1
done

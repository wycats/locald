#!/bin/bash
echo "Starting dummy server on port $PORT"
python3 -m http.server $PORT

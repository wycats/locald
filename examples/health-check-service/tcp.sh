#!/bin/bash
echo "Starting tcp-app on port $PORT..."
# Simple HTTP server using python to ensure port is open and stays open
python3 -m http.server $PORT

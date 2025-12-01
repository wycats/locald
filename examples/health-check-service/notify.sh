#!/bin/bash
echo "Starting notify-app..."
sleep 2
if [ -z "$NOTIFY_SOCKET" ]; then
  echo "NOTIFY_SOCKET is not set!"
  exit 1
fi
echo "Sending READY=1 to $NOTIFY_SOCKET"
# Use python to send datagram if nc is tricky with abstract sockets or whatever
python3 -c "import socket, os; s = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM); s.connect(os.environ['NOTIFY_SOCKET']); s.send(b'READY=1')"
# Keep running
sleep 1000

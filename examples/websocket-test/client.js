const WebSocket = require('ws');

const ws = new WebSocket('wss://websocket-test.localhost', {
  rejectUnauthorized: false
});

ws.on('open', function open() {
  console.log('connected');
  ws.send('hello');
});

ws.on('message', function incoming(data) {
  console.log('received: %s', data);
  process.exit(0);
});

ws.on('error', function error(err) {
  console.error('error:', err);
  process.exit(1);
});

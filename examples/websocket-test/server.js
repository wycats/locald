const WebSocket = require('ws');
const http = require('http');

const port = process.env.PORT || 8080;
const server = http.createServer((req, res) => {
  res.writeHead(200);
  res.end('WebSocket Echo Server');
});

const wss = new WebSocket.Server({ server });

wss.on('connection', (ws) => {
  console.log('Client connected');
  ws.on('message', (message) => {
    console.log(`Received: ${message}`);
    ws.send(message);
  });
});

server.listen(port, () => {
  console.log(`Server listening on port ${port}`);
});

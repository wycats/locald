import http.server
import socketserver
import os
import json

PORT = int(os.environ.get("PORT", 4444))

class Handler(http.server.SimpleHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header('Content-type', 'application/json')
        self.end_headers()
        response = {
            "service": "hydra",
            "status": "ok",
            "db_url": os.environ.get("DATABASE_URL", "not_set")
        }
        self.wfile.write(json.dumps(response).encode('utf-8'))

with socketserver.TCPServer(("", PORT), Handler) as httpd:
    print(f"Serving Hydra on port {PORT}")
    httpd.serve_forever()

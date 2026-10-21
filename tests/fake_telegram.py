#!/usr/bin/env python3
"""A fake Telegram Bot API for tests/telegram.sh: records every call, serves queued updates.

POST /control/update {...}  queues an update (its update_id is assigned)
GET  /control/calls         every Bot API call so far: {"method", "body", "message_id"?}
A token of "bad" gets 401 on every method.
"""
import json
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

lock = threading.Lock()
updates, calls = [], []
counters = {"update": 1, "message": 100}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def reply(self, code, obj):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def body(self):
        n = int(self.headers.get("Content-Length") or 0)
        return json.loads(self.rfile.read(n) or b"{}")

    def do_GET(self):
        if self.path == "/control/calls":
            with lock:
                return self.reply(200, calls)
        self.reply(404, {})

    def do_POST(self):
        b = self.body()
        if self.path == "/control/update":
            with lock:
                b["update_id"] = counters["update"]
                counters["update"] += 1
                updates.append(b)
            return self.reply(200, {"ok": True})
        parts = self.path.split("/")
        token, method = parts[1][len("bot"):], parts[2]
        if token == "bad":
            return self.reply(401, {"ok": False, "error_code": 401, "description": "Unauthorized"})
        call = {"method": method, "body": b}
        with lock:
            calls.append(call)
        if method == "getMe":
            return self.reply(200, {"ok": True, "result": {"username": "fake_bot"}})
        if method == "getUpdates":
            deadline = time.time() + min(b.get("timeout", 0), 2)
            while True:
                with lock:
                    ready = [u for u in updates if u["update_id"] >= b.get("offset", 0)]
                if ready or time.time() >= deadline:
                    return self.reply(200, {"ok": True, "result": ready})
                time.sleep(0.05)
        if method == "sendMessage":
            with lock:
                mid = counters["message"]
                counters["message"] += 1
                call["message_id"] = mid
            return self.reply(200, {"ok": True, "result": {"message_id": mid}})
        self.reply(200, {"ok": True, "result": True})


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
print(server.server_address[1], flush=True)
server.serve_forever()

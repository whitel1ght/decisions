#!/usr/bin/env python3
"""Speak MCP to `BIN mcp` over stdio: initialize, then one tools/call; print the result text.

usage: mcp_client.py BIN TOOL JSON_ARGUMENTS
"""
import json
import subprocess
import sys


def main():
    binary, tool, args = sys.argv[1], sys.argv[2], json.loads(sys.argv[3])
    p = subprocess.Popen([binary, "mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)

    def send(msg):
        p.stdin.write(json.dumps(msg) + "\n")
        p.stdin.flush()

    def response(want):
        while True:
            line = p.stdout.readline()
            if not line:
                sys.exit("server closed")
            msg = json.loads(line)
            if msg.get("id") == want:
                return msg

    send({"jsonrpc": "2.0", "id": 1, "method": "initialize",
          "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "e2e-client", "version": "1"}}})
    response(1)
    send({"jsonrpc": "2.0", "method": "notifications/initialized"})
    send({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": tool, "arguments": args}})
    print(response(2)["result"]["content"][0]["text"])
    p.stdin.close()
    p.wait(timeout=10)


main()

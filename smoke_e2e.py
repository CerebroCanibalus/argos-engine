# mypy: ignore-errors
# ruff: noqa - ad-hoc MCP smoke script, exempt from package lint/type rules
"""End-to-end smoke: real network search through the keyless fanout.

Run manually (needs internet): python smoke_e2e.py
"""
import subprocess
import json
import sys


proc = subprocess.Popen(
    [r"target\release\argos-engine.exe"],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True,
    encoding="utf-8",
)


def send_msg(msg):
    proc.stdin.write(json.dumps(msg) + "\n")
    proc.stdin.flush()
    if "id" not in msg:
        return None
    return json.loads(proc.stdout.readline())


send_msg({
    "jsonrpc": "2.0", "id": 1, "method": "initialize",
    "params": {"protocolVersion": "2024-11-05", "capabilities": {},
               "clientInfo": {"name": "smoke-e2e", "version": "1.0"}},
})
send_msg({"jsonrpc": "2.0", "method": "notifications/initialized"})

# 1. status: providers + reachability
status = send_msg({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                   "params": {"name": "status", "arguments": {}}})
print("1. status ->", json.dumps(status)[:600])

# 2. real search through the fanout
search = send_msg({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                   "params": {"name": "search",
                              "arguments": {"query": "rust programming language", "limit": 8}}})
text = json.dumps(search)
print("2. search ->", text[:1500])

if '"isError": true' in text:
    print("E2E FAILED: search returned an error")
    proc.terminate()
    sys.exit(1)

# 3. summarize engines/failover visibility
try:
    payload = json.loads(search["result"]["content"][0]["text"])
    engines = {}
    for row in payload:
        engines[row.get("engine", "?")] = engines.get(row.get("engine", "?"), 0) + 1
    print(f"3. results={len(payload)} engines={engines}")
except Exception as ex:
    print(f"3. could not summarize: {ex}")

print("E2E OK")
proc.terminate()

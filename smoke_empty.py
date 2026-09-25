# mypy: ignore-errors
# ruff: noqa - ad-hoc probe, exempt from package lint
"""Probe: does argos.search return [] for any query, and if so, do providers error?"""
import subprocess, json, sys

proc = subprocess.Popen(
    [r"target\release\argos-engine.exe"],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True,
    encoding="utf-8",
)


def send(msg):
    proc.stdin.write(json.dumps(msg) + "\n")
    proc.stdin.flush()
    if "id" not in msg:
        return None
    return json.loads(proc.stdout.readline())


send({"jsonrpc": "2.0", "id": 1, "method": "initialize",
      "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                 "clientInfo": {"name": "probe-empty", "version": "1"}}})
send({"jsonrpc": "2.0", "method": "notifications/initialized"})

cases = [
    "rust programming language",                          # typical - should yield
    "qzkjqzkjqzkj asdfasdf",                             # gibberish - empty?
    "AI",                                                 # common short
    "x",                                                  # 1 char
    "",                                                   # empty (rejected)
    "   ",                                                # whitespace (rejected)
]

for q in cases:
    r = send({"jsonrpc": "2.0", "id": 99, "method": "tools/call",
              "params": {"name": "search", "arguments": {"query": q, "limit": 5}}})
    if r and "result" in r:
        content = r["result"]["content"][0]["text"]
        parsed = json.loads(content) if content.startswith("[") else content
        n = len(parsed) if isinstance(parsed, list) else "ERR"
        engines = {}
        if isinstance(parsed, list):
            for item in parsed:
                engines[item.get("engine", "?")] = engines.get(item.get("engine", "?"), 0) + 1
        is_err = r["result"].get("isError", False)
        print(f"q={q!r:50} -> n={n} err={is_err} engines={engines}")
    else:
        print(f"q={q!r:50} -> RAW: {json.dumps(r)[:200]}")

proc.terminate()
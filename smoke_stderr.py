# mypy: ignore-errors
# ruff: noqa - ad-hoc probe, exempt from package lint
"""Probe with stderr capture: which providers actually contribute vs error out?"""
import subprocess, json

proc = subprocess.Popen(
    [r"target\release\argos-engine.exe"],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    text=True, encoding="utf-8",
)


def send(msg):
    proc.stdin.write(json.dumps(msg) + "\n"); proc.stdin.flush()
    return json.loads(proc.stdout.readline()) if "id" in msg else None


send({"jsonrpc": "2.0", "id": 1, "method": "initialize",
      "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                 "clientInfo": {"name": "probe", "version": "1"}}})
send({"jsonrpc": "2.0", "method": "notifications/initialized"})

# Run 3 different queries to see per-provider behavior across the fanout
for q in ["rust async tokio tutorial", "kubernetes networking guide", "rust borrow checker"]:
    r = send({"jsonrpc": "2.0", "id": 99, "method": "tools/call",
              "params": {"name": "search", "arguments": {"query": q, "limit": 6}}})
    if r and "result" in r:
        items = json.loads(r["result"]["content"][0]["text"])
        engines = {}
        for item in items: engines[item.get("engine", "?")] = engines.get(item.get("engine", "?"), 0) + 1
        print(f"q={q!r:35} -> n={len(items):2} engines={engines}")
    elif r and "error" in r:
        print(f"q={q!r:35} -> ERROR: {r['error']['message']}")

# Now: status shows transport reachability (true even on captcha pages)
s = send({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
          "params": {"name": "status", "arguments": {}}})
status = json.loads(s["result"]["content"][0]["text"])
print("\nstatus:", json.dumps(status["providers"], indent=2))

stderr_out = proc.stderr.read()
if stderr_out.strip():
    print("\n--- STDERR ---")
    print(stderr_out[:2000])
else:
    print("\n--- STDERR: empty (providers log nothing on failure) ---")

proc.terminate()
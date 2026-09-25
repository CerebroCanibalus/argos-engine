# mypy: ignore-errors
# ruff: noqa - ad-hoc smoke
"""Verify v0.2.0 features on the fresh .exe:
- SearchOutcome shape (results / providers / warnings)
- compact source per hit
- domains parameter (site: scoping appended to q)
"""
import subprocess, json, os

def call(proc, msg):
    proc.stdin.write(json.dumps(msg) + "\n"); proc.stdin.flush()
    return json.loads(proc.stdout.readline()) if "id" in msg else None

exe = r"target\release\argos-engine.exe"
print("bin:", os.path.getsize(exe), "bytes\n")

# --- 1. SearchOutcome shape ---
proc = subprocess.Popen([exe], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                       text=True, encoding="utf-8")
call(proc, {"jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"protocolVersion":"2024-11-05","capabilities":{},
                     "clientInfo":{"name":"smoke","version":"1"}}})
call(proc, {"jsonrpc":"2.0","method":"notifications/initialized"})

r = call(proc, {"jsonrpc":"2.0","id":2,"method":"tools/call",
                "params":{"name":"search","arguments":{
                    "query":"Spitfire LABS review","limit":6}}})
text = r["result"]["content"][0]["text"]
out = json.loads(text)
print("1. plain search:")
print(f"   results={len(out['results'])}  providers={len(out['providers'])}  warnings={len(out['warnings'])}")
for prov in out["providers"]:
    print(f"     - {prov['name']:11} kind={prov['status'].get('kind')!r:18} "
          + (f"count={prov['status']['count']}" if prov["status"].get("kind") == "ok" else
             f"status={prov['status'].get('status')}" if "status" in prov["status"] else ""))
for w in out["warnings"]:
    print(f"   warn: {w}")
print("   sources:", [x["source"] for x in out["results"][:4]])
proc.terminate(); proc.wait(timeout=3)

# --- 2. domains scoping ---
print()
proc = subprocess.Popen([exe], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                       text=True, encoding="utf-8")
call(proc, {"jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"protocolVersion":"2024-11-05","capabilities":{},
                     "clientInfo":{"name":"smoke","version":"1"}}})
call(proc, {"jsonrpc":"2.0","method":"notifications/initialized"})

r = call(proc, {"jsonrpc":"2.0","id":2,"method":"tools/call",
                "params":{"name":"search","arguments":{
                    "query":"Spitfire LABS review","limit":8,
                    "domains":["kvrforums.com","reddit.com"]}}})
if "error" in r:
    print("2. domain-scoped ERROR:", r["error"])
else:
    out = json.loads(r["result"]["content"][0]["text"])
    print(f"2. domain-scoped (kvrforums.com OR reddit.com):")
    print(f"   results={len(out['results'])}  providers={[(p['name'],p['status']) for p in out['providers']]}")
    print(f"   sources: {[x['source'] for x in out['results'][:8]]}")
proc.terminate(); proc.wait(timeout=3)
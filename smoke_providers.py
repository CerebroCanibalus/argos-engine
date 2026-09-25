# mypy: ignore-errors
# ruff: noqa - ad-hoc probe
"""Verify each provider in isolation with explicit env override."""
import subprocess, json, os, sys

for prov in ["duckduckgo", "bing", "brave"]:
    env = os.environ.copy()
    env["ARGOS_PROVIDERS"] = prov
    try:
        p = subprocess.Popen(
            [r"target\release\argos-engine.exe"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, encoding="utf-8", env=env,
        )
        def s(m):
            p.stdin.write(json.dumps(m) + "\n"); p.stdin.flush()
            return json.loads(p.stdout.readline()) if "id" in m else None
        s({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2024-11-05","capabilities":{},
            "clientInfo":{"name":"p","version":"1"}}})
        s({"jsonrpc":"2.0","method":"notifications/initialized"})
        r = s({"jsonrpc":"2.0","id":3,"method":"tools/call",
               "params":{"name":"search","arguments":{
                   "query":"rust tokio tutorial","limit":4}}})
        if r and "result" in r:
            txt = r["result"]["content"][0]["text"]
            if txt.startswith("["):
                items = json.loads(txt)
                eng = {}
                for x in items: eng[x.get("engine","?")] = eng.get(x.get("engine","?"),0)+1
                err_flag = r["result"].get("isError", False)
                print(f"{prov:11}: {len(items)} results, engines={eng}, isError={err_flag}")
            else:
                print(f"{prov:11}: TEXT={txt[:200]}")
        elif r and "error" in r:
            data = r["error"]
            hint = data.get("data",{}).get("hint","")
            print(f"{prov:11}: ERROR '{data.get('message','?')}' hint={hint[:120]}")
        else:
            print(f"{prov:11}: UNEXPECTED {r}")
        p.terminate()
        p.wait(timeout=5)
    except subprocess.TimeoutExpired:
        print(f"{prov:11}: TIMEOUT (>30s) - likely transport hang")
        try: p.kill()
        except: pass
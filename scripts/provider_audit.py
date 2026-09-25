"""Run a small live provider quality matrix against the local Argos binary.

This is intentionally conservative: one process per provider/query, a short
pause between requests, and no writes to fixtures. It is a diagnostic harness,
not a load test.
"""
from __future__ import annotations

import json
import os
import subprocess
import time
from pathlib import Path

EXE = Path("target/release/argos-engine.exe").resolve()
QUERIES = [
    {
        "id": "niche-vst",
        "query": "free Spitfire LABS piano VST",
        "domains": ["kvrforums.com", "reddit.com"],
    },
    {"id": "general-rust", "query": "Rust async Tokio tutorial"},
    {"id": "exact-entity", "query": "Spitfire Audio LABS free"},
]


def rpc(proc: subprocess.Popen[str], message: dict[str, object]) -> dict[str, object]:
    proc.stdin.write(json.dumps(message) + "\n")
    proc.stdin.flush()
    line = proc.stdout.readline()
    if not line:
        raise RuntimeError(proc.stderr.read()[-1000:])
    return json.loads(line)


def call(provider: str, item: dict[str, object]) -> dict[str, object]:
    env = os.environ.copy()
    env["ARGOS_PROVIDERS"] = provider
    started = time.perf_counter()
    proc = subprocess.Popen(
        [str(EXE)],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
        text=True,
        encoding="utf-8",
    )
    try:
        rpc(proc, {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "argos-provider-audit", "version": "1"},
            },
        })
        proc.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
        proc.stdin.flush()
        arguments: dict[str, object] = {
            "query": item["query"],
            "limit": 10,
            "page": 1,
        }
        if item.get("domains"):
            arguments["domains"] = item["domains"]
        response = rpc(proc, {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": "search", "arguments": arguments},
        })
        row: dict[str, object] = {
            "provider": provider,
            "query_id": item["id"],
            "elapsed_ms": round((time.perf_counter() - started) * 1000, 1),
        }
        if "error" in response:
            row["error"] = response["error"]
            return row
        outcome = json.loads(response["result"]["content"][0]["text"])
        results = outcome.get("results", [])
        row.update({
            "count": len(results),
            "providers": outcome.get("providers", []),
            "warnings": outcome.get("warnings", []),
            "sources": [result.get("source") for result in results],
            "titles": [result.get("title") for result in results],
            "urls": [result.get("url") for result in results],
        })
        return row
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()


def main() -> None:
    rows = []
    for provider in ("duckduckgo", "bing", "brave"):
        for item in QUERIES:
            try:
                rows.append(call(provider, item))
            except Exception as exc:  # diagnostic output must survive one bad run
                rows.append({"provider": provider, "query_id": item["id"], "error": f"{type(exc).__name__}: {exc}"})
            time.sleep(1.5)
    print(json.dumps(rows, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()

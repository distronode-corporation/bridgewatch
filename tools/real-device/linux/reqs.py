#!/usr/bin/env python3
"""reqs.py <run-dir> <name>: how many requests a stand-in got, which made-up token each
carried (by its file name under the run directory, never its value), and the top paths.

Run through `harness.sh reqs <name>`.
"""
import collections
import json
import pathlib
import sys

run, name = pathlib.Path(sys.argv[1]), sys.argv[2]
tokens = {p.read_text(): p.name for p in run.glob("token-*")}
by_token = collections.Counter()
paths = collections.Counter()
log = run / "srv" / f"{name}.requests.jsonl"
for line in (log.read_text().splitlines() if log.exists() else []):
    r = json.loads(line)
    h = {k.lower(): v for k, v in r["headers"].items()}
    t = h.get("private-token") or h.get("authorization")
    if t and t.lower().startswith("bearer "):
        t = t[7:]
    by_token[tokens.get(t, "none" if t is None else "OTHER")] += 1
    paths[r["path"].split("?")[0]] += 1
print(f"{name}: {sum(by_token.values())} requests, tokens {dict(by_token)}")
for p, k in paths.most_common(8):
    print(f"   {k:4d} {p}")

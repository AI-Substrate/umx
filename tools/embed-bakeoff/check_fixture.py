#!/usr/bin/env python3
"""Check the bake-off fixture's shape (stdlib only). Exits 1 on a broken rule."""
import json
import re
import sys
from collections import Counter
from pathlib import Path

D = Path(__file__).resolve().parents[2] / "docs/project/benchmarks/embed-bakeoff"
STOP = set("a an the of to in on for and or is are was were be by with at as it its this that from how what why when where which who do does did we our you your i not no can should about into over after before than then there their they them so have has had one get got".split())


def words(s):
    return re.findall(r"[a-z0-9']+", s.lower())


nodes = [json.loads(l) for l in (D / "nodes.jsonl").read_text().splitlines() if l.strip()]
queries = [json.loads(l) for l in (D / "queries.jsonl").read_text().splitlines() if l.strip()]
byid = {n["id"]: n for n in nodes}
errors = []
if len(byid) != len(nodes):
    errors.append("duplicate node ids")
if len({q["id"] for q in queries}) != len(queries):
    errors.append("duplicate query ids")

for q in queries:
    if not 1 <= len(q["relevant"]) <= 3:
        errors.append(f"{q['id']}: {len(q['relevant'])} relevant ids")
    missing = [r for r in q["relevant"] if r not in byid]
    if missing:
        errors.append(f"{q['id']}: unknown relevant ids {missing}")
        continue
    qw = words(q["text"])
    q3 = {tuple(qw[i:i + 3]) for i in range(len(qw) - 2)}
    for r in q["relevant"]:
        nw = words(byid[r]["title"] + " " + byid[r]["note"])
        n3 = {tuple(nw[i:i + 3]) for i in range(len(nw) - 2)}
        if q3 & n3:
            errors.append(f"{q['id']}: reuses 3 consecutive words from {r}: {sorted(q3 & n3)[0]}")
        if q["kind"] == "paraphrase":
            shared = (set(qw) - STOP) & set(nw)
            if shared:
                errors.append(f"{q['id']}: paraphrase shares content words with {r}: {sorted(shared)}")

long_nodes = [n["id"] for n in nodes if len(words(n["note"])) > 550]
for q in queries:
    if q["kind"] == "tail" and q["relevant"][0] not in long_nodes:
        errors.append(f"{q['id']}: tail query's first relevant node is not long")

print(f"nodes {len(nodes)}, by domain {dict(Counter(n['id'].split('-')[0] for n in nodes))}")
print(f"queries {len(queries)}, by kind {dict(Counter(q['kind'] for q in queries))}")
print(f"long notes (> 550 words) {len(long_nodes)}")
for e in errors:
    print("ERROR", e)
sys.exit(1 if errors else 0)

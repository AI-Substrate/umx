#!/usr/bin/env python3
"""Writes data.js from a loaded umx graph, for index.html to draw. Read only: it runs `umx query` and `umx edge list`.

    python3 export-graph.py --graph <alias>
    open "index.html#data=data.js&play=1"

data.js holds real graph text. It is git-ignored here. Keep it out of commits, chats and screenshots you share.

What the page needs from the graph:
  - band nodes: one type (default `segment`) whose nodes are the steps along the spine;
  - an order for them: a step number, or a start and end written as HH:MM (both is best);
  - placing links: relations (default raised_in, part_of) from other nodes to a band node.
"""
import argparse
import collections
import json
import os
import re
import subprocess
import sys

NAME = re.compile(r"^[a-z_][a-z0-9_]*$")


def umx(binary, *args):
    r = subprocess.run([binary, *args], capture_output=True, text=True, env={**os.environ, "UMX_OUTPUT": "json"})
    try:
        d = json.loads(r.stdout)
    except ValueError:
        sys.exit("umx gave no JSON for: " + " ".join(args[:2]) + "\n" + r.stderr[:400])
    if d.get("status") != "ok":
        sys.exit("umx refused: " + json.dumps(d.get("errors"))[:600])
    return d["data"]


def families(nodes, band_type):
    """Four groups for the four quadrants: types dealt out biggest first, each to the lightest group."""
    counts = collections.Counter(n["t"] for n in nodes if n["t"] != band_type)
    groups = [{"types": [], "n": 0} for _ in range(min(4, len(counts)))]
    for t, c in counts.most_common():
        g = min(groups, key=lambda g: g["n"])
        g["types"].append(t)
        g["n"] += c
    return [{"name": " / ".join(g["types"][:2]) + (" …" if len(g["types"]) > 2 else ""), "types": g["types"]} for g in groups]


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--graph", required=True, help="alias of a loaded graph (see `umx graph list`)")
    ap.add_argument("--out", default=os.path.join(os.path.dirname(os.path.abspath(__file__)), "data.js"))
    ap.add_argument("--umx", default="umx", help="the umx binary")
    ap.add_argument("--band-type", default="segment")
    ap.add_argument("--start-field", default="starts_at")
    ap.add_argument("--end-field", default="ends_at")
    ap.add_argument("--step-field", default="step")
    ap.add_argument("--place-relations", default="raised_in,part_of")
    ap.add_argument("--families", help="a JSON file: [{\"name\": ..., \"types\": [...]}, ...], at most four")
    a = ap.parse_args()
    for name in [a.band_type, a.start_field, a.end_field, a.step_field, *a.place_relations.split(",")]:
        if not NAME.match(name):
            sys.exit("not a plain name: " + name)

    # Column names must differ from the variable name, or umx answers query_alias_shadows_variable.
    cypher = ("MATCH (x) RETURN x.id AS id, x.type AS kind, x.status AS st, x.title AS title, "
              "x.%s AS a, x.%s AS b, x.%s AS step, x.evidence AS ev" % (a.start_field, a.end_field, a.step_field))
    rows = umx(a.umx, "query", cypher, "--graph", a.graph, "--limit", "100000")["rows"]
    index = {r["id"]: i for i, r in enumerate(rows)}
    nodes = []
    for r in rows:
        ev = None  # where the node's evidence sits in a transcript, used to order nodes inside a band
        for e in r.get("ev") or []:
            sel = (e or {}).get("selector") or {}
            if "seq" in sel:
                ev = [os.path.basename(e.get("source") or ""), sel["seq"][0]]
                break
        nodes.append({"id": r["id"], "t": r["kind"], "s": r["st"], "n": (r["title"] or "")[:110],
                      "a": r.get("a"), "b": r.get("b"), "step": r.get("step"), "ev": ev})
    listed = umx(a.umx, "edge", "list", "--graph", a.graph, "--limit", "100000")["rows"]
    edges = [[index[e["from"]], index[e["to"]], e["type"]] for e in listed if e["from"] in index and e["to"] in index]

    bands = [n for n in nodes if n["t"] == a.band_type]
    if not bands:
        sys.exit("no nodes of type %s: pick the band type with --band-type" % a.band_type)
    out = {
        "graph": {"alias": a.graph, "nodes": len(nodes), "edges": len(listed)},
        "config": {"bandType": a.band_type, "placeRelations": a.place_relations.split(","),
                   "alongLabel": "%s order (%s)" % (a.band_type, " or ".join(a.place_relations.split(",")))},
        "families": json.load(open(a.families)) if a.families else families(nodes, a.band_type),
        "nodes": nodes,
        "edges": edges,
    }
    with open(a.out, "w") as f:
        f.write("window.POC_DATA=" + json.dumps(out, separators=(",", ":")) + ";\n")
    place = set(a.place_relations.split(","))
    band_ix = {i for i, n in enumerate(nodes) if n["t"] == a.band_type}
    placed = {e[0] for e in edges if e[2] in place and e[1] in band_ix and e[0] not in band_ix}
    print("wrote %s: %d nodes, %d links, %d bands, %d nodes placed by data" % (a.out, len(nodes), len(edges), len(bands), len(placed)))


if __name__ == "__main__":
    main()

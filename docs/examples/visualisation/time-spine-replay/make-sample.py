#!/usr/bin/env python3
"""Writes sample-data.js: a made-up workshop graph, so the exemplar runs with no real graph and no client text.

Everything here is invented. The same seed gives the same file. Run: python3 make-sample.py
"""
import json
import os
import random

R = random.Random(20261112)

SEGMENTS = [  # title, start, end
    ("Welcome and the brief", "09:00", "09:25"),
    ("What riders told us", "09:25", "09:55"),
    ("Breakout one: what is broken at night", "09:55", "10:35"),
    ("Report-backs one", "10:35", "11:05"),
    ("Morning break", "11:05", "11:20"),
    ("Breakout two: routes we could run", "11:20", "12:05"),
    ("Report-backs two", "12:05", "12:40"),
    ("Lunch", "12:40", "13:25"),
    ("Pressure test: what would make this fail", "13:25", "14:10"),
    ("Votes and next steps", "14:10", "14:40"),
]
SUBSEGMENTS = [  # inside the first segment: a start and end but no step number
    ("Chair's welcome", "09:00", "09:10"),
    ("The brief in three slides", "09:10", "09:25"),
]
# how many of each kind are raised in each segment (by position in SEGMENTS)
MIX = {
    "problem": [2, 9, 16, 6, 0, 2, 1, 0, 12, 1],
    "option": [1, 2, 4, 12, 0, 18, 14, 0, 3, 4],
    "mechanism": [0, 1, 2, 3, 0, 8, 5, 0, 2, 1],
    "question": [2, 3, 3, 2, 0, 2, 2, 0, 4, 1],
    "decision": [1, 0, 0, 3, 0, 1, 4, 0, 2, 7],
    "lesson": [0, 2, 1, 1, 0, 0, 1, 0, 3, 2],
    "document": [2, 2, 0, 1, 0, 1, 0, 0, 0, 1],
}
WORDS = {
    "problem": (["Last bus leaves before", "No shelter at", "Long wait at", "Unsafe walk from", "No timetable shown at", "Drivers skip"],
                ["the hospital shift change", "the harbour stop", "the college gate", "the depot end", "the market square", "the ring road"]),
    "option": (["Run a loop through", "Add an hourly service to", "Swap the big bus for a minibus on", "Trial an on-call stop at", "Extend route 9 to"],
               ["the hospital", "the harbour", "the college", "the old town", "the retail park", "the night market"]),
    "mechanism": (["Fund it from", "Staff it with", "Measure it by", "Publish it through"],
                  ["the parking levy", "volunteer marshals", "tap-on counts", "the council app", "a six-week trial"]),
    "question": (["Who pays for", "How many people need", "What happens to", "Can we legally run"],
                 ["the 2am service?", "a stop at the hospital?", "the school run?", "an on-call stop?"]),
    "decision": (["Trial", "Drop", "Cost up", "Take to council:"],
                 ["the hospital loop", "the 2am harbour run", "the minibus swap", "on-call stops", "a six-week pilot"]),
    "lesson": (["Riders care more about", "Nobody trusts", "The data hides"],
               ["waiting time than journey time", "a timetable that changes", "people who gave up riding"]),
    "document": (["Rider survey:", "Depot roster:", "Night economy report:"],
                 ["summary", "appendix B", "the map", "2025 figures"]),
}
STATUS = {"problem": ["observed", "agreed", "contested"], "option": ["proposed", "proposed", "proposed", "agreed", "contested"],
          "mechanism": ["proposed", "proposed", "agreed"], "question": ["open", "open", "answered"],
          "decision": ["agreed", "agreed", "proposed", "contested"], "lesson": ["observed"], "document": ["context"]}
PEOPLE = ["Ada Okoro", "Ben Lindqvist", "Chiara Rossi", "Dev Anand", "Esme Clarke", "Farid Haddad", "Grace Nwosu", "Hiro Tanaka",
          "Ines Duarte", "Jonas Weber", "Kira Petrov", "Liam Byrne"]
TEAMS = ["Table one", "Table two", "Table three", "Online group"]

nodes, edges = [], []


def node(kind, title, status, **extra):
    n = {"id": "S%04d" % len(nodes), "t": kind, "s": status, "n": title, "a": None, "b": None, "step": None, "ev": None}
    n.update(extra)
    nodes.append(n)
    return len(nodes) - 1


def link(a, b, kind):
    edges.append([a, b, kind])


meeting = node("meeting", "Night bus network: community workshop (invented sample)", "observed")
seg = [node("segment", t, "observed", a=a, b=b, step=k + 1) for k, (t, a, b) in enumerate(SEGMENTS)]
sub = [node("segment", t, "observed", a=a, b=b) for t, a, b in SUBSEGMENTS]
for k in range(len(seg) - 1):
    link(seg[k], seg[k + 1], "precedes")
for s in seg:
    link(s, meeting, "part_of")
for s in sub:
    link(s, seg[0], "part_of")

people = [node("person", p, "observed") for p in PEOPLE]
teams = [node("team", t, "observed") for t in TEAMS]
for i, p in enumerate(people):
    link(p, meeting, "attended")
    link(p, teams[i % len(teams)], "member_of")
for t in teams:  # a team is placed by data in the two breakouts
    link(t, seg[2], "part_of")
    link(t, seg[5], "part_of")

items = []  # (index, kind, segment position)
for kind, per_seg in MIX.items():
    heads, tails = WORDS[kind]
    for k, count in enumerate(per_seg):
        for c in range(count):
            title = "%s %s" % (R.choice(heads), R.choice(tails))
            ev = ["sample-transcript.ndjson", k * 1000 + R.randrange(900)] if R.random() < 0.7 else None
            i = node(kind, title, R.choice(STATUS[kind]), ev=ev)
            items.append((i, kind, k))
            if R.random() < 0.86:  # most things say where they were raised; the rest get placed by rule or sit on the shelf
                link(i, sub[R.randrange(2)] if k == 0 and R.random() < 0.6 else seg[k], "raised_in")
            if R.random() < 0.55:
                link(i, R.choice(people + teams), "raised_by")

by_kind = lambda kind: [x for x in items if x[1] == kind]


def near(k, pool, spread=1):
    close = [x for x in pool if abs(x[2] - k) <= spread]
    return R.choice(close or pool)


for i, kind, k in items:
    if kind == "option":
        if R.random() < 0.8:
            link(i, near(k, by_kind("problem"), 3)[0], "answers")
        if R.random() < 0.35:
            link(i, near(k, by_kind("decision"), 4)[0], "part_of")
    if kind == "mechanism":
        link(i, near(k, by_kind("option"))[0], "supports")
    if kind == "problem" and k >= 8:  # the pressure test pushes back on earlier options
        for _ in range(2):
            link(i, R.choice([x for x in by_kind("option") if x[2] < 8])[0], "undercuts")
    if kind == "lesson":
        link(i, near(k, by_kind("problem"), 2)[0], "relates_to")
    if kind == "decision":
        for _ in range(R.randrange(2, 5)):
            link(near(k, by_kind("option"), 4)[0], i, "supports")
    if kind == "question" and R.random() < 0.5:
        link(near(k, by_kind("option"), 2)[0], i, "answers")
    if kind == "document":
        link(near(k, by_kind("problem"), 2)[0], i, "sourced_from")

out = {
    "graph": {"alias": "sample_night_bus_workshop", "nodes": len(nodes), "edges": len(edges)},
    "config": {"bandType": "segment", "alongLabel": "time (raised in a segment)"},
    "families": [
        {"name": "Options and mechanisms", "types": ["option", "mechanism"]},
        {"name": "Problems and questions", "types": ["problem", "question"]},
        {"name": "Decisions and lessons", "types": ["decision", "lesson"]},
        {"name": "People and sources", "types": ["person", "team", "document", "meeting"]},
    ],
    "nodes": nodes,
    "edges": edges,
}
path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "sample-data.js")
with open(path, "w") as f:
    f.write("// Invented sample, written by make-sample.py. No real graph text.\n")
    f.write("window.POC_DATA=" + json.dumps(out, separators=(",", ":")) + ";\n")
print("wrote", path, len(nodes), "nodes", len(edges), "edges")

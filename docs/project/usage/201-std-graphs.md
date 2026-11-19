---
id: 201
title: A new graph uses std v1: write std names with no declaration, and read which entries std gives
route: build
status: current
source: ws008 T2a (decision D57: std built in and versioned, new graphs opt in by default)
reqs: [req-98]
supersedes: none
---
## Do
Make graphs with plain `umx graph new <folder> --alias <alias>`: the graph uses std v1, and its graph.json's ontology says `"std": 1`. On it, write the std names straight away with no `ontology add`: `umx node add --type person --set job_title="Head of sales"`, `umx node add --type meeting --set starts_on=2026-11-17T09:00:00+10:00`, `umx edge add <person> works_for <organisation>`, `umx node status <node> stalled --reason "<why>"`. None warns off-vocabulary, word search finds `job_title` text, and Cypher knows `person` as a label. Declare only what the graph adds beyond std, with `umx ontology add` as before.

Read the vocabulary with `umx ontology show`: entries with `std: true` (and the names in `std_statuses` and `std_relations`) come from umx, the rest are the graph's own. `umx ontology show --std` prints std v1 alone. Use `--no-std` only for a graph that must not share the common names (a scratch graph, a vocabulary test).

## Don't
Don't declare a std name again to make it known: it is already, and a re-declaration only merges into std's entry (a field std places keeps std's kind). Don't copy std's entries into graph.json by hand: umx never writes them there, so a std graph's graph.json holds only `"std": 1` and its own declarations. Don't remove `"std"` from graph.json to get rid of a std name: std is all or nothing. On `std_newer` (a graph.json from a newer umx), keep working: the graph loads with this umx's std names, and only names the newer std added warn until umx is updated.

## Example
```bash
umx graph new /Users/me/work/launch --alias launch
umx node add --graph launch --type person --title "Ada" --set job_title="Head of sales"   # no warning
umx search "head of sales" --graph launch                                               # finds Ada by job_title
umx ontology show --graph launch                                                         # person std: true, hint std_entries_shown
umx ontology show --std                                                                  # std v1 alone
umx graph new /Users/me/work/scratch --alias scratch --no-std                            # no std
```

## Why
Graphs that use the same names for the same things line up across graphs, and an agent should not spend a dozen `ontology add` calls declaring the people, meeting and time names on every new graph. Keeping std's entries out of graph.json keeps the files small, keeps a graph without std byte for byte as it was, and lets a later std add names without rewriting every graph.

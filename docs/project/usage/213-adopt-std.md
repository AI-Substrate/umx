---
id: 213
title: Move a graph made without std onto std with ontology adopt-std, renaming only by --map
route: build
status: current
source: ws008 T3 (req-101; decision D57)
reqs: [req-101]
supersedes: none
---
## Do
Run `umx ontology adopt-std --dry-run --graph <g>` first: it writes nothing and prints the plan, what std now supplies (`folded`), the graph's own fields kept on std types (`extended`), the names std does not have (`custom`), and any conflict with its fix. Then run it without `--dry-run`. graph.json gets `"std": 1` and loses its own copies of std's entries; no node or edge line changes, so a graph built to std's names adopts with no line change. To move a type or relation of your own onto std's name, say so with `--map individual=person` (repeat it): only the lines with that name change, stamped like `node edit --type` and `edge edit --relation` (`updated`, an edge's `updated_by`, `edited_by_session`), and every id and `umx:<graph>/<id>` address stays. Before a big `--map`, run it with `--dry-run` to see how many lines it renames: they go in one write, and memory holds about one more image of the graph while it runs. Run it again whenever you like: a std graph answers `already_std` and writes nothing. It works on a secured graph named with `--graph`.

## Don't
Don't expect umx to rename on its own: `--dry-run` suggests a rename for a near miss of a std name (`organization`, hint `adopt_rename_suggested`) and you choose. Don't adopt while a conflict is listed: nothing is written until it is fixed. A field declared with another kind than std's (`adopt_field_kind_clash`) needs its values unset where they are held (the conflict's next lists them) and its declaration removed first; stored `authored_by`, `edited_by` or `similar_to` edges (`adopt_reserved_relation`) need `--map authored_by=written_by`. Don't `--map` onto a name the graph declares differently (`adopt_map_merge`), across symmetry (`adopt_map_symmetry`) or so two edges become one (`adopt_map_duplicate_edge`). Don't chain or swap maps (`--map a=b --map b=c`), or map onto a name other writes refuse (`std`, `graph`, `not_same_as`, a derived or provenance name); `not_same_as` edges stay `umx entity deny`'s. An edge holding its own `updated` refuses a rename (`edge_reserved_field_in_use`), as `edge edit` does. Don't hand-edit graph.json to give a std field another kind: it loads with std's kind and warns `std_field_conflict_stored`.

## Example
```bash
umx graph new ~/work/old --alias old --no-std                         # as graphs were before std
umx ontology adopt-std --dry-run --graph old                          # the plan; nothing written
umx ontology adopt-std --map individual=person --graph old            # std: 1; individual lines become person
umx ontology adopt-std --graph old                                    # already_std: nothing written
```

## Why
std's names mean the same thing in every std graph, so moving an old graph onto them should keep everything that already says the same thing and change nothing else. One write set keeps graph.json and the lines in step across a crash, ids keep every link and address working, and a rename the caller did not ask for would change what other graphs and agents read, so umx only offers it.

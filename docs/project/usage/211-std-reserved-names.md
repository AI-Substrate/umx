---
id: 211
title: std names are umx's in a std graph, std itself is reserved everywhere, and std relations expect end types softly
route: build
status: current
source: ws008 T2b (req-99; decision D57; the cypher-expert's std form)
reqs: [req-99]
supersedes: none
---
## Do
Use std's names as they are. In a std graph, `umx ontology add type person` or `umx ontology add relation works_for` with nothing else is `"unchanged": true`: std has them already. A field std has keeps std's kind on any type or relation (`job_title` is a string, `due_on`, `starts_on` and `ends_on` are dates, `aliases` and `tags` are lists, `step` is an int); declare another name for another kind.

Link people and work with the ends std expects: `works_for` person to organisation, `member_of` person or team to team or organisation, `raised_by` a question, risk, decision or action to a person, team or organisation, `held_at` a meeting or event to a place or system. An edge outside them is stored with the warning `relation_endpoint_unexpected`, which names the expected types: check the direction and the types, and if the graph really links other types that way, widen the relation once with `umx ontology add relation works_for --from person --to team`.

`std:` is accepted before a std type or relation name on any flag or word that takes one (`--type std:person`, `umx edge add <a> std:works_for <b>`), and the name is stored plain. In Cypher write std names plain: `MATCH (p:person)-[:member_of]->(t:team)`.

## Don't
Don't name a type, relation or field `std`, in any graph (`reserved_name`). Don't declare or add umx's provenance and derived relations, in any graph (`umx ontology add relation authored_by` is `derived_relation`, `session_of` is `reserved_relation`), nor, in a std graph, its provenance types (`umx ontology add type session` is `reserved_name`; a graph without std may use `model` or `agent` as its own types): umx records who wrote what itself (`umx docs get node`). Don't write `(n:std:person)` or `-[:std:member_of]-` in a query (`query_std_qualifier`): it is a label union that would match every node. On `reserved_relation_stored` at load (an authored_by or edited_by edge an older umx stored), read the edge with the next and leave it: nothing is dropped, and authorship reads from the node's session fields. This refines usage 201: a std name re-declared with a field of another kind is refused (`std_field_conflict`), not merged.

## Example
```bash
umx ontology add type person --graph launch                       # unchanged: std has it
umx ontology add field person job_title:int --graph launch        # std_field_conflict: job_title is a string
umx edge add <ada> works_for <platform team> --graph launch       # stored, warns relation_endpoint_unexpected (organisation)
umx ontology add relation works_for --from person --to team --graph launch
umx edge add <bo> std:works_for <platform team> --graph launch    # no warning; stored as works_for
umx query 'MATCH (p:person)-[:member_of]->(t:team) RETURN p.title AS name' --graph launch
```

## Why
std names mean the same thing in every graph that uses std, so a graph can add to them but not change their kinds or reuse umx's own provenance names. The end types are soft because real data has exceptions; the warning catches a reversed or mistyped edge without blocking the write. Reserving `std` keeps the `std:` qualifier unambiguous on the command line, and refusing it in Cypher stops a typo from widening a query to every node.

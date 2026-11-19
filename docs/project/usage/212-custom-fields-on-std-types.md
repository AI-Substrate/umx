---
id: 212
title: A std graph adds its own fields to std types, never removes or retypes std's own
route: build
status: current
source: ws008 T2c (req-100; decision D57)
reqs: [req-100]
supersedes: none
---
## Do
Add a field a std type lacks with `umx ontology add field person shoe_size:int`: graph.json gets only `shoe_size`, and `umx ontology show` lists std's fields first (`"std": true`), then yours (`"std": false`). Use it like any field: `node add` and `node edit --set`, batch, `--where`, `--columns`, word search for a string field, `umx ontology set field person shoe_size --role ...`, and `p.shoe_size` in Cypher. Several types take a field in one call, std's types included: `umx ontology add field risk,team impact:int`. Take a custom field out with `umx ontology remove field person shoe_size`.

Re-running a setup script that declares a field std already gives the type (`umx ontology add field meeting starts_on:date`, `person job_title:string`, `--relation works_for starts_on:date`) is fine: `"unchanged": true`, nothing written. To describe such a field, add it with `--description`: graph.json gets the graph's own declaration, std's kind and your description. To change its role, use `umx ontology set field person job_title --role content`; removing that field again (`umx ontology remove field person job_title`) drops only the graph's declaration and std's comes back. When that moves the role, it warns `role_changed` and the nodes are pending until remade, as with `ontology set field`.

## Don't
Don't give a std field name another kind (`std_field_conflict`): pick another name. On a type std does not give the field to, the name is fine with std's kind (`umx ontology add field topic due_on:date`). Don't use a reserved key as a field name (`id`, `title`, `status`, `note`, `prov_key` and the other provenance keys, the locator fields, `entity_key`, `std`): `reserved_name`. Don't try to remove a field std gives the type (`umx ontology remove field person aliases` is `reserved_name`): a graph adds to std, it never narrows it. Don't expect `--all-types` to reach std's types: it means the types graph.json declares. Don't hand-edit graph.json to retype a std field: the graph loads, but std's kind wins.

## Example
```bash
umx ontology add field person shoe_size:int --graph launch           # graph.json: person { shoe_size:int } only
umx ontology add field person job_title:string --graph launch        # unchanged: std has it
umx ontology add field person job_title:int --graph launch           # std_field_conflict
umx ontology add field risk,team impact:int --graph launch           # std types count as declared
umx node add --type person --title Ada --set shoe_size=42 --graph launch
umx query 'MATCH (p:person) WHERE p.shoe_size > 40 RETURN p.title AS name' --graph launch
umx ontology remove field person aliases --graph launch              # reserved_name: std's own field
umx ontology remove field person shoe_size --graph launch            # removes the extension (once no node holds it)
```

## Why
std's fields mean the same thing in every std graph, so a graph may add to a std type but never narrow or retype it. Treating std's fields as already declared keeps graph.json to the graph's own words and lets setup scripts re-run without writing copies of std into the file. This refines usage 211: a std field redeclared with std's kind where std already places it writes nothing.

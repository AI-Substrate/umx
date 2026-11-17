---
id: 114
title: Declare data fields with --role data; they are never embedded
route: build
status: current
source: decision D56 (2026-11-11); plan 082 (embeddings-expert, architect, graph-storage-expert, security); ws004 s22
reqs: [req-97, req-34, req-35, req-75]
supersedes: none
---
## Do

When you declare a string field that holds a file name, an id, an agent id, a seat, a hash, a locator or
provenance, declare it as data: `umx ontology add field <type> <name>:string --role data`. Data fields get no
vector and are left out of the whole-node text, so they never skew similarity or go to an embedder. Prose that
says what something means is content only when declared `--role content`: a string field with no role is data
(note 206).

To change a declared field later, use `umx ontology set field <type> <field> --role data|content`. The reply's
`pending` and the `role_changed` warning say how many nodes' vectors will be remade by the background worker.
Check roles with `umx ontology show`: each field shows `role` and `role_default`.

Before you change a field's role or run reembed on a graph another seat writes, send that graph's owning seat
one line saying what you will change, and give it a moment to object. Role changes and reembed remake vectors
and call the embedder.

On `text_form_outdated` at load, the graph has vectors made from text that held a field now marked data (a pulled
graph.json, or vectors from before D56). They are kept and still searched, and stay pending: a load never remakes
or clears them, and nothing does until you run `umx embeddings reembed --dry-run` to see the cost, then `umx
embeddings reembed`, or you state the field content again. graph.json's `text_form` rises to 2 only at reembed, or at the next graph.json write once no
node is outdated; never at load.

## Don't

Don't try `--role content` on an int, float, bool, date or list field: they are never embedded
(`role_not_embeddable`). Don't expect `ontology set field` to declare a field (`field_not_declared`); declare
it with `ontology add field` first. Don't re-run `ontology add field` with another `--role` to change a role: an add
that differs only in role is refused (`field_role_conflict`); use `ontology set field`. Don't try to make provenance or identity keys (`prov_key`,
`authored_by_session`, `edited_by_session`, `entity_key`, and the legacy identity keys `person_key`, `org_key`,
`team_key` that older graphs may still carry) or provenance nodes embed:
umx never embeds them, whatever graph.json says. Don't use roles to hide text from word search: data fields of
kind string are still found by words and `--where`. A role needs graph.json format 4, so an older umx
refuses the graph; upgrade umx on every machine that loads it. Don't change roles on another seat's graph
without telling its owner first.

## Example

    umx ontology add field finding source_file:string --role data
    umx ontology add field finding detail:string --role content
    umx ontology set field finding source_file --role data
    umx ontology show
    umx embeddings reembed --dry-run

## Why

D56: data fields (file names, ids, seats, hashes, locators, provenance) carry no meaning an embedding can use,
so embedding them adds noise to similarity and sends identifiers off the machine for nothing. Leaving them out
is whole-node text form 2. Vectors whose text did not change are kept (a graph with no data field loads with
zero pending and zero embed calls). Nothing is re-embedded on load; only reembed or an explicit role change
remakes vectors.

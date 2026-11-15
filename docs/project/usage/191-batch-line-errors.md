---
id: 191
title: Batch lines say which field is wrong, node add takes note-shaped evidence, refs go in _ref fields
route: build
status: current
source: plan 205 (workstream 003 batch follow-ups, field reports)
reqs: [req-88]
supersedes: none
---
## Do

- Give a batch `node add` line's evidence as pointer strings, or as `{"pointer": ..., "kind": ...}` entries exactly
  as a `node note` line takes them. Mix both in one list. An entry without a kind takes the line's
  `evidence_kind`, else `document`.
- Put custom fields in `sets`: `{"op":"edge add",...,"sets":[{"field":"why","value":"..."}]}`. A top-level `why`
  fails with "the edge add line has no field why: custom fields go in sets".
- Name a node a batch line added with the `_ref` field (`node_ref`, `parent_ref`, `from_ref`, `to_ref`). When a
  plain field holds a ref the batch already bound, the failed reply carries the hint `batch_use_ref_field`
  naming the field to use.
- Write refs in lowercase: a letter a-z, then lowercase a-z, 0-9, `_` and `-`. The CLI checks every ref while it
  reads the file, so a bad ref fails at once, naming the field, the character and its position.
- Read a `batch_line_invalid` reason for the field path: "the node note field evidence[0].pointer has the wrong
  type: give a string" names the op, the path and the type it takes.
- On `edge_exists_differs`, follow its next: `umx edge edit <id> --set <field>=<value>` changes the stored edge in
  place.

## Don't

- Don't give a kind on an entry and `evidence_kind` on the same node add line: it is `batch_line_invalid`. Pick
  one.
- Don't set one field twice in a line's `sets`. Only the last value is kept, and the line warns
  `duplicate_field` naming the field: the earlier values are dropped.
- Don't read a missing warning as none: past 100 warnings, `batch_warnings_truncated` gives the total, including
  warnings found after the write (one `vector_shard_unreadable` per node whose shard cannot be read).

## Example

    {"op":"node add","ref":"r1","type":"risk","title":"Disk fills up","evidence":["notes.md#L9",{"pointer":"calls/3-nov.vtt#seq=12-14","kind":"room"}]}
    {"op":"edge add","from_ref":"r1","relation":"blocks","to_ref":"q1","sets":[{"field":"why","value":"Raised in the review"}]}
    {"op":"node note","node_ref":"r1","append":"Confirmed","evidence":[{"pointer":"notes.md#L12","kind":"analyst"}]}

## Why

Agents building graphs with `umx batch` hit errors that did not say what to fix: a type error with no field, an
unknown top-level `why` with no advice, `node_not_found` on a ref given in a plain field, and a node add rejecting
the evidence shape a node note accepts as "not one JSON object". Uppercase refs failed only when the daemon checked
them. A field set twice was silently last-wins, and warnings found after the write bypassed the 100 cap.

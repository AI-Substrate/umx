---
id: 167
title: Use refs an earlier applied batch bound with umx batch --refs-from
route: build
status: current
source: ws003 s24, plan 135; graph-addressing-expert and architect joint answer (topic batch-refs-from, 2026-11-12)
reqs: [req-88]
supersedes: none
---
## Do

When a later batch (often a trailing links batch) needs nodes an earlier batch
added, save the earlier batch's JSON reply and pass it with `--refs-from`.
Repeat the flag, one receipt per earlier batch. Use the imported refs in any
`_ref` field: `parent_ref`, `from_ref`, `to_ref`, `node_ref`.

The receipt contract (part of the documented batch output, so its shape is a
wire meaning):

- `data.applied` is `true`. A dry run's receipt (`applied: false`) holds no ids
  and is refused with `batch_refs_from_dry_run`.
- `data.refs` maps each ref that batch itself defined to its full 26-character
  id. Imported refs are never listed again, so a receipt does not grow along a
  chain and a stale import is not passed on. Pass every receipt a batch needs.
- `data.graph_id` is the id of the graph the batch wrote. `--refs-from` requires
  it to equal the id of the graph the new batch writes, or refuses with
  `batch_refs_other_graph`; two receipts naming different graphs are refused
  the same way. Copies of one graph share its id, so any copy matches.

Dry-run the batch that uses the refs first. Each imported ref resolves like an
explicit id at the line that uses it, in dry and real runs alike.

## Don't

Do not hand-write the map or edit a receipt: only the JSON envelope an applied
`umx batch` printed is read (a `--human` reply or a failed reply is
`usage_invalid`). `--refs-from -` (stdin) is not accepted.

Do not define a ref in the batch with the same name as an imported one: that
line is `batch_ref_conflict`, never a shadow. Two receipts giving one ref
different ids is also `batch_ref_conflict`.

Do not expect a deleted, missing or provenance node to resolve: the line fails
with `batch_ref_unknown`, and the CLI adds a `batch_ref_source` error line right
after it naming the receipt the ref came from. Ids are never re-minted or
guessed. Rebuild the receipt by re-running the earlier step instead.

A receipt without `graph_id` (written before this feature) skips the graph
check. That is safe only because ids are unique, so another graph's id finds
no node here; do not rely on the graph check for those old receipts.

Do not use an imported ref to reach another graph: refs are local to the
batch's graph. Write a cross-graph end as `umx:<alias>/<id>`. At most 10,000
refs are imported across all receipts (`batch_too_large`).

There is no author check: any author's or session's receipt works. Keeping one
author per chain is a convention, not a rule umx enforces.

## Example

```sh
cat > nodes.jsonl <<'OPS'
{"op":"node add","ref":"q","type":"question","title":"Storage choice"}
{"op":"node add","ref":"a","type":"option","title":"Files","parent_ref":"q"}
OPS
umx batch --file nodes.jsonl > nodes.receipt.json

cat > links.jsonl <<'OPS'
{"op":"edge add","from_ref":"a","relation":"answers","to_ref":"q"}
OPS
umx batch --file links.jsonl --refs-from nodes.receipt.json --dry-run
umx batch --file links.jsonl --refs-from nodes.receipt.json
```

## Why

Fleets build a graph in steps: nodes first, links last. Carrying ids by hand
between steps is error-prone. The receipt umx already prints holds the ids, so
reading it keeps no state in umx (no stored per-author map) and keeps the file
path on the client: the daemon receives only the refs and the graph id.

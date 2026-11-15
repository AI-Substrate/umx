---
id: 146
title: Two resource nodes with one locator after a merge load and are reported, never erased
route: doctor
status: current
source: ws006 S3a (plan 118); workshop 010 section 9a (graph-storage-expert); project manager split of s3, 2026-11-12; review S241-1 (graph-storage-expert)
reqs: [req-77, req-67]
supersedes: none
---
## Do
- On `duplicate_locator` after `umx graph load` (or in `umx check`): two resource nodes hold the same file, folder
  or URL, as when both sides of a merge added it. Keep the lower id, the one the warning names as in use. Move any
  edges, notes or evidence you want from the other node onto it, then remove the other node.
- On `locator_not_normalised`: a resource node's source was edited by hand into a form umx would not write (`./a.md`,
  `docs//a.md`, an upper-case host). It still matches as its normal form. To store the normal form, fix the source
  in nodes.jsonl by hand.
- On `locator_field_kept`: a node holds a custom field named `locator` that is not a locator, written before umx
  reserved the name. It loads and is kept as it is. Rename it by hand in nodes.jsonl (for example to `location`)
  before you make that node a resource.
- Expect graph.json's format to rise (to 5) once any node has a locator. An older umx then refuses the graph with
  `graph_format_newer`: upgrade umx rather than editing the file.

## Don't
- Don't delete either line by hand to make the warning go away until you've moved what you need: nothing is erased
  for you, and both nodes stay in the graph meanwhile.
- Don't write `locator` or a `locator_*` name as a custom field: `--set locator=...` is refused with
  `locator_field_invalid` and nothing changes. The locator is a nested object only umx writes.

## Example
```bash
umx graph load ~/graphs/acme        # warns duplicate_locator for 01JF... (in use: 01JD...)
umx check --graph acme              # lists the same row
```

## Why
Workshop 010 9a: one resource node per locator, but a merge can bring in two. Like a repeated id (req-67), it
loads, is reported, and nothing is erased, so no merge loses work.

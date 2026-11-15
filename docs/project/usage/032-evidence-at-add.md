---
id: 032
title: Give a node its evidence in the same node add
route: ingest
status: current
source: refines 011; workshop 007 node add row; node add --evidence
reqs: [req-18, req-58]
supersedes: none
---
## Do
When you create a node from a source, cite it in the same command: `umx node add ... --evidence <pointer>` (repeat for several) with `--evidence-kind room|chat|screen|document|analyst` (default `document`). The pointer form and kind rules are those in 011. One `--evidence-kind` applies to every pointer in that command, so use separate commands for different kinds. For evidence you find later, use `umx node note` as in 011.

## Don't
Don't add the node and then run `node note` only to attach its first evidence: one command does both in one write. If any pointer is refused (`evidence_pointer_invalid`, `evidence_outside_roots`) or the kind is unknown (`usage_invalid`), no node is added, so fix the pointer and run the whole command again; don't retry without the evidence.

## Example
```bash
umx node add --type problem --title "Approval delays" --evidence 'transcripts/016.ndjson#seq=4100-4120' --evidence-kind room
```

## Why
A node never exists, even briefly, without its source, and an agent spends one command instead of two.

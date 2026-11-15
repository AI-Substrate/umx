---
id: 060
title: A write leaves other node lines as they are in nodes.jsonl
route: build
status: current
source: ws003 s05 design note 2 (graph-storage-expert, 2026-11-10), plan 026, workshop 008 section 2.3
reqs: [req-01, req-02]
supersedes: none
---
## Do
Expect a write to change only the lines of the records it changed. In `git diff` after `umx node add`, `umx node edit`
or any other node write, only the new or changed node's line appears in nodes.jsonl. A node line written by hand or
by another tool (keys in another order, extra spaces, unknown keys) keeps its bytes until umx changes that node. A
file whose last line has no final newline gets one at the first write. When umx changes it, the line is rewritten in umx's own form, with its unknown keys and number forms
(`1.0`) kept.

## Don't
Don't read a hand-edited node line that a write left alone as "umx did not load it". It is loaded and in use; umx
just did not rewrite it. Don't rely on a write to tidy up a hand-edited node: change that node through the CLI if
you want it in umx's form. Edges are not covered: edges.jsonl is still rewritten in umx's form on every edge write.

## Example
```bash
# nodes.jsonl has one hand-edited line for node 01JC...; add another node
umx node add --type risk --title "Vendor delays in procurement"
git diff --stat graph/nodes.jsonl   # one line added, the hand-edited line untouched
umx node note 01JC... --append "Checked with procurement."   # now that line is rewritten in umx's form
```

## Why
umx keeps nodes.jsonl in memory as its bytes and changes only the lines of the nodes a write touches. Diffs stay
small and a merge of two branches touches fewer lines. Before s05, every write rewrote every node line in umx's form.

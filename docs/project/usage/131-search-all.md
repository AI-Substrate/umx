---
id: 131
title: Select all open graphs explicitly and opt into named graphs per call
route: query
status: current
source: D53; the project manager plan 074; security and domain expert notes (2026-11-11)
reqs: [req-30, req-66, req-91]
supersedes: none
---
## Do

Use `umx search words --all` or `umx query '<query>' --all` to read every loaded open graph,
one primary copy per graph id, even when `UMX_GRAPH` points at one graph. Without a graph/default
selected, plain search and query already cover those open graphs.

Use `--include-named` only when the user asked for named or secured graphs in this task; never as
a default, and never as a retry for empty results. It requires `--all` and affects only that call.
The `secured_graphs_read` warning lists the named graphs read and says plainly that named text
is now in this output and stays on this machine only if the caller keeps it there.

Use `--graph a,b` when you intend to name a particular list: it already includes every named/secured
graph you list. `--scope name` is a saved list, not a way to save all/include-named.

## Don't

Do not combine `--all` with `--graph` or `--scope`; do not use `--include-named` without `--all`.
These combinations are `usage_invalid`, not silently narrowed scopes. No env, config or saved
scope turns on the opt-in.

Do not treat visibility opt-in as permission to send a query or graph text elsewhere. Each graph
keeps its own embedder, trust and locality rule. Without a local instance, a local-only graph
stays word-only with `similarity_unavailable`, never a hosted call.

Do not reflexively widen a skipped search. The skipped warning suggests naming a graph with
`--graph <name>`, not adding include-named.

## Example

```sh
umx search approval delay --all
umx query 'MATCH (n) RETURN n.graph AS graph, count(*) AS nodes' --all
# Only for a task whose user asked for named/secured graphs:
umx search approval delay --all --include-named
umx query 'MATCH (n) RETURN n.graph AS graph, n.title AS title' --all --include-named
```

Batch ref errors also identify the first bad character and its 1-based position: `brd_exA` has an
uppercase `A` at position 7; `9abc` must start with a lower-case letter; `a.b` has a dot at position 2.
Over-long refs name the 32-character limit. Unsafe/nonprintable characters are code points, and
the rest of the ref is not echoed. `next` stays `umx docs get batch`.

## Why

Explicit all-graph selection should not depend on a hidden default, and reading named data needs
a conscious per-task choice and a clear disclosure without weakening per-graph embedding rules.

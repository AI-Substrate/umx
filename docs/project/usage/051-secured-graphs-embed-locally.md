---
id: 051
title: A secured graph never goes to a hosted embedding model
route: build
status: current
source: architect ruling with the security role, 2026-11-10 (blueprint d-19); workstream 004
reqs: [req-46, req-72, req-73]
supersedes: none
---
## Do
Mark a graph secured when it holds client or private text, and give the machine a local embedder instance for it. For how config is set and checked, see notes 030 (`umx config`) and 031 (roots); this note adds only the secured rule.

## Don't
Don't expect `--embedder`, `UMX_*` env vars, the machine config or a repo's config to send a secured graph's text to a hosted model. The graph's policy beats every layer: nothing is sent. Loading or repairing the graph warns `embedder_not_local_for_secured` (its word search still works), and `umx embeddings reembed` is refused with `secured_embed_refused`.

## Example
```bash
umx config set embedders.nomic.kind local
umx config set embedders.nomic.model nomic-embed-text-v1.5
umx config set embedders.nomic.dims 256
umx config check
```

## Why
Secured text must never leave the machine (req-46). Making the graph's policy win over config, env and flags means no single setting, committed or typed, can leak it.

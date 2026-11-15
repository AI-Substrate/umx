---
id: 145
title: secured_shards_remain means old vector files sit in the graph folder; report them, a person removes them
route: doctor
status: current
source: security gate e remedy (security reviewer, 2026-11-12), the project manager and project lead on the catalogue wording (2026-11-12)
reqs: [req-46, req-91]
supersedes: none
---
## Do
- On `secured_shards_remain` (from `umx graph load` or `umx graph set` on a graph that is named, trusted or local),
  report the warning and the paths in its `candidates` to the person, and stop. Those are `embeddings/` and
  `similar/` files written while the graph was open.
- Know that the graph is fine: umx no longer reads or writes those files, and it re-embeds every node into this
  machine's cache by itself. `umx embeddings status` shows the cache filling.
- Check whether git tracks the files with `git ls-files` in the graph folder; umx does not check git.
- You may `umx graph unload` and `umx graph load` the graph to show whether the warning still holds.

## Don't
- Don't delete, `git rm` or commit the listed files yourself, don't rewrite git history or force-push, and don't set the graph
  back to open to make the warning go away. Removing them, and deciding whether history must be rewritten, is the
  person's (and the repository owner's) call.
- Don't copy the old files into the cache; umx never needs them.

## Example
```bash
umx graph set client_x --visibility named     # warns secured_shards_remain, listing the old files
umx embeddings status                          # the cache refilling for client_x
git -C /path/to/client_x ls-files embeddings similar
```

## Why
Vectors are not text, but they allow similarity and membership inference, and similar_to lists show relationships
directly. Files committed while the graph was open stay in git history and every clone until a person removes them,
so the remedy (`umx docs get embeddings`, Vectors left from when a graph was open) is theirs, not an agent's.

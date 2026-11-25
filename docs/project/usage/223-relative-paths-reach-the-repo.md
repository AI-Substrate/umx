---
id: 223
title: Relative evidence and locator paths may reach anywhere in the graph's repo
route: ingest
status: current
source: prime packet check-sources (2026-11-25; Jordan: yes to `..` inside the graph's git repo)
reqs: [req-47]
supersedes: none
---
## Do
Cite a file elsewhere in the same git repo as the graph with a plain relative path that starts with `..`: `--evidence '../../x/sources/a.ndjson#seq=1-4'` or `--locator ../specs/plan.md`. umx resolves it from the graph folder inside the git repo that holds it (the folder `root:repo` names, which must be inside `UMX_ROOTS`), with symlinks followed, and stores the pointer as written. Hashing, `umx check` and refresh read it the same way.

## Don't
Don't climb past the repo's top: that, a symlink that leads out, or `..` in a graph that is not in a git repo is refused with `evidence_outside_roots`, as before. Don't put `..` after another part (`a/../b`) or in an absolute path: refused. For files outside the repo, add a source root (`umx root add`) and cite `root:<name>/<path>`.

## Example
```bash
umx node add --type decision --title "Ship in May" --evidence '../../x/sources/3-nov.ndjson#seq=120-140' --evidence-kind room
umx check --graph launch
```

## Why
A graph usually lives beside the sources it cites, in the same repo. A relative path reads naturally and survives a clone, as `root:repo/<path>` does, while the repo stays the fence.

---
id: 149
title: Add each file, folder or URL once as a resource node, and re-point it rather than add another
route: ingest
status: current
source: ws006 S3b (plan 121); workshop 010 sections 3 and 12; s3 done_when (the project with the graph-addressing-expert); graph-addressing-expert and security on review
reqs: [req-77]
supersedes: none
---
## Do
- Add a file, folder or URL the work cites as a resource node, once:
  `umx node add --type document --locator docs/brief.md`. The source is a path relative to the graph folder,
  `root:repo/<path>` for elsewhere in the same git repo, `root:<name>/<path>` for a root this machine set, or an
  http(s) URL. A trailing `/` makes a folder. Without `--title` the title is the file or folder name.
- On `resource_exists`, use the node it returns: the graph already holds that source (a graph-folder path and
  its `root:repo/` form count as the same file). Nothing was added.
- On `resource_elsewhere`, link to the node it names in the other graph rather than keeping two.
- Link people, decisions and questions to the resource with ordinary edges (`made`, `shared`, `cites`).
- When a file moves, re-point its node: `umx node edit <node> --locator <new path>`. It keeps its id and edges.
  Read `locator_changed`: it says how many edges touch the node and how many evidence pointers cited the old
  source, so you can check them.
- Read a resource's locator with `umx resource show <node>`; filter with `--where locator_kind=folder` or
  `MATCH (n) WHERE n.locator_kind = 'file'`.

## Don't
- Don't pass an absolute path or `file:` URL: it is refused with `locator_absolute`, whose `next` shows the
  `root:` form. Don't put a user or password in a URL (`locator_invalid`).
- Don't add a second node for the same source to give it a new title: edit the existing one.
- Don't set `locator` or `locator_*` with `--set`: they are never custom fields (`locator_field_invalid`).
- Don't read `n.locator` in Cypher: it is null with `query_unknown_property`. Use the `locator_*` names.
- Don't expect umx to read or fetch the source yet: hash and length stay empty until `umx resource check` lands.

## Example
```bash
umx node add --type document --locator docs/brief.md                 # titled brief.md
umx node add --type document --locator ./docs//brief.md              # resource_exists: the same node back
umx edge add 01JD...PERSON shared 01JD...BRIEF
umx node edit 01JD...BRIEF --locator docs/brief-v2.md                # locator_changed: 1 edge, 0 evidence
umx resource show 01JD...BRIEF
```

## Why
Workshop 010 section 3: one resource node per locator, so every link and citation meets on one node. Committed
files never hold an absolute path, because it differs on every machine. Re-pointing keeps the node's history
and links when a file moves.

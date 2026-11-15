---
id: 013
title: Link to lines with a selector edge; promote to an excerpt only when needed
route: resources
status: planned
source: D35 (2026-11-10); workshop 010
reqs: [req-79, req-81]
supersedes: none
---
## Do
To say a node relates to specific lines, point the edge at the resource with a selector: `umx edge add <answer> answered_in <T>#seq=3206-3240`. Promote the passage to an excerpt node (`umx excerpt promote <T>#seq=...`) only when it needs its own notes, status or many links; promotion rewrites the existing edges.

## Don't
Don't make a node per transcript line. Don't promote every cited passage.

## Example
```bash
umx edge add <question> asked_in <T>#seq=1200-1210
```

## Why
Lines stay addressable without bloating the graph; excerpts appear only where they earn it.

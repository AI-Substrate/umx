---
id: 026
title: write_not_durable means the change was made; don't redo it
route: build
status: current
source: blueprint 001 E-5 (ap-13)
reqs: [req-01]
supersedes: none
---
## Do
Treat a `write_not_durable` warning as success: the change is in the file and in memory, but the folder sync after it failed, so a crash before the OS flushes could lose it. Carry on, and tell a person if it keeps happening (a disk or permission problem).

## Don't
Don't run the command again: a second `node add` makes a second node.

## Example
```bash
umx node add --type problem --title "Approval delays"   # status warning, write_not_durable
umx node list --type problem                             # the node is there
```

## Why
umx reports an error only when nothing changed (the GraphFiles contract), so a warning never hides a change that did not happen, and an error never hides one that did.

---
id: 195
title: Read flagged values in JSON; --human hides them
route: explore
status: current
source: F224 (the project manager 2026-11-13; security N1 on #389)
reqs: [req-153, req-46]
supersedes: none
---
## Do
Read `umx node get`, `umx node list`, `umx entity show`, `umx edge show` and `umx edge list` as JSON when you work with the values. The JSON reply keeps every value and names the flagged ones in `secret_suspected` (usage 174). With `--human`, umx prints `[secret suspected, hidden]` in place of each flagged value and keeps the `secret_suspected` list, so a person at a terminal sees where the secret is without seeing it.

On a node, and on an edge's evidence or a field umx names itself, only the flagged value is hidden. An edge custom field is flagged by its number (`edge:<id>/field 2`), and the number counts stored fields a reply may not show, so `--human` hides every value of that edge that umx did not write itself, its sentence and titles included. A flag this version of umx does not know hides the same way.

## Don't
Don't use `--human` output to decide what is safe to copy: it hides values, it does not clean them, and the JSON still holds them. Don't read a hidden edge value as missing: rerun the read as JSON to see which field is flagged, then tell the user which edge and field hold it.

## Example
```bash
umx node get 01JC8XA4H7 --human            # note: [secret suspected, hidden]   secret_suspected: field:note
umx edge list --columns all --human        # a row with an edge:<id>/field 2 flag shows only its ids and type
umx node get 01JC8XA4H7                    # JSON: the note as stored, and "secret_suspected": ["field:note"]
```

## Why
`--human` output is read by people and pasted into chats and tickets, where a value repeated in full beside its own flag is the easiest way for a secret to leave the machine. JSON is for agents and the graph view, which already act on the flags (usage 174). Hiding a whole flagged edge when the field cannot be placed exactly errs toward hiding too much, never too little.

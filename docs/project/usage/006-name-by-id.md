---
id: 006
title: Name nodes by id or address, never by title
route: build
status: current
source: verb grammar 007 rule 7; req-14
reqs: [req-14]
supersedes: none
---
## Do
Use the id printed by `node add`, a unique prefix of 6+ characters, or the address `umx:<alias>/<id>`. Get an id from words with `umx node find`.

## Don't
Don't use titles as references; titles change.

## Example
```bash
umx edge add 01JC8XA4H7 blocks 01JC8XB2K9
```

## Why
Ids are permanent; titles are not (D-addressing, req-14).

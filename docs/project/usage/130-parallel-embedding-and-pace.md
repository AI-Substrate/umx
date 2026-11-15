---
id: 130
title: Background embedding runs calls in parallel; pace it per instance on the machine
route: build
status: current
source: ws004 s25; embeddings-expert, security, architect and graph-storage-expert, 2026-11-11
reqs: [req-120, req-35]
supersedes: none
---
## Do
Expect a big backlog (a restart, a model change, a large `umx batch`) to embed several calls at once: up to the instance's concurrency ceiling less one slot kept for searches (31 for `azure_openai`; for `local`, its `sessions` less one, at least one). Searches stay quick during a drain: their query embed takes the kept slot.

To change how fast background embedding may send to an instance, set `embedders.<name>.tokens_per_minute` in the MACHINE config and restart the daemon (default 1500000 for `azure_openai`; a `local` instance is unpaced unless set). If two instances use one Azure resource, give them values that add up to less than its quota: each is paced on its own.

## Don't
Don't put `tokens_per_minute` in a repo's config: every `embedders.*` key is machine only, and a repo value is ignored with `config_repo_ignored`. Don't read a 429 in the daemon log as lost work: the instance pauses for the provider's `Retry-After`, halves its rate once, and its jobs come back; other instances go on.

## Example
```bash
umx config set embedders.azure_large.tokens_per_minute 1000000   # machine config
umx daemon restart
```

## Why
One call at a time left Azure's 32 concurrent calls unused, so embedding a real graph was slow. Calls now overlap, paced so umx takes only its share of a deployment other tools use too.

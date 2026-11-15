---
id: 076
title: config.toml, secrets.env and graph-policies.json over 64 KiB are refused, never read
route: doctor
status: current
source: F45 (security), the project manager 2026-11-10; PR for plan 043
reqs: [req-72]
supersedes: none
---
## Do
- If `config_file_refused` (or a refused secrets or state file in `umx doctor`) says "it is over 64 KiB", look at what wrote that file. A repo's `.umx/config.toml` arrives by git pull. Shorten it, or remove it and set again with `umx config set`.
- Keep umx's own files small: a config is a few `key = value` lines, and secrets are one `NAME=value` per line.

## Don't
- Don't split one huge file into many to get round the limit; umx reads only `$UMX_HOME/config.toml`, the repo's `.umx/config.toml`, `secrets.env` and `state/graph-policies.json`.
- Don't expect a partial read: an over-limit file is refused whole, like an untrusted one. A refused state file makes every graph count as secured.

## Example
```bash
umx doctor
umx config show
```

## Why
A file that arrives by git pull is not under the user's control; reading a multi-GB file whole would stall or exhaust the daemon (F45).

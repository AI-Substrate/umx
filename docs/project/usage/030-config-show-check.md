---
id: 030
title: Read and change umx's settings with umx config, never by guessing
route: doctor
status: current
source: workstream 002 s07 (req-72), design approved by architect and security 2026-11-10, PR for plan 016
reqs: [req-72]
supersedes: none
---
## Do
Run `umx config show` to see every setting, its value and where it came from (default, machine, repo, env,
flag). Run `umx config check` when something about providers, roots or secrets looks wrong, and run the
`next` it prints. Change a setting with `umx config set <key> <value>` (the machine file) or add `--repo` for
the repository you are in; remove one with `umx config unset <key>`. Name an embedder instance in snake_case:
`embedders.<name>.<key>`.

## Don't
Don't edit `$UMX_HOME/config.toml` or `<repo>/.umx/config.toml` by hand when a `set` or `unset` will do: umx
writes them in a fixed order and refuses to rewrite a file it cannot parse. Don't put a key or token in any
config value; name the env var in `api_key_env` and keep the value in the environment or
`$UMX_HOME/secrets.env` (owner-only, plain `NAME=value` lines). Don't try to make a repo config choose a
hosted embedder, change an endpoint or widen roots: the repo layer may only set `embedder.active` to `none` or
a local instance, and anything else is ignored with `config_repo_ignored`. Don't read another tool's config
(Flowspace or others) to fill umx's; copy the values with `umx config set`.

## Example
```bash
umx config show
umx config set embedders.nomic.kind local
umx config set embedders.nomic.model nomic-embed-text-v1.5
umx config set embedders.nomic.dims 256
umx config set embedder.active nomic
umx config set embedder.active none --repo
umx config check
```

## Why
One typed list of settings, with sources shown and problems named with runnable fixes, means an agent never
has to reverse engineer where a value came from. Secrets stay out of files and envelopes, and a cloned
repository cannot redirect graph text to another endpoint.

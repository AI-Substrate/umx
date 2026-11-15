---
id: 061
title: Set up an embedder in the machine config; az login is the default sign-in
route: doctor
status: current
source: req-73 (D27a, D27b, D27c); workstream 004 s05, architect ruling 2026-11-10 (instances built once at daemon start, with no I/O)
reqs: [req-72, req-73, req-46]
supersedes: none
---
## Do
Name an embedder instance in the machine config with `umx config set embedders.<name>.<key>`, choose it with
`umx config set embedder.active <name>`, then restart the daemon so it builds the instance. For Azure OpenAI the
sign-in is `az login` (`auth = cli`, the default): sign in with the Azure CLI first, and umx asks `az` (then `azd`)
for a token on first use. The token is never shown or stored. To use an api key instead, set `auth api_key` and
put the NAME of the env var that holds it in `api_key_env`; the value goes in the environment or in
`$UMX_HOME/secrets.env` (a `NAME=value` line, chmod 600).

Instances are built, and the machine's `embedder.active` is wired, once when the daemon starts. So any change to
`embedder.active` or an `embedders.*` key in the machine config warns `config_restart_needed` and takes effect only
after `umx daemon stop`, then `umx daemon start`. A repository's `.umx/config.toml` is different: setting
`embedder.active` there to `none` or a local instance narrows the graphs in that repository, read per graph, with
no restart. `umx embeddings probe` reads the config as it is now, so it can check a choice before the restart.
`umx daemon status` shows the active instance, the credential that last worked (`az` and its path, or `api_key`
and the env var name) and where `az` and `azd` were found.

## Don't
Don't put a key or token in config.toml or in a repo config: umx refuses it. Don't copy another tool's config or
secrets file; set the values with `umx config set`. Don't name a deployment or endpoint in a graph: a graph records
only its model key (note 057). Don't leave a stale `AZURE_OPENAI_API_KEY` around and expect `az login` to win: with
`auth api_key` the key is what umx sends.

## Example
```bash
az login
umx config set embedders.azure_small.kind azure_openai
umx config set embedders.azure_small.endpoint https://<resource>.openai.azure.com
umx config set embedders.azure_small.deployment <deployment>
umx config set embedders.azure_small.model text-embedding-3-small
umx config set embedders.azure_small.dims 256
umx config set embedder.active azure_small
umx daemon stop          # embedder.active and embedders.* apply at the next start
umx daemon start
umx embeddings probe
```

## Why
The project's rule (D27a): Azure through the Azure CLI by default, keys allowed but never the default, and the
credential explicit in config. Building every instance at start means no query ever waits on construction, and a
broken instance is reported (`umx daemon status`, `umx embeddings probe`) instead of stopping the daemon.

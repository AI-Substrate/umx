---
id: 120
title: Probe a new embedder without a restart, copy flowspace3's embedder with config import, read embeddings status
route: doctor
status: current
source: workstream 004 s27 (Azure rollout frictions, 2026-11-11); architect ruling 2026-11-11 (Op::EmbeddingsStatus, Op::ConfigImport via ConfigFile::Flowspace3); project lead rule 2026-11-11 (tell a graph's owner before changing its settings)
reqs: [req-73, req-46, req-53]
supersedes: 062
---
## Do
Check an embedder with `umx embeddings probe`. It first checks the embedder can run without sending any text (for
Azure: gets an `az login` token, or checks the key was found), then embeds one fixed sentence umx owns, never
anything from a graph. It prints the instance, the model key (`model@dims`), `dims`, `norm` (about 1),
`latency_ms`, `locality` and the credential that worked. `--embedder <name>` tries another instance;
`--graph <alias>` checks the instance that graph would use (its folder's repo config and its effective policy
apply).

You can probe an instance you just added or changed with `umx config set`, before any restart: the probe builds it
from the config as it is now, uses it once and drops it, and warns `embedder_not_in_use`. That warning means it
works but the daemon still runs what it started with; `umx daemon restart` makes the daemon use it.

To set up the embedder flowspace3 already uses, copy it instead of retyping it:
`umx config import --from flowspace3 --dry-run` shows the keys it would set, then run it without `--dry-run`. It
copies only `kind`, `endpoint`, `deployment`, `model`, `dims`, `api_version` and `auth = "cli"` (az login) into
the machine config as one instance (flowspace3's provider name in snake_case, or `--as <name>`). If it says the
model cannot be told from the deployment name, pass `--model <model>`; `--dims <n>` sets the width (for example
256). The reply's `key` is `model@dims`; it warns `import_key_unmatched` when no loaded graph records that key.
Follow the `next` it prints as it is: it keeps your `--as`, `--model`, `--dims` and `--dry-run`. Then probe it with
`--embedder <name>`. The imported instance is in the machine config, so it is trusted (D62): once the daemon uses
it, a graph whose embedder setting is `trusted` may send its text to it. Tell the owners of such graphs.

`--graph <alias>` without `--embedder` probes the instance the running daemon uses for that graph, as built at
start. After changing that instance's config, test the new settings with `umx embeddings probe --embedder <name>`.

To see how far embedding has got, run `umx embeddings status`: per loaded graph, `vectors_pending`,
`vectors_held`, `vectors_done`, the `embedder` serving it (or `null`) and `state`. It sends nothing; each row's
`check` is the probe command that tests whether that embedder answers. `state: unknown` means its shards could not
be read at load (counts `null`); run `umx embeddings repair --graph <alias>`. `umx graph list` shows the same counts.

Before you switch `embedder.active` to a new instance, check that the probe's `key` equals each graph's recorded
`key` in `umx embeddings status`. A graph whose key differs does not use that instance; it waits, pending, until
someone reembeds it.

Before you change a graph's settings (its embedder through `embedder.active`, policy or visibility, a reembed, its
model, field roles or similarity floor), tell the graph's owner in one line first and give them a moment to object.

## Don't
Don't expect a probe of a new instance to change what the daemon embeds with: only a restart does. Don't use the
probe to test what a graph's text embeds to; it never sends graph text. Don't expect it to pick a hosted instance
for a secured graph; it refuses (`embedder_unavailable`). Don't retry it in a loop on `embedder_unavailable`; fix
what the message names first.

Don't copy flowspace3's keys or `secrets.env` by hand: `config import` never reads them, and umx signs in with
`az login`. Don't pass `--force` unless you mean to replace an instance that is already defined
(`config_import_exists`). `config import` does not switch `embedder.active`; switching it changes which embedder
every graph without a recorded model uses, so tell the graphs' owners first.

Don't poll `umx embeddings status` in a tight loop; once every few seconds is plenty.

## Example
```bash
umx config import --from flowspace3 --dry-run
umx config import --from flowspace3
umx embeddings probe --embedder azure_embed     # warns embedder_not_in_use: works, restart to use it
umx config set embedder.active azure_embed      # after telling each graph's owner
umx daemon restart
umx embeddings status
```

## Why
The Azure rollout (2026-11-11) hit three frictions: the probe refused an instance added before the restart, the
embedder was typed in by hand from flowspace3's config, and agents reached for `umx embeddings status`, which did
not exist. umx copies config and never shares it (D27), so import is a one-time copy, and it never reads another
tool's secrets.

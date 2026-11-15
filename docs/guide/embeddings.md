# Embeddings

Embeddings let umx match meaning, not only words: `umx search "sign-off is slow"` finds a node titled "Approval
delays", and every node gets `similar_to` edges to the nodes most like it. They are optional. Without them
everything else works, and search uses exact values and words.

The commands are in `umx docs get embeddings`, `umx docs get config` and `umx docs get models`. This page is about
choosing and running an embedder.

## How it works

When the daemon has an embedder, every node gets vectors: one for the whole node and one for each text field (the
title, the note, and each field its type declares as a string). Numbers, dates, booleans and lists get none.

- **After the write.** A write returns as soon as the node is on disk. Its vectors are made afterwards in the
  background. `umx graph list` shows `vectors_pending` falling to 0 as they land.
- **Only what changed.** A vector stores a hash of its text. A field whose text did not change is never embedded
  again, so re-saving a node, or loading a graph whose vectors a teammate committed, costs nothing.
- **In the graph folder.** Vectors live in `embeddings/<model@dims>/<shard>.tsv` and the `similar_to` lists in
  `similar/<model@dims>/`. Commit them with the graph. A graph that is not fully open (named, trusted or local) keeps both
  in a private cache on this machine instead (below).
- **One model per graph.** A graph records its model in `graph.json` as a key, `model@dims`, with `+p1` when the
  model uses a prompt prefix: `embeddinggemma-2@256+p1`, `text-embedding-3-small@256`. Vectors from two models are
  never mixed or compared.

## Choosing an embedder

| | Local: EmbeddingGemma 2 | Azure OpenAI |
|---|---|---|
| Where text goes | Nowhere: llama.cpp inside the daemon | Your Azure OpenAI resource |
| Graphs set to `local` (and secured) | Yes | No: their text never leaves the machine |
| Graphs set to `trusted` | Yes | Yes, when the instance is in this machine's config |
| Setup | `umx models fetch` once (about 558 MB) | An Azure OpenAI deployment and `az login` |
| Speed | About 70 texts a second on a Mac (Metal); about 3 a second on the CPU | Depends on your deployment's rate limits |
| Memory | About 1 GB in the daemon while loaded | None locally |
| Cost | Your machine | Tokens on your Azure bill |
| Key | `embeddinggemma-2@256+p1` | for example `text-embedding-3-small@256` |

Use the local model for client or private graphs, and on any machine where text must not leave. Use Azure when
you want a hosted model and the graph may go there. A machine can have both: each graph uses the instance that
matches its own model.

EmbeddingGemma 2 was chosen in a bake-off of open models for local use (BEIR retrieval benchmarks at 256 dims; it
led, and losing dims from 768 to 256 cost it least). The results are in
`docs/project/benchmarks/embed-bakeoff/RESULT.md`.

### Why 256 dimensions

Vectors are stored as 256 one-byte numbers each. That keeps the vector files small enough to commit and merge, and
memory to about 1 KB a node. For EmbeddingGemma 2, going from 768 to 256 dims lowered retrieval quality by under
two points in the bake-off. Use 256 unless you have measured a reason not to. The local model supports 768, 512,
256 and 128; Azure's text-embedding-3 models take the width you configure.

## Setting up the local embedder

    umx models fetch embeddinggemma-2 --dry-run    # what it will download, and the space it needs
    umx models fetch embeddinggemma-2              # about 558 MB, checked by sha256
    umx config set embedders.gemma.kind local
    umx config set embedders.gemma.model embeddinggemma-2
    umx config set embedders.gemma.dims 256
    umx config set embedder.active gemma
    umx daemon restart
    umx embeddings probe

`gemma` is a name you choose for the instance. The model goes to `~/.cache/umx/models/` (or under
`$XDG_CACHE_HOME`) and is checked by sha256 every time it loads. umx never downloads a model while it embeds; only
`umx models fetch` does.

On a Mac it runs on the GPU through Metal. Without a GPU (Linux, a VM, or `embedders.gemma.execution cpu`) it runs
on the CPU at about 3 texts a second: fine for search and small graphs, slow for a big import. `umx doctor` says
which it uses. More threads help on the CPU: `umx config set embedders.gemma.threads 8`.

The model loads on first use and stays in memory while the daemon runs. The first search after a restart may
answer with words only while it loads (`similarity_unavailable`, "still loading"); search again in a moment.

## Setting up Azure OpenAI

You need an Azure OpenAI resource with an embedding deployment, and the Azure CLI signed in:

    az login
    umx config set embedders.azure_small.kind azure_openai
    umx config set embedders.azure_small.endpoint https://<resource>.openai.azure.com
    umx config set embedders.azure_small.deployment <deployment name>
    umx config set embedders.azure_small.model text-embedding-3-small
    umx config set embedders.azure_small.dims 256
    umx config set embedder.active azure_small
    umx daemon restart
    umx embeddings probe

`deployment` is the deployment's name in your resource, not the model's name. umx signs in through `az` (then
`azd`), found on `PATH` when the daemon starts, and keeps the token until shortly before it expires.
`umx daemon status` shows where it found `az` and `azd` and which credential last worked.

If you must use a key instead:

    umx config set embedders.azure_small.auth api_key
    umx config set embedders.azure_small.api_key_env AZURE_OPENAI_API_KEY

The config holds only the variable's name. Put the key in the environment the daemon starts with, or in
`~/.umx/secrets.env` (`AZURE_OPENAI_API_KEY=...`, mode 600). umx never prints it.

## Checking it works

    umx embeddings probe                  # the active instance
    umx embeddings probe --graph launch   # the instance that graph would use
    umx doctor

The probe embeds one fixed sentence of umx's own, never graph text, and prints the model key, `dims`,
`latency_ms`, `locality` (`local` or `hosted`) and the credential that worked. Its errors say what to do:
`embedder_not_configured` (no instance chosen), `embedder_unavailable` (no `az login`, a rejected key, the service
cannot be reached or is rate limiting), `embedder_invalid` (the instance's config is wrong: run `umx config check`).

## Graphs and models

Give a graph its model when you create it, so every machine embeds it the same way:

    umx graph new ~/graphs/launch --alias launch --embedding-model embeddinggemma-2@256+p1

A graph that records no model is embedded with `embedder.active`, and records nothing until you re-embed it.

When a pulled graph records a different model from the one this machine has, umx warns `embedder_mismatch`, keeps
using the committed vectors for search, and leaves new and edited nodes pending. Either set up an instance for the
graph's model, or move the graph to yours on purpose:

    umx embeddings reembed --graph launch --dry-run     # nodes, texts and estimated tokens; sends nothing
    umx embeddings reembed --graph launch

The old model's vectors stay in their own folder, so switching back costs nothing. Agree a model with the people
who share a graph before re-embedding it: it changes `graph.json` and every vector file.

A repository can narrow the embedder for the graphs inside it, in `<repo>/.umx/config.toml`:

    umx config set embedder.active none --repo      # no embedding for graphs in this repository

A repository file may only choose `none` or a local instance. It can never switch a graph to a hosted embedder.

## Secured, trusted and local graphs

A graph's `embedder` setting says which instances may get its text: `any`, `trusted` (any instance in this
machine's config, local or hosted) or `local`. A secured graph (`umx graph new ... --secured`, or
`umx graph set <alias> --policy secured`) is `local`: it is only embedded by a local instance. With a hosted one it
gets no vectors and warns `embedder_not_local_for_secured`; its word search still works. A `trusted` graph may use a
hosted instance from this machine's config. Any graph that is not fully open (visibility `open` and embedder `any`)
keeps its vectors and `similar_to` lists in a private cache under `~/.umx/vectors/`, never in the graph folder, so
they are never committed. A teammate who clones such a graph
embeds it again on their own machine.

## Secrets in text

Before anything goes to a hosted embedder, umx scans each text for things that look like credentials: cloud and
API keys, private keys, tokens, passwords in URLs. A hit never stops the write. It warns `secret_suspected`,
naming the node, the field and the rule (never the text), and holds that field (and the whole-node vector) back
from hosted embedders. `umx graph list` counts these nodes in `vectors_held`.

Redact the text, or, if it is not a secret, allow that exact text on this machine:

    umx node edit <node> --note "the note without the key"
    umx embeddings allow --node <node> --field note

A search query that looks like a secret is not sent to a hosted embedder either.

## Floors and similar_to

Vector matches below a floor are dropped from search and from `similar_to`. The defaults are a cosine of 0.60
for `embeddinggemma-2@256+p1` and 0.50 for other models. None is calibrated yet: they are starting points, and
`umx search` reports the floor it used in `floors`.

Each node keeps `similar_to` edges to its top 10 matches at or above the floor. A graph can change both in
`graph.json`:

    "embedding": { ..., "similarity": { "k": 10, "floor": 0.65 } }

Read them by naming the relation: `umx neighbours <node> --relation similar_to`. They are a hint that two nodes
may be about the same thing, for a person or agent to check, never a link that says they are. umx keeps them
current as vectors change; after a merge, or if they look wrong, `umx similarity rebuild --graph <alias>`.

## After a merge or pull

    umx embeddings repair --graph <alias>

tidies the vector files (two branches embedding the same text, lines for fields a node no longer has) and queues
any node whose vector is missing, stale or conflicting. Lines umx cannot read, such as a merge marker, are kept and
reported (`vector_line_kept`) for a person to remove.

## Costs

- A hosted embedder bills per token. `umx embeddings reembed --dry-run` estimates the tokens before you commit to a
  re-embed. Day to day, only new and changed text is embedded.
- Background embedding waits when the provider says it is busy, and never drops a node for it.
- The local model costs about 1 GB of memory per instance while the daemon runs. Define only the instances your
  graphs use.

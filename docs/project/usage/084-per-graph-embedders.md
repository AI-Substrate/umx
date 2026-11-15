---
id: 084
title: Each graph embeds with the instance of its own model; embedder.active is only the default
route: doctor
status: current
source: embeddings-expert ruling 2026-11-10 19:15; workstream 004 s17 (plan 047)
reqs: [req-73, req-75, req-46]
supersedes: none
---
## Do
Define one embedder instance per model you use (`umx config set embedders.<name>.*`), then restart the daemon: it
builds and keeps every instance. Each graph embeds with the instance whose model and dims match the key the graph
records (`umx graph new --embedding-model <model@dims>`, note 057), and that its policy allows: a secured graph only
ever uses a local instance. When several match, `embedder.active` wins, then the instance name in sort order.
`embedder.active` is the default: it serves graphs that record no model, and it is what `umx embeddings reembed`
moves a graph to. A repository's `.umx/config.toml` that sets `embedder.active` allows only the instance it names
(`none` allows none; `none` is never an instance name, so `embedders.none.*` keys are `config_key_unknown`) for its
graphs, even when it names the same value as the machine. If it names anything else (an instance this machine does
not define, or a hosted one), umx ignores the value with `config_repo_ignored` and its graphs use no instance until it
is fixed. `umx embeddings probe --graph <alias>` probes the instance that graph uses.

A `local` instance runs a model umx ships on this machine (llama.cpp; today `embeddinggemma-2`, dims 768, 512, 256
or 128; its key is `embeddinggemma-2@<dims>+p1`). Its file lives in `$XDG_CACHE_HOME/umx/models` (else
`~/.cache/umx/models`) and is checked by sha256 before every load; embedding never downloads it, `umx models fetch`
does. `umx doctor` names what runs it (`metal` or `cpu`); on the CPU it notes `doctor_local_embedder_cpu`, or warns
`doctor_local_embedder_slow` under the CPU floor (note 085). A secured graph needs a local instance to embed at all; one that
records no model uses the default if it is local, else the first local instance by name.

Each local instance loads its model on first use and keeps it in memory while the daemon runs: about 1 GB of RSS for
`embeddinggemma-2` per instance (and per session). Define only the local instances your graphs use.

## Don't
Don't switch `embedder.active` to change one graph's model: the graph keeps its recorded model. Add an instance for
that model, or reembed the graph. Don't expect a graph whose model no instance makes to embed: it warns
`embedder_mismatch` and its nodes stay pending. A repository whose `.umx/config.toml` umx refuses or cannot parse
counts as `embedder.active = "none"` for its graphs until it is fixed (`umx config check`): nothing of theirs is sent.

## Example
```bash
umx config set embedders.small.kind azure_openai
umx config set embedders.small.model text-embedding-3-small
umx config set embedders.small.dims 256
umx config set embedders.large.kind azure_openai
umx config set embedders.large.model text-embedding-3-large
umx config set embedders.large.dims 256
umx config set embedders.gemma.kind local
umx config set embedders.gemma.model embeddinggemma-2
umx config set embedders.gemma.dims 256
umx daemon stop
umx daemon start
umx embeddings probe --graph launch
umx doctor
```

## Why
Vectors are only comparable within one model and width (req-75). Choosing the instance from the graph's recorded key
lets graphs on different models live in one daemon without mixing, and keeps a secured graph's text on the machine
whatever the default is (req-46).

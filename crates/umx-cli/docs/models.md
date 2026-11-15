# Models

The local embedder (`kind = "local"`) reads a model file from the models folder on this machine. umx never
downloads a model while it embeds: you fetch it once, on purpose.

    umx models fetch embeddinggemma-2 --dry-run
    umx models fetch embeddinggemma-2

`--dry-run` shows the repo, the commit, each file with its size and sha256, the folder, the bytes still `needed`
and the space `available` there, and fetches nothing. No daemon is needed.

## What a fetch does

- It downloads each file of the model pinned in umx (a Hugging Face repo at a fixed commit) over https from
  huggingface.co and its download hosts only, sending no token or credential.
- It checks the size and the sha256 before the file is kept: a file that does not match is refused
  (`model_hash_mismatch`) and nothing is left under its name.
- It writes into `<models>/<org>--<repo>/<commit>/`, the folder 0700 and each file 0600. The models folder is
  `$XDG_CACHE_HOME/umx/models`, or `~/.cache/umx/models`.
- A file already there with the right sha256 is not downloaded again, so a second fetch is quick.
- It checks there is room first (`model_disk_space`, naming what it needs).
- It holds the models folder's lock (`.models.lock`) while it writes, so two fetches at once take turns: the
  second finds the file there and downloads nothing.

Progress, when stderr is a terminal, gives sizes only. A fetch that stops (`model_fetch_failed`) names the file,
the step (connect, redirect, status, size, write, disk, path) and the host, never a URL.

## Where it runs

`embedders.<name>.execution` is `gpu` by default where umx has a GPU backend, else `cpu`. The model, its file and
its key (`embeddinggemma-2@256+p1`) are the same everywhere, so nothing re-embeds when a graph moves machines.

- macOS: Metal, about 70 texts a second.
- Linux with an NVIDIA GPU: only in a umx built with `cargo build -p umx --release --locked --features cuda`, which
  needs the CUDA toolkit (set `CMAKE_CUDA_ARCHITECTURES`, such as 89 for an RTX 4090, to compile for one card). The
  feature is the only switch and nothing is downloaded at build time. If that build finds no CUDA device (no driver,
  no GPU in the container) it still starts and runs on the CPU, and `umx doctor` says
  `cpu: this cuda build found no CUDA device`.
- Anywhere else, or with `execution = "cpu"`: the CPU, from an exact F32 copy of the file that umx makes beside it
  once (about 1.1 GB), at about 3 texts a second with 4 threads.

## The models

- `embeddinggemma-2`: EmbeddingGemma 2 (Apache 2.0), ggml-org's BF16 GGUF, about 558 MB. The local embedder's model
  (`embedders.<name>.kind = "local"`, model `embeddinggemma-2`); see `umx docs get config`.

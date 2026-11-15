---
id: 086
title: Fetch the local embedder's model once with umx models fetch; embedding never downloads
route: start
status: current
source: ws004 s14 (plan 050); architect and security rulings on s14 (2026-11-10)
reqs: [req-74, req-46]
supersedes: 074
---
## Do
Use the local embedder for secured graphs (note 051). It is EmbeddingGemma 2, the BF16 GGUF file from
ggml-org/embeddinggemma-2-GGUF at a commit pinned in umx, run by llama.cpp inside umx. Its key is
`embeddinggemma-2@256+p1`; use that key when you create a graph that will embed locally (note 057).

Put the model on the machine first, once, when the human asks for local embedding: `umx models fetch
embeddinggemma-2`. It needs no daemon. Check first with `--dry-run`, which prints the repo, the commit, each file's
size and sha256, the folder and the free space, and fetches nothing. The file is 558 MB and lands in
`$XDG_CACHE_HOME/umx/models` (or `~/.cache/umx/models`) under `ggml-org--embeddinggemma-2-GGUF/<commit>/`. The fetch
talks https to huggingface.co and its download hosts only, sends no token, and keeps a file only when its size and
sha256 match the pin. Running it again is cheap: a file already there with the right sha256 is not downloaded.

When `embed` or `ready` says the model is missing, its `next` names this command. umx checks the file's sha256 before
every load and refuses a different file by name. llama.cpp then opens the file again by path, so keep the models
folder your own: a file swapped in the moment between the check and that open is not caught (an accepted window,
architect C33).

Embed locally on a Mac: Metal runs it at about 70 texts a second with about 1 GB of memory.

## Don't
Don't fetch a model the human did not ask for: it is a large download. Don't copy a model file in by hand or point
umx at another folder to skip the check; a wrong file is refused at load anyway.

On `model_disk_space`, free the space it names; don't delete other caches to make room without asking. On
`model_hash_mismatch`, nothing was kept: fetch again, and if it repeats, stop and tell the human (the download is not
the pinned file). On `model_fetch_failed`, read the step and host it names (connect, redirect, status, size, write,
disk, path); a `path` step means the models folder or a file in it is a symlink or not yours.

CPU-only local embedding works since ws004 s16, at about 3 texts a second; note 085 says what to expect and what
`doctor_local_embedder_cpu` and `doctor_local_embedder_slow` mean.

Don't swap in another quantisation (Q8_0, Q4) of the model under the same key: it gives other vectors.

## Example
```bash
umx models fetch embeddinggemma-2 --dry-run
umx models fetch embeddinggemma-2
umx graph new ./graphs/clients --alias clients --embedding-model embeddinggemma-2@256+p1
```

## Why
Secured text must never leave the machine (req-46), so the model has to be local, and embedding must never reach the
network on its own: the fetch is a separate, deliberate step. Pinning the commit, the size and the sha256 means every
machine runs the same bytes and gets the same vectors. Writing to a temp file and renaming only after the sha256
matches means a failed or interrupted fetch never leaves a file the embedder would load.

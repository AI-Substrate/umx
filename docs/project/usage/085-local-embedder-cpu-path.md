---
id: 085
title: The local embedder is embeddinggemma-2 on llama.cpp; Metal is fast, and CPU-only now works at about 3 texts a second
route: start
status: current
source: ws004 s16 (plan 051); embeddings-expert ruling 2026-11-10 (CPU floor); architect ruling 2026-11-10 (F32 path, no build change); decisions D44 and D46
reqs: [req-74, req-46]
supersedes: 074
---
## Do
Use the local embedder for secured graphs (note 051). It is EmbeddingGemma 2, the BF16 GGUF file from
ggml-org/embeddinggemma-2-GGUF at a pinned commit, run by llama.cpp inside umx. Its key is
`embeddinggemma-2@256+p1` on every machine, GPU or CPU; use that key when you create a graph that will embed locally
(note 057).

Put the model file on the machine first, once, with `umx models fetch embeddinggemma-2` (note 086). It downloads
the 558 MB BF16 file into `~/.cache/umx/models/ggml-org--embeddinggemma-2-GGUF/<commit>/`. umx checks the file's sha256 before every load and refuses a different file by name. llama.cpp then opens the file
again by path, so keep the models folder your own (an accepted window, architect C33).

On a Mac, Metal runs it at about 70 texts a second with about 1 GB of memory.

With no GPU (Linux, a VM, a Mac with Metal off), umx runs it on the CPU from an F32 copy of the model file, which it
makes itself beside the BF16 file the first time it needs it: about 4 seconds, 1.1 GB more on disk, mode 0600, pinned
by its own sha256. The CPU path embeds about 3 texts of 300 tokens a second at 4 threads (queries in about 40 ms),
with about 1.3 GB of memory. That is enough for small graphs and for search; a large ingest takes hours. `umx doctor`
shows the backend `cpu` and the note `doctor_local_embedder_cpu` with the rate (exit 0). To go faster, raise the
instance's threads (`umx config set embedders.<name>.threads 8`) or embed on a Mac.

## Don't
Don't delete the F32 file to save space on a CPU-only machine: umx makes it again on the next load. On a GPU machine it
is never made. If umx says `model_disk_space`, free the space it names (about 1.2 GB) or embed on a GPU; nothing was
written. If it says the F32 file has the wrong sha256, delete that file; umx makes it again from the BF16 file.

Don't treat `doctor_local_embedder_slow` as normal: it now means the CPU path is under its floor of 2 texts a second
(fewer than 3 threads, a model with no CPU file, or a slow or overloaded machine). Raise threads, or embed on a GPU.

Don't swap in another quantisation (Q8_0, Q4) of the model under the same key: it gives other vectors, so it would be
another model name, and Q8_0 also misses the parity gate. The F32 copy is not another quantisation: it holds exactly
the same weights, and its vectors match the BF16 ones (cosine 0.99998), so it keeps the key and nothing re-embeds.

## Example
```bash
umx models fetch embeddinggemma-2
umx graph new ./graphs/clients --alias clients --embedding-model embeddinggemma-2@256+p1
```

## Why
Secured text must never leave the machine (req-46), so the local embedder has to be usable wherever umx runs.
llama.cpp's CPU kernels run BF16 weights about 16 times slower than F32 ones (0.2 against about 3 texts a second), and
no F32 file is published, so umx widens the BF16 file, which is exact. Faster CPU kernels (KleidiAI, Accelerate)
would need build changes the architect declined (ws004 s16); the CPU floor is 2 texts a second, Metal's stays 10.

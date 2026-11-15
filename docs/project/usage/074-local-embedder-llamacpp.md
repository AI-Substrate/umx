---
id: 074
title: The local embedder is embeddinggemma-2 on llama.cpp; it is fast on a Mac (Metal) and not usable CPU-only
route: similarity
status: superseded
source: ws004 s04 (PR to come), D44 and D46 (2026-11-10), embeddings-expert and project manager rulings 2026-11-10
reqs: [req-74, req-46]
supersedes: none
superseded_by: 086, 085
---
## Do
Use the local embedder for secured graphs (note 051). It is EmbeddingGemma 2, the BF16 GGUF file from
ggml-org/embeddinggemma-2-GGUF at a pinned commit, run by llama.cpp inside umx. Its key is
`embeddinggemma-2@256+p1`; use that key when you create a graph that will embed locally (note 057).

Put the model file on the machine first, once: until umx can fetch models itself (ws004 s14), run
`tools/embed-bakeoff/fetch.sh gguf` from a unimatrix checkout. It downloads the 558 MB file into
`~/.cache/umx/models/ggml-org--embeddinggemma-2-GGUF/<commit>/` over plain https, without a Hugging Face token.
umx checks the file's sha256 before every load and refuses a different file by name. llama.cpp then opens the file
again by path, so keep the models folder your own: a file swapped in the moment between the check and that open is
not caught (an accepted window, architect C33).

Embed locally on a Mac: Metal runs it at about 70 texts a second with about 1 GB of memory.

## Don't
Don't rely on CPU-only local embedding. llama.cpp's CPU path runs this model at about 0.2 texts a second on Apple
silicon, far under the floor of 10, so Linux without a GPU is not a supported embedding setup until the follow-up
(ws004 s04b) fixes it. CUDA on Linux is the GPU route there, opt in, and not built yet. When the embedder runs
CPU-only, umx says so with the warning `local_embedder_slow`, naming the backend.

Don't swap in another quantisation (Q8_0, Q4) of the model under the same key: it gives other vectors, so it would be
another model name, and Q8_0 also misses the parity gate.

## Example
```bash
tools/embed-bakeoff/fetch.sh gguf
umx graph new ./graphs/clients --alias clients --embedding-model embeddinggemma-2@256+p1
```

## Why
Secured text must never leave the machine (req-46), so the local embedder has to be good enough to use. The bake-off
(ws004 s03) and the GPU spike picked this model and runtime: on Metal it is about 4.5 times faster than the CPU ONNX
path, at about a seventieth of the CPU time and a third of the memory, with the same vectors (parity 0.9998 against
the reference). Its CPU-only path is the known gap.

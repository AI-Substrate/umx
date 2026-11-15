---
id: 102
title: On Linux with an NVIDIA GPU, build umx with the cuda feature; doctor shows cuda, and with no device it runs on the CPU
route: start
status: current
source: ws004 s19 (plan 069); D46 (CUDA as an opt-in feature); architect ruling 2026-11-11 (feature chain, r-gate-features)
reqs: [req-74, req-46]
supersedes: none
---
## Do
Only on Linux with an NVIDIA GPU and the CUDA toolkit installed (nvcc on the PATH), build umx with its `cuda` feature:

```bash
cargo build -p umx --release --locked --features cuda
```

`umx docs get models` and `umx docs get doctor` say the same. The feature is the only switch: nothing reads an env var to turn CUDA on, and nothing is downloaded at build time.
Set `CMAKE_CUDA_ARCHITECTURES` to your card (89 for an RTX 4090) to compile llama.cpp for that card only, which is
much faster; leaving it unset builds llama.cpp's own list of architectures. It changes the compile, not the vectors.

The local embedder then runs on the GPU by default (`embedders.<name>.execution` is `gpu`), from the same BF16 file
under the same key `embeddinggemma-2@256+p1`, so graphs embedded on a Mac or on the CPU need no re-embedding. Fetch the
model as usual (note 086). `umx doctor` shows the embedder row as `embeddinggemma-2@256+p1 (cuda)`.

If the cuda build finds no CUDA device (no NVIDIA driver, `CUDA_VISIBLE_DEVICES=-1`, a container started without the
GPU), it still starts and runs on the CPU from the F32 copy (note 085). Doctor then shows
`embeddinggemma-2@256+p1 (cpu: this cuda build found no CUDA device)` and the `doctor_local_embedder_cpu` note. Fix the
driver or the device and restart the daemon to get the GPU back.

## Don't
Don't build with `--features cuda` on a Mac or on a machine without the CUDA toolkit: the build fails in llama.cpp's
cmake step. macOS uses Metal without any feature.

Don't pass `--all-features` to cargo in this repo: it turns on `cuda`. `just clippy` builds the list in
`crates/umx-arch/rules.toml` (r-gate-features); `just check-cuda` lints and tests the cuda build on a Linux machine
with the toolkit.

Don't set `GGML_CUDA_CCCL_VERSION` (or other `GGML_*` env vars) when you build: llama.cpp's build reads them, and
that one makes it download NVIDIA's CCCL from GitHub during the build.

Don't treat `cpu: this cuda build found no CUDA device` as a slow GPU: nothing ran on the GPU. Check `nvidia-smi`.

## Example
```bash
CMAKE_CUDA_ARCHITECTURES=89 cargo build -p umx --release --locked --features cuda
umx models fetch embeddinggemma-2
umx daemon start
umx doctor
```

## Why
On the RTX 4090 the cuda build embeds about 185 texts of 300 tokens a second, queries in under 5 ms at p95, with
about 1.85 GB of VRAM and 1.4 GB of memory, against about 4 texts a second on the CPU; its vectors match Metal's and
the CPU's (cosine 0.9999 or better) and the s03 reference (0.9997) (plan 069, assets/RESULT.md). CUDA stays opt-in (D46): it needs NVIDIA's toolkit to build and links NVIDIA's
static libraries, which the default Mac and Linux builds must not depend on. It links `cuda-no-vmm`, so the driver is
loaded at run time and a cuda build still starts on a machine without one.

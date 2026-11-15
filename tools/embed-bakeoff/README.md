# embed-bakeoff

Workstream 004 step s03: which local embedding model secured graphs use. Result:
`docs/project/benchmarks/embed-bakeoff/RESULT.md`.

This crate is its own workspace and is listed in the root `Cargo.toml` `exclude`. No umx crate
depends on it and `just check` never builds it, so ort and tokenizers stay out of umx's dependency
rules (architect ruling 3, s03 packet).

## Two paths

- **Reference** (`beir_eval.py`, Python, sentence-transformers): quality on BEIR SciFact, NFCorpus and
  ArguAna and on the 300-node fixture, at 256 and native dims. Runs in a throwaway venv.
- **Rust** (`src/main.rs`, ort + tokenizers): the path s04 would ship. Parity against the reference
  vectors, batch invariance, the fixture, and speed and memory in the shipped config.

## Run

```sh
# Rust path
tools/embed-bakeoff/fetch.sh                          # ONNX exports at pinned commits, plain curl, no HF token
tools/embed-bakeoff/fetch.sh gguf                     # the embeddinggemma-2 GGUF umx-adapter-llamacpp loads (until s14)
python tools/embed-bakeoff/widen_f32.py <BF16.gguf> <F32.gguf>  # gguf-py's F32 widening: the second origin for the adapter's own (s16)
tools/embed-bakeoff/locked.sh build                   # release build in a tools/cargo-slot slot (D41)
tools/embed-bakeoff/run-ort.sh embeddinggemma-2 qwen3-embedding-0.6b   # check, parity, quality, speed

# Reference path (needs results/parity-ref-*.json before run-ort.sh's parity step)
python3 -m venv /tmp/bakeoff-venv && /tmp/bakeoff-venv/bin/pip install --require-hashes -r tools/embed-bakeoff/requirements.txt
export BAKEOFF_PY=/tmp/bakeoff-venv/bin/python
tools/embed-bakeoff/run-beir.sh                       # encode and score the four candidates + the Qwen3 control
tools/embed-bakeoff/beir.sh vectors embeddinggemma-2  # 20 + 20 fixture vectors for the parity gate
tools/embed-bakeoff/beir.sh check <model>             # cat/kitten, EOS and prompt samples

python3 tools/embed-bakeoff/beir_report.py --write    # tables, top group and pick into RESULT.md
python3 tools/embed-bakeoff/check_fixture.py          # fixture rules
python3 tools/embed-bakeoff/test_runners.py           # runner failure handling, fake commands (in `just check`)
```

`run-beir.sh` and `run-ort.sh` stop at the first failed stage with its exit status, never print `done`
for it, and remove that stage's old result JSON, so a failed rerun cannot leave earlier numbers looking
current. `beir_eval.py` writes every result and cached vector to `<name>.part` and renames it on success,
and checks each BEIR zip's sha256 against the pinned value on every use, cached or new.

`beir.sh` points `HF_HOME` at `~/.cache/umx/hf` and sets `HF_HUB_DISABLE_IMPLICIT_TOKEN=1`, so the
stored `~/.cache/huggingface/token` is never read or sent; every repo is pinned by revision. BEIR zips
go to `~/.cache/umx/beir` and are never committed. `voyage-4-nano` needs `trust_remote_code`; its one
`.py` was read at the pinned revision first (RESULT.md, "Not pursued").

## The Rust path

- Loads `onnx/model.onnx` from the file so ORT finds the external weight files beside it; 4 intra-op
  threads.
- Feeds whatever the graph declares: `input_ids`, `attention_mask`, `position_ids`, `token_type_ids`,
  an empty KV cache (`past_key_values.*`, Qwen3) and empty media features (`*_features`,
  embeddinggemma-2). fastembed 6 cannot feed the last three, which is why it is not used.
- Right padding; mean pooling or last-token pooling by the attention mask, or the graph's own
  `sentence_embedding`; L2-normalise, cut to 256, re-normalise. Prompts are the same as `beir_eval.py`.
- `check` also embeds texts of different lengths one by one and in one padded batch, and reports the
  smallest cosine between the two.

## History

The first candidate pair (nomic-embed-text-v1.5, EmbeddingGemma-300M) ran through fastembed with the
harness at commit 63ae520. `report.py --write` still renders those tables from `results/`.

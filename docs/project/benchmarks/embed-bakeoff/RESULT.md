# Embedding bake-off: local model for secured graphs (workstream 004 s03)

Workstream 004 step s03 (rulings D27d, D38, D41). Candidates (decision): embeddinggemma-2,
harrier-oss-v1-0.6b, voyage-4-nano, and Qwen3-Embedding-0.6B as the baseline; jina-embeddings-v5
excluded (CC BY-NC). Acceptance rules: embeddings-expert.
The first candidate pair (nomic-embed-text-v1.5, EmbeddingGemma-300M) is kept below as history.

Everything needed to rerun is in `tools/embed-bakeoff/` (README). Raw results are in `results/`.
The tables in the next section are `python3 tools/embed-bakeoff/beir_report.py --write` output.

<!-- beir:start -->

### BEIR nDCG@10 (sentence-transformers reference path, one generic retrieval prompt per model)

Per-task 95% CIs: 2,000 bootstrap resamples of queries. Leaderboard (MTEB) numbers use per-task
instructions and will differ.

| Model | Licence | Dims | SciFact | NFCorpus | ArguAna | Mean | Cut cost (native - 256) | Fixture recall@10 at 256 |
|---|---|---|---|---|---|---|---|---|
| embeddinggemma-2 | Apache 2.0 | 256 | 0.837 [0.80, 0.87] | 0.350 [0.32, 0.38] | 0.572 [0.55, 0.59] | 0.586 | +0.017 | 0.931 |
| embeddinggemma-2 | Apache 2.0 | 768 | 0.852 [0.82, 0.88] | 0.369 [0.33, 0.40] | 0.590 [0.57, 0.61] | 0.604 |  |  |
| qwen3-embedding-0.6b | Apache 2.0 | 256 | 0.665 [0.62, 0.71] | 0.331 [0.30, 0.37] | 0.660 [0.64, 0.68] | 0.552 | +0.026 | 0.956 |
| qwen3-embedding-0.6b | Apache 2.0 | 1024 | 0.701 [0.66, 0.74] | 0.356 [0.32, 0.39] | 0.679 [0.66, 0.69] | 0.578 |  |  |
| voyage-4-nano | Apache 2.0 | 256 | 0.706 [0.66, 0.75] | 0.349 [0.32, 0.39] | 0.515 [0.50, 0.53] | 0.523 | +0.056 **flag** (> 0.03) | 0.981 |
| voyage-4-nano | Apache 2.0 | 2048 | 0.753 [0.71, 0.79] | 0.397 [0.36, 0.43] | 0.589 [0.57, 0.61] | 0.580 |  |  |
| harrier-oss-v1-0.6b | MIT | 256 | 0.666 [0.62, 0.71] | 0.288 [0.25, 0.32] | 0.579 [0.56, 0.60] | 0.511 | +0.092 **flag** (> 0.03) | 0.931 |
| harrier-oss-v1-0.6b | MIT | 1024 | 0.766 [0.73, 0.80] | 0.382 [0.35, 0.42] | 0.661 [0.64, 0.68] | 0.603 |  |  |
| qwen3-embedding-0.6b, no Instruct line (control) | | 256 | 0.636 | 0.249 | 0.665 | 0.517 | | 0.738 |

Prompts (from each model card): **embeddinggemma-2** q `task: search result | query: {q}`; d `title: {title or none} | text: {text}`; **qwen3-embedding-0.6b** q `Instruct: Given a web search query, retrieve relevant passages that answer the query\nQuery:{q}`; d plain; **voyage-4-nano** q `Represent the query for retrieving supporting documents: {q}`; d `Represent the document for retrieval: {d}`; **harrier-oss-v1-0.6b** q `Instruct: Given a web search query, retrieve relevant passages that answer the query\nQuery: {q}`; d plain.

### Top group at 256 (leader embeddinggemma-2, mean 0.586)

| Model | leader - model (mean nDCG@10) | paired bootstrap 95% CI (5,000) | p | Holm threshold (family 0.10) | In top group |
|---|---|---|---|---|---|
| qwen3-embedding-0.6b | +0.034 | [+0.018, +0.051] | 0.0000 | 0.033 | no |
| voyage-4-nano | +0.063 | [+0.047, +0.079] | 0.0000 | 0.050 | no |
| harrier-oss-v1-0.6b | +0.075 | [+0.058, +0.092] | 0.0000 | 0.100 | no |

Leave-one-task-out (mean nDCG@10 at 256 over the other two tasks):

| Task dropped | embeddinggemma-2 | qwen3-embedding-0.6b | voyage-4-nano | harrier-oss-v1-0.6b | Leader |
|---|---|---|---|---|---|
| scifact | 0.461 | 0.496 | 0.432 | 0.434 | qwen3-embedding-0.6b |
| nfcorpus | 0.704 | 0.662 | 0.610 | 0.623 | embeddinggemma-2 |
| arguana | 0.594 | 0.498 | 0.527 | 0.477 | embeddinggemma-2 |

Top group: embeddinggemma-2, qwen3-embedding-0.6b. Top two by mean (parity gate): embeddinggemma-2, qwen3-embedding-0.6b.

### similar_to floor at 256 (D28), on the 300-node fixture

| Model | relevant p5 / p50 / p95 | best irrelevant p5 / p50 / p95 | proposed floor |
|---|---|---|---|
| embeddinggemma-2 | 0.671 / 0.758 / 0.834 | 0.683 / 0.725 / 0.777 | 0.724 |
| qwen3-embedding-0.6b | 0.422 / 0.585 / 0.720 | 0.432 / 0.527 / 0.638 | 0.530 |
| voyage-4-nano | 0.280 / 0.486 / 0.637 | 0.292 / 0.413 / 0.532 | 0.406 |
| harrier-oss-v1-0.6b | 0.534 / 0.665 / 0.747 | 0.541 / 0.639 / 0.717 | 0.626 |

### Rust path (ort over the onnx-community export, shipped config: 1 session x 4 threads, batch 16; speed provisional: measured at 1-minute load up to 157, rule 16)

| Model | parity min cos docs / queries at 256 | batch invariance min cos | docs/s | query p50 / p95 ms | first / warm load s | RSS MB after load / after run / peak | fixture recall@10 / MRR@10 at 256 | load avg start / end |
|---|---|---|---|---|---|---|---|---|
| embeddinggemma-2 | 0.9998 / 0.9999 (pass) | 1.0000 | 10.7 | 17.1 / 33.0 | 2.15 / 0.57 | 1008 / 3104 / 3106 | 0.931 / 0.851 | 81 / 80 |
| qwen3-embedding-0.6b | 0.9998 / 0.9997 (pass) | 1.0000 | 3.9 | 50.7 / 104.4 | 5.21 / 0.77 | 1890 / 4005 / 4046 | 0.956 / 0.855 | 80 / 157 |

### Floors and pick

- embeddinggemma-2: passes every floor.
- qwen3-embedding-0.6b: fails or not measured: docs/s >= 10.
- **Pick: embeddinggemma-2** (top group, passes parity and floors; licences rank equal, then docs/s, then RSS).

<!-- beir:end -->

## How it was measured

- **Quality** on BEIR SciFact, NFCorpus and ArguAna (test qrels, graded relevance, nDCG@10), plus
  the 300-node synthetic fixture as a sanity check (recall@10 >= 0.80). Reference path:
  sentence-transformers 6.2.0 / transformers 5.22.1 / torch 2.15.0 on the Apple GPU (MPS), in a
  throwaway venv (`tools/embed-bakeoff/requirements.txt`). Each model's pinned original repo, its own
  pooling and normalisation modules, max_seq_length 2048 for all four (nothing truncated on the
  fixture; 1 NFCorpus document over 2048 for the Qwen-tokenised models). Vectors cut to 256 and
  re-normalised; exact cosine over the whole corpus; for ArguAna a query's own id is ignored, as in BEIR.
- **One generic prompt per model**, the one s04 would ship (prompt column above), for every task.
  Published MTEB scores use per-task instructions and will differ. The optional per-task reference
  rows were not run.
- **Rust path** (what s04 ships): ONNX Runtime 1.28.0 through ort 2.0.0-rc.13 over the onnx-community
  export, tokenizers 0.22.2, right padding, our own pooling (mean, last token by attention mask, or the
  graph's own `sentence_embedding`), L2-normalise, cut, re-normalise. Parity = cosine at 256 against
  the reference vectors of the same 20 documents and 20 queries from the fixture; batch invariance =
  the same texts one by one against one padded batch. Speed and memory in the shipped config: one
  session, 4 intra-op threads, batch 16, warm-up first. The speed corpus is the whole fixture cut into
  texts of exactly 300 tokens with the model's own tokenizer (up to 160 asked; the fixture yields 125
  for embeddinggemma-2 and 120 for Qwen3, the `corpus_texts` field); query latency one query at a time. RSS from `ps` after load and after the run; peak from getrusage.
- **Decoder models** (Qwen3, harrier): the tokenizer appends `<|endoftext|>`, which last-token pooling
  needs; logged in `results/beir-check-*.json` and `results/ort-check-*.json`.
- **Instrument checks**: cat/kitten above cat/carburetor for all four (`results/beir-check-*.json`);
  Qwen3 without its Instruct line as a control (row in the table).
- **Conditions**: Apple M4 Max, 16 cores, 128 GB, macOS 27.2, with other work running. Rust runs
  through `tools/cargo-slot` (D41), `nice -n 19`; the 1-minute load average is in each result file.
  Quality does not depend on load; speed does.
- **Source.** Branch s004/s03-local-bakeoff off main b567a3c. BEIR encodes and the parity reference
  vectors ran from `beir_eval.py` as committed in 8c68ef9; the scores were recomputed from those cached
  vectors with the D28 cosine added, as committed in d46cb11. The Rust runs (check, parity, quality,
  speed) used `src/main.rs` as committed in 25e0766. The encode path and the Rust harness are
  unchanged from those commits to this PR's head.
- **QMSum skipped.** The packet allowed QMSum (meeting transcripts) only if it wired up in 30 minutes;
  it was not attempted, so the three BEIR tasks above are the whole external set.

## Caveats

- **SciFact carries embeddinggemma-2's whole lead.** It scores 0.837 there against 0.665 to 0.706 for
  the others; on NFCorpus it is level and on ArguAna it trails Qwen3 (0.572 against 0.660). Neither
  card discloses fine-tuning data, so train-set exposure can be neither proved nor ruled out: possible
  exposure, unverified. Under the leave-one-task-out rule (expert, 14:40) the top group is
  embeddinggemma-2 and Qwen3-Embedding-0.6B.
- **Cut cost.** harrier-oss-v1-0.6b loses 0.092 mean nDCG@10 from 1024 to 256 (its card does not
  claim Matryoshka training) and voyage-4-nano 0.056 from 2048 to 256. Both are flagged; neither is in
  the top group.
- **Speed is provisional.** It ran under load against the rule of 16 (the run waited 30 minutes and
  then went ahead): 1-minute load 81 to 80 during embeddinggemma-2's run and 80 to 157 during Qwen3's.
  embeddinggemma-2's 10.7 docs/s is just over the floor of 10; Qwen3's 3.9 is far under it. Qwen3 ran
  under heavier load, so the ratio (about 2.7x, in line with a 270M text model against 0.6B) is not a
  controlled comparison. s04 re-measures embeddinggemma-2 under load 16.
- The fixture is near ceiling and synthetic; it is a sanity check, not a ranking. On it the two
  leaders are level at 256 (MRR@10 0.851 for embeddinggemma-2, 0.855 for Qwen3, through ort).
- **fastembed cannot run the two leaders.** fastembed 6.0.1 feeds only input_ids, attention_mask and
  token_type_ids. The Qwen3 export also needs position_ids and an (empty) KV cache, and the
  embeddinggemma-2 export needs (empty) image, video and audio feature inputs. The harness therefore
  drives ort directly, and s04's adapter will need to as well (the planned name was
  `umx-adapter-fastembed`).

## Not pursued

- voyage-4-nano's card says it shares an embedding space with hosted voyage-4, which would let secured
  graphs (local) be compared with open graphs (hosted). Not pursued (D38: providers are Azure and
  local only). voyage-4-nano was judged purely as a local model.
- voyage-4-nano's reference path needs `trust_remote_code`. Its `modeling_qwen3_bidirectional.py` was
  read at the pinned revision before running: 91 lines, a bidirectional attention mask over Qwen3Model
  and a linear 1024 to 2048 head; no file, network or process access. The ONNX export needs no code.

## Data and model files

BEIR datasets, fetched at run time into `~/.cache/umx/beir`, never committed:

| Dataset | URL | sha256 |
|---|---|---|
| SciFact | https://public.ukp.informatik.tu-darmstadt.de/thakur/BEIR/datasets/scifact.zip | 536e14446a0ba56ed1398ab1055f39fe852686ecad24a6306c80c490fa8e0165 |
| NFCorpus | https://public.ukp.informatik.tu-darmstadt.de/thakur/BEIR/datasets/nfcorpus.zip | efe5be03f8c5b86a5870102d0599d227c8c6e2484328e68c6522560385671b0b |
| ArguAna | https://public.ukp.informatik.tu-darmstadt.de/thakur/BEIR/datasets/arguana.zip | cfdf79adce27a401b3cd3ea267903134dbfab2c6afeb95d7fe5724a00bf7557b |

Reference (sentence-transformers) repos, pinned by revision, fetched with `token=False` and
`HF_HOME=~/.cache/umx/hf` (so no stored Hugging Face token is read):

| Model | Repo | Revision | Licence |
|---|---|---|---|
| embeddinggemma-2 | google/embeddinggemma-2 | f9972ac22e696a105c482202646979e33e4b5a1e | Apache 2.0 |
| harrier-oss-v1-0.6b | microsoft/harrier-oss-v1-0.6b | f9b9dc8d367d443f2479d27aa5d8d2850c0774ee | MIT |
| voyage-4-nano | MongoDB/voyage-4-nano | 5fb34e870f49f19e79c94d5e118f5ee730133279 | Apache 2.0 |
| Qwen3-Embedding-0.6B | Qwen/Qwen3-Embedding-0.6B | 97b0c614be4d77ee51c0cef4e5f07c00f9eb65b3 | Apache 2.0 |

ONNX exports for the Rust path (third-party: onnx-community), fetched by `fetch.sh` with plain curl at
a pinned commit, no token, into `~/.cache/umx/models/<org>--<repo>/<commit>/`:

| Repo @ commit | File | sha256 |
|---|---|---|
| onnx-community/Qwen3-Embedding-0.6B-ONNX @ c25a394dd583836952667c12f008335071b3f43d | onnx/model.onnx | bf27b2f3f9ef9c32ca337d75b361fa99439deaeaefe82e4701b2dbd8439197cc |
| | onnx/model.onnx_data | f0a61604465929a27e68aa6217c8c89ec6186572f0209fdb7711adda48a9b9a9 |
| | tokenizer.json | def76fb086971c7867b829c23a26261e38d9d74e02139253b38aeb9df8b4b50a |
| | config.json | 66a10929782f3c9a3cd5dec90e2a95c60e05736134a63cd54479eeae80bed175 |
| onnx-community/harrier-oss-v1-0.6b-ONNX @ e4daffa011e666dcd2ff2a3c6c05084090ac314d | onnx/model.onnx | 46e6d18455f25b2ebe9bccdc0fe3bf2a36442796ecc0991141c12c1f098f27b9 |
| | onnx/model.onnx_data | 1f936ece1604ac0117e808f9db9959e3e8b6b860d44d1a6ecf55a5353ee2d55c |
| | onnx/model.onnx_data_1 | 5b2a43d2df8ba4706d44769c023f2627309a8e48ec0fea70e92afc1a974e169f |
| | tokenizer.json | 2320ebaed2032c36f5c539cb8531d0642a10024e164f1d81ebbe163b259e3ba5 |
| | config.json | c55e075ac57223f7442ef1755051e9bd212e0d4ff2ed68694037f5a21dba7eeb |
| onnx-community/voyage-4-nano-ONNX @ fb6f32cceb63b6eac92e1fdc08e00d03c41cdf5a | onnx/model.onnx | 71197bbd7550b266ebe3d38be66bb3f96493bf4a4f8ef1337f46489db45f5d24 |
| | onnx/model.onnx_data | 637c07c9e7bdca172af40b203822d3c247826954af9138f2f6a363bc67595e13 |
| | tokenizer.json | c40c3736449ad0f4084a187dfe16f6850b2a2933dfe041394f850608b2890140 |
| | config.json | 0a37f11f814acca77619c3039409743285d15690eb33493d077784e653e9a524 |
| onnx-community/embeddinggemma-2-ONNX @ daa72c51243991dfcaf9f9137d2c573d8f7790c0 | onnx/model.onnx | bc47de15f81208a5c99e5ab10f746d5e33b51ea228b7dc0bef9c133a94f1c1c3 |
| | onnx/model.onnx_data | 9fd452bfc6916e5a92f080f8dada35598ca4512b21c65897d4c5974d5f9630a2 |
| | tokenizer.json | 4d777ef5bdc1aa36227abdfb77c3e49e7b9c892d16e1b6bda41c393504828be4 |
| | config.json | 8d011bfe08b5e345bbe0b81e5c6fd02c381920b345b986047bc2a33ce7b90d1d |

ONNX Runtime binary (fetched by ort-sys at build time, hash pinned in ort-sys 2.0.0-rc.13
`build/download/dist.tsv`): `https://cdn.pyke.io/0/pyke:ort-rs/ms@1.28.0/aarch64-apple-darwin+coreml.tar.lzma2`,
sha256 `6934874e2e953576d9c1db47ff1af39c62c4f4220dbe6f988e131f72879674c7`.

## History: first candidate pair (nomic-embed-text-v1.5 vs EmbeddingGemma-300M)

Superseded by the decided candidate set; kept as history. These runs used fastembed 6.0.1 and
the harness at commit 63ae520 (`git show 63ae520:tools/embed-bakeoff/src/main.rs`, with its
`run-all.sh`). The tables are
`python3 tools/embed-bakeoff/report.py --write` output from the `results/check-*`, `quality-*` and
`speed-*` files. The default-config rerun the expert asked for at 13:30 was overtaken by the new
candidate set and never ran, so the RSS question for these two stays open.

### Conditions (first run)

- Machine: Apple M4 Max, 16 cores, 128 GB, macOS 27.2. CPU only (ONNX Runtime 1.28.0, pyke build
  through ort 2.0.0-rc.13), fastembed 6.0.1.
- Repo commit at the start of the runs: 57b949e (main), harness on branch s004/s03-local-bakeoff.
- Load rules: every run behind `/tmp/umx-just-check.lock`, `nice -n 19`, ONNX intra-op threads 4
  (1 per pool session). Other work ran on the machine at the same time: 1-minute load averages are in each
  result file and in the speed table. Quality figures do not depend on load; speed figures do.
- Fixture: 300 synthetic nodes, 75 in each of 4 invented domains (food-safety compliance at an
  invented food maker, delivery of an invented software product, an invented design studio's
  workshops, call notes with invented customers). 30 notes run past 512 tokens, each with one fact
  that only appears after word 450; 15 distractor pairs. 80 queries: 40 paraphrase, 15 keyword,
  10 near-miss, 10 tail, 5 title, each with 1 to 3 relevant ids. `tools/embed-bakeoff/check_fixture.py`
  proves no query reuses 3 consecutive words from a relevant node and no paraphrase shares a
  content word with one. No client data; every name is invented.
- Text: whole-node Markdown key-value (`title: ...` newline `note: ...`), max_length 2048 for both
  models. Doc token mean 128, max 756; nothing was truncated at 2048 (30 of 300 would be at 512).

<!-- report:start -->

### Instrument checks

| Check | Result |
|---|---|
| nomic-embed-text-v1.5 @256: cos(cat, kitten) > cos(cat, carburetor) | 0.861 > 0.556: pass |
| embeddinggemma-300m @256: cos(cat, kitten) > cos(cat, carburetor) | 0.920 > 0.616: pass |
| Nomic @256 without prefixes (control) must score clearly worse | recall@10 0.963 vs 0.981, MRR@10 0.828 vs 0.826 |
| Nomic @256 tail queries at max_length 512 must score worse than at 2048 (facts sit past token 512) | recall@10 0.900 vs 1.000, MRR@10 0.579 vs 0.646 |

### Quality (recall is per query: relevant found in top k over relevant count, averaged)

| Model | Dims | recall@5 | recall@10 | MRR@10 | cut cost (r@10 768 - 256) | docs truncated at max_length | effective max_length |
|---|---|---|---|---|---|---|---|
| nomic-embed-text-v1.5 | 256 | 0.912 | 0.981 | 0.826 | 0.006 | 0 of 300 | 2048 |
| nomic-embed-text-v1.5 | 768 | 0.944 | 0.988 | 0.854 |  | 0 of 300 | 2048 |
| nomic, no prefixes (control) | 256 | 0.919 | 0.963 | 0.828 | 0.006 | 0 of 300 | 2048 |
| nomic, no prefixes (control) | 768 | 0.906 | 0.969 | 0.808 |  | 0 of 300 | 2048 |
| nomic, max_length 512 (control) | 256 | 0.912 | 0.969 | 0.812 | 0.019 | 30 of 300 | 512 |
| nomic, max_length 512 (control) | 768 | 0.944 | 0.988 | 0.848 |  | 30 of 300 | 512 |
| embeddinggemma-300m | 256 | 0.981 | 0.988 | 0.903 | 0.006 | 0 of 300 | 2048 |
| embeddinggemma-300m | 768 | 0.994 | 0.994 | 0.928 |  | 0 of 300 | 2048 |
| embeddinggemma-300m-q (int8) | 256 | 0.981 | 0.981 | 0.909 | 0.013 | 0 of 300 | 2048 |
| embeddinggemma-300m-q (int8) | 768 | 0.994 | 0.994 | 0.935 |  | 0 of 300 | 2048 |

### recall@10 / MRR@10 at 256 by query kind

| Kind | n | Nomic | Gemma | Gemma int8 |
|---|---|---|---|---|
| keyword | 15 | 1.000 / 1.000 | 1.000 / 0.967 | 1.000 / 0.967 |
| near_miss | 10 | 1.000 / 1.000 | 0.950 / 0.950 | 0.900 / 0.950 |
| paraphrase | 40 | 0.963 / 0.754 | 0.988 / 0.843 | 0.988 / 0.868 |
| tail | 10 | 1.000 / 0.646 | 1.000 / 0.950 | 1.000 / 0.900 |
| title | 5 | 1.000 / 0.900 | 1.000 / 1.000 | 1.000 / 1.000 |

### Cosine at 256: each relevant pair vs the best irrelevant hit per query (for the similar_to floor, D28)

| Model | relevant p5 / p50 / p95 | best irrelevant p5 / p50 / p95 | proposed floor (irrelevant p95 + relevant p5) / 2 |
|---|---|---|---|
| nomic-embed-text-v1.5 | 0.630 / 0.712 / 0.821 | 0.604 / 0.670 / 0.728 | 0.679 |
| embeddinggemma-300m | 0.426 / 0.555 / 0.689 | 0.385 / 0.484 / 0.568 | 0.497 |

### Speed (warm, ONNX intra-op threads capped; texts of 300 tokens cut from the fixture)

| Model | Setup | batch | docs/s | query p50 ms | query p95 ms | first load s | warm load s/session | RSS MB after load / after run / peak | load avg start/end |
|---|---|---|---|---|---|---|---|---|---|
| nomic-embed-text-v1.5 | pool of 4 x 1-thread sessions | 1 | 26.3 | 10.9 | 12.3 | 0.32 | 0.22 | n/a / n/a / 3128 | 22 / 30 |
| nomic-embed-text-v1.5 | 1 session x 4 threads (default) | 1 | 16.7 | 6.8 | 9.4 | 0.27 | 0.23 | n/a / n/a / 1260 | 30 / 29 |
| embeddinggemma-300m | pool of 4 x 1-thread sessions | 1 | 26.3 | 11.7 | 13.4 | 1.16 | 1.10 | n/a / n/a / 7663 | 29 / 25 |
| embeddinggemma-300m | 1 session x 4 threads (default) | 1 | 18.3 | 10.1 | 11.7 | 0.96 | 0.89 | n/a / n/a / 3698 | 25 / 26 |
| embeddinggemma-300m-q (int8) | pool of 4 x 1-thread sessions | 1 | 24.8 | 71.5 | 85.0 | 0.64 | 0.52 | n/a / n/a / 6184 | 26 / 24 |
| embeddinggemma-300m-q (int8) | 1 session x 4 threads (default) | 1 | 13.1 | 30.6 | 37.3 | 0.61 | 0.55 | n/a / n/a / 2261 | 24 / 22 |

### Floors at 256

| Floor | nomic-embed-text-v1.5 | embeddinggemma-300m |
|---|---|---|
| recall@10 >= 0.80 | pass | pass |
| cut cost <= 0.05 | pass | pass |
| query p95 <= 150 ms | pass | pass |
| warm load <= 2 s | pass | pass |

The pool rows are for reference: the pool is a setting, off by default, and is not a floor (ruling 13:30).

### Verdict

- Gemma minus Nomic at 256: recall@10 +0.006, MRR@10 +0.077.
- Sign test on per-query reciprocal rank (Gemma better / worse / tied): 18 / 10 / 52, one-sided p = 0.092.
- Bootstrap 95% CI on the MRR@10 difference (2,000 resamples of the 80 queries, seed 19): [+0.004, +0.159].
- Gemma int8 (tried, dropped): MRR@10 +0.006 against fp32, but slower on Apple CPU and barely lighter.
- Gemma clears the margin and the sign test: embeddinggemma-300m led this pair, **superseded by the 13:47 candidate set**.

<!-- report:end -->

### Caveats (first run)

- **Near-ceiling fixture.** recall@10 is about 0.98 for both models, so it does not separate them;
  MRR@10 decides. The Nomic no-prefix control is not clearly worse (recall@10 0.963 against 0.981,
  MRR@10 the same). The prefixes are applied: prefixed document texts are exactly 4 tokens longer
  (mean 128.2 against 124.2) and 18 queries change rank. Read this as "prefix effect small on this
  fixture", not as proof the prefixes do not matter; s04 keeps them, as Nomic's model card requires.
- **Tail control.** At max_length 512 Nomic's tail MRR@10 drops from 0.646 to 0.579 and recall@10
  from 1.0 to 0.9. Tail queries stay partly answerable from a long node's topic, so the drop is
  smaller than the late facts alone would suggest.
- **Sign test p = 0.092** is close to the 0.10 line; the bootstrap interval above is the second view.
- Cold load means the first load in a fresh process with the model files already on disk (the page
  cache may be warm). The true cold cost is the download: `fetch.sh` took 65 s for both full-precision
  models (0.5 GB Nomic, 1.2 GB Gemma) on this connection.

### Model files (first run)

Fetched by `tools/embed-bakeoff/fetch.sh` with plain curl at a pinned commit, no Hugging Face token,
no `HF_*` environment, into `~/.cache/umx/models/<org>--<repo>/<commit>/`.

`nomic-ai/nomic-embed-text-v1.5` at commit `e9b6763023c676ca8431644204f50c2b100d9aab` (Apache 2.0):

| File | sha256 |
|---|---|
| onnx/model.onnx | 147d5aa88c2101237358e17796cf3a227cead1ec304ec34b465bb08e9d952965 |
| tokenizer.json | d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66 |
| config.json | 9ab00bd92cee80a569f708140b7b6c1661a65891ff3765b1519e181ba2f2c92b |
| special_tokens_map.json | 5d5b662e421ea9fac075174bb0688ee0d9431699900b90662acd44b2a350503a |
| tokenizer_config.json | d7e0000bcc80134debd2222220427e6bf5fa20a669f40a0d0d1409cc18e0a9bc |

`onnx-community/embeddinggemma-300m-ONNX` at commit `5090578d9565bb06545b4552f76e6bc2c93e4a66` (Gemma terms):

| File | sha256 |
|---|---|
| onnx/model.onnx | ea91fd315a7c152d427d231746f0f811a1ac93beaba656abfdf2b24e091265e4 |
| onnx/model.onnx_data | ef835ae565d8695236652475903078e8ed794c7c35faf1164d78ec3238e8a88d |
| onnx/model_quantized.onnx (int8) | 172efde319fe1542dc41f31be6154910b05b78f7a861c265c4600eec906bd6d8 |
| onnx/model_quantized.onnx_data (int8) | 705626e28e4c23c82ade34566b4197d97f534c12275fa406dfb71e9937d388c0 |
| tokenizer.json | 4dda02faaf32bc91031dc8c88457ac272b00c1016cc679757d1c441b248b9c47 |
| config.json | 6e1f06404b7163e0325ed2ea3e6781cde50f4a50b31780a95ad0d30e8404d77b |
| special_tokens_map.json | 2f7b0adf4fb469770bb1490e3e35df87b1dc578246c5e7e6fc76ecf33213a397 |
| tokenizer_config.json | 3ca953eea6c3c9fcda9cf3df22949ff18b216f7c74bd6459230f3f1013953f3a |

ONNX Runtime binary (fetched by ort-sys at build time, hash pinned in ort-sys 2.0.0-rc.13
`build/download/dist.tsv`): `https://cdn.pyke.io/0/pyke:ort-rs/ms@1.28.0/aarch64-apple-darwin+coreml.tar.lzma2`,
sha256 `6934874e2e953576d9c1db47ff1af39c62c4f4220dbe6f988e131f72879674c7`.

### Gemma licence (Gemma Terms of Use, last modified 2026-04-01, read at ai.google.dev/gemma/terms)

This applies to EmbeddingGemma-300M only. embeddinggemma-2 is Apache 2.0 (its card and repo
metadata), so none of the conditions below apply to it.

EmbeddingGemma is listed in the terms' appendix, so the weights are "Gemma" whichever repo they come
from. What matters for umx, in short (not a legal opinion):

- **Vectors are free.** Embeddings are "Output"; Google claims no rights in Output, and Output is not
  a Model Derivative. Committed vectors carry no Gemma obligations.
- **Using it binds the user.** Anyone who uses the weights accepts the terms, including the Prohibited
  Use Policy (incorporated by reference) and termination on breach. Google may update the terms and
  says it may restrict use it believes breaks them. Nomic's Apache 2.0 has none of this.
- **Shipping the weights is Distribution, with conditions.** If umx bundles, mirrors or re-hosts the
  weights, it must pass the use restrictions on as an enforceable provision in umx's own terms, give
  every recipient a copy of the terms, mark modified files, and ship a NOTICE file with Google's
  required sentence. umx is MIT; it would need those added terms for that download.
- **Fetching from Hugging Face on the user's request** keeps umx out of Distribution: the user gets
  the weights from the publisher and accepts the terms there. umx would still need to tell the user
  that Gemma terms apply before the first fetch.
- **A daemon that serves Gemma similarity to other people over a network** could count as a "Hosted
  Service", which the terms treat as Distribution. A local daemon for one user is not.

**For the project owner before Gemma becomes the default:** nothing stops umx using Gemma, but making it the
default means every umx user accepts Google's terms and use policy, and umx cannot re-host the weights
without adding those terms to its own licence. Nomic carries no such conditions.

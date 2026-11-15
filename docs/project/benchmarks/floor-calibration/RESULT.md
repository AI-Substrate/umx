<!-- Generated from floors.json by umx-bench's floor-calibration down to the notes marker. -->
# Similarity floor calibration (workstream 004 s26)

Method (embeddings-expert ruling 2026-11-11): per key, every text embedded as umx stores it (umx's own adapters and embed loop, int8 rows), cosines as umx scores them. Search floor: p99 of query-to-random-document cosine (200 random non-relevant documents per query, each document's best row). similar_to floor: p99 of document-to-random-document cosine (whole-node rows, 200 per document, up to 3000 documents per task). Gate (s26b): umx search's hybrid nDCG@10 on each BEIR task within 0.01 of the floor at 0 AND the s03 fixture's umx search recall@10 within 0.02 of the floor at 0, else the search floor is lowered in 0.01 steps, never below the query null's p50 (stopping there is recorded as floor_gate_unmet).

## Floors

| key | search floor | search p99 | similar floor | run | times lowered | gate unmet |
| --- | --- | --- | --- | --- | --- | --- |
| `embeddinggemma-2@256+p1` | 0.7101 | 0.7401 | 0.8462 | fc-2026-11-11-cdc1965 | 3 | no |
| `text-embedding-3-large@256` | 0.409 | 0.529 | 0.6133 | fc-2026-11-11-cdc1965 | 12 | no |
| `text-embedding-3-small@256` | 0.4095 | 0.5195 | 0.5807 | fc-2026-11-11-cdc1965 | 11 | no |

## Null distributions (pooled over the BEIR tasks)

| key | query-doc p50 / p95 / p99 | doc-doc p50 / p95 / p99 |
| --- | --- | --- |
| `embeddinggemma-2@256+p1` | 0.5983 / 0.7014 / 0.7401 | 0.7069 / 0.8095 / 0.8462 |
| `text-embedding-3-large@256` | 0.2758 / 0.4456 / 0.529 | 0.3757 / 0.537 / 0.6133 |
| `text-embedding-3-small@256` | 0.2687 / 0.4377 / 0.5195 | 0.3294 / 0.5018 / 0.5807 |

## Labels: umx search at floor 0 and at the search floor

| key | task | nDCG@10 at 0 | at floor | recall@10 at 0 | at floor | vector rows/query at 0 | at floor | queries with a vector row at 0 | at floor |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `embeddinggemma-2@256+p1` | arguana | 0.548 | 0.5473 | 0.8748 | 0.8748 | 10.0 | 9.8826 | 1406 / 1406 | 1406 |
| `embeddinggemma-2@256+p1` | fixture (gated on recall@10) | 0.7118 | 0.7343 | 0.9875 | 0.9875 | 10.0 | 8.8375 | 80 / 80 | 80 |
| `embeddinggemma-2@256+p1` | nfcorpus | 0.3625 | 0.3624 | 0.1826 | 0.1817 | 9.8266 | 9.4025 | 323 / 323 | 321 |
| `embeddinggemma-2@256+p1` | scifact | 0.7739 | 0.7739 | 0.8996 | 0.8996 | 10.0 | 9.9567 | 300 / 300 | 300 |
| `text-embedding-3-large@256` | arguana | 0.5141 | 0.5141 | 0.8535 | 0.8535 | 10.0 | 10.0 | 1406 / 1406 | 1406 |
| `text-embedding-3-large@256` | fixture (gated on recall@10) | 0.7302 | 0.7537 | 0.9813 | 0.975 | 10.0 | 7.5875 | 80 / 80 | 80 |
| `text-embedding-3-large@256` | nfcorpus | 0.3826 | 0.3734 | 0.1895 | 0.18 | 9.7616 | 7.3127 | 323 / 323 | 289 |
| `text-embedding-3-large@256` | scifact | 0.7243 | 0.7285 | 0.8537 | 0.8537 | 10.0 | 9.63 | 300 / 300 | 298 |
| `text-embedding-3-small@256` | arguana | 0.4814 | 0.4814 | 0.8122 | 0.8122 | 10.0 | 10.0 | 1406 / 1406 | 1406 |
| `text-embedding-3-small@256` | fixture (gated on recall@10) | 0.6979 | 0.7318 | 0.9688 | 0.95 | 10.0 | 6.825 | 80 / 80 | 80 |
| `text-embedding-3-small@256` | nfcorpus | 0.3687 | 0.3628 | 0.1769 | 0.171 | 9.7926 | 7.5387 | 323 / 323 | 292 |
| `text-embedding-3-small@256` | scifact | 0.719 | 0.72 | 0.8433 | 0.8499 | 10.0 | 9.8067 | 300 / 300 | 300 |

## Lowering steps (the gates that failed at each floor tried)

| key | floor tried | failed |
| --- | --- | --- |
| `embeddinggemma-2@256+p1` | 0.7401 | fixture recall10 -0.1 |
| `embeddinggemma-2@256+p1` | 0.7301 | fixture recall10 -0.0562 |
| `embeddinggemma-2@256+p1` | 0.7201 | fixture recall10 -0.0375 |
| `text-embedding-3-large@256` | 0.529 | scifact ndcg10 -0.0102, nfcorpus ndcg10 -0.0321, fixture recall10 -0.0625 |
| `text-embedding-3-large@256` | 0.519 | nfcorpus ndcg10 -0.0314, fixture recall10 -0.0688 |
| `text-embedding-3-large@256` | 0.509 | nfcorpus ndcg10 -0.0299, fixture recall10 -0.0563 |
| `text-embedding-3-large@256` | 0.499 | nfcorpus ndcg10 -0.0249, fixture recall10 -0.05 |
| `text-embedding-3-large@256` | 0.489 | nfcorpus ndcg10 -0.0264, fixture recall10 -0.05 |
| `text-embedding-3-large@256` | 0.479 | nfcorpus ndcg10 -0.0238, fixture recall10 -0.0438 |
| `text-embedding-3-large@256` | 0.469 | nfcorpus ndcg10 -0.0184 |
| `text-embedding-3-large@256` | 0.459 | nfcorpus ndcg10 -0.0156 |
| `text-embedding-3-large@256` | 0.449 | nfcorpus ndcg10 -0.0116 |
| `text-embedding-3-large@256` | 0.439 | nfcorpus ndcg10 -0.0133 |
| `text-embedding-3-large@256` | 0.429 | nfcorpus ndcg10 -0.0103 |
| `text-embedding-3-large@256` | 0.419 | nfcorpus ndcg10 -0.0101 |
| `text-embedding-3-small@256` | 0.5195 | nfcorpus ndcg10 -0.0197, fixture recall10 -0.1813 |
| `text-embedding-3-small@256` | 0.5095 | nfcorpus ndcg10 -0.0196, fixture recall10 -0.1625 |
| `text-embedding-3-small@256` | 0.4995 | nfcorpus ndcg10 -0.0173, fixture recall10 -0.1625 |
| `text-embedding-3-small@256` | 0.4895 | nfcorpus ndcg10 -0.0168, fixture recall10 -0.1313 |
| `text-embedding-3-small@256` | 0.4795 | nfcorpus ndcg10 -0.0143, fixture recall10 -0.1063 |
| `text-embedding-3-small@256` | 0.4695 | nfcorpus ndcg10 -0.0128, fixture recall10 -0.1 |
| `text-embedding-3-small@256` | 0.4595 | nfcorpus ndcg10 -0.0103, fixture recall10 -0.0875 |
| `text-embedding-3-small@256` | 0.4495 | fixture recall10 -0.0563 |
| `text-embedding-3-small@256` | 0.4395 | fixture recall10 -0.0313 |
| `text-embedding-3-small@256` | 0.4295 | fixture recall10 -0.0313 |
| `text-embedding-3-small@256` | 0.4195 | fixture recall10 -0.025 |

## Provenance

| key | run date | umx commit | adapter | deployment | texts sent | est. tokens sent |
| --- | --- | --- | --- | --- | --- | --- |
| `embeddinggemma-2@256+p1` | 2026-11-11 | cdc19658 | llamacpp.embedder | - | 0 | 0 |
| `text-embedding-3-large@256` | 2026-11-11 | cdc19658 | azure-openai.embedder | text-embedding-3-large-01 | 0 | 0 |
| `text-embedding-3-small@256` | 2026-11-11 | cdc19658 | azure-openai.embedder | text-embedding-3-small-01 | 0 | 0 |

Datasets (fetched at run time, never committed):

- arguana: https://public.ukp.informatik.tu-darmstadt.de/thakur/BEIR/datasets/arguana.zip sha256 `cfdf79adce27a401b3cd3ea267903134dbfab2c6afeb95d7fe5724a00bf7557b`
- nfcorpus: https://public.ukp.informatik.tu-darmstadt.de/thakur/BEIR/datasets/nfcorpus.zip sha256 `efe5be03f8c5b86a5870102d0599d227c8c6e2484328e68c6522560385671b0b`
- scifact: https://public.ukp.informatik.tu-darmstadt.de/thakur/BEIR/datasets/scifact.zip sha256 `536e14446a0ba56ed1398ab1055f39fe852686ecad24a6306c80c490fa8e0165`

<!-- notes: hand-written below this line -->

## Notes

- **s26b rule (embeddings-expert, 2026-11-11 19:20)**: the s26 runs gated only on BEIR, which left
  embeddinggemma-2 at its p99 (0.7401); that floor cut the s03 fixture's paraphrases (recorded-vector hybrid recall@10
  0.963 to 0.825). The search floor now also has to keep the s03 fixture's umx search recall@10 within 0.02 of no
  floor, in the same run, and never goes below the query null's p50 (floor_gate_unmet if it would). All three keys
  were rerun from cached vectors (nothing sent to any provider): embeddinggemma-2 0.7401 to 0.7101 (the fixture alone
  drove 3 steps), text-embedding-3-large@256 unchanged at 0.409 (fixture recall 0.975 against 0.9813 at 0), and
  text-embedding-3-small@256 0.4495 to 0.4095 (the fixture drove the last 4 steps). No key hit its p50 stop.
- **Search floors are quality-preserving cuts**, not 1-in-100 noise bounds: every key's was lowered from its p99.
  NFCorpus drove the BEIR steps for both OpenAI models; at large's p99 SciFact also failed narrowly (0.0102) and passed
  from the next step; ArguAna never failed. Drops are not monotonic in the floor (large NFCorpus 0.0116 at 0.449,
  0.0133 at 0.439; small fixture 0.0313 at both 0.4395 and 0.4295), so the first passing step is the answer the rule
  asks for, not a proven optimum. Large's 0.409 is below its own query-noise p95 (0.4456).
- **similar_to floors are the noise p99 unchanged** (no label gate in the ruling). For embeddinggemma-2 this moves the
  default from the old uncalibrated 0.60 to 0.8462, so its similar_to lists keep far fewer, closer neighbours; for the
  OpenAI models from 0.50 to 0.6133 (large) and 0.5807 (small). Stored lists made with the old floors are stale through
  `_meta.tsv` and rebuild on load.
- **Text form**: umx embeds three texts per BEIR node (whole node, title, note), each as production stores it, so the
  query-to-document null is each document's best row, as the vector leg compares it. BEIR documents with no title take
  their first 12 words as the node title.
- **Tokens and cost** (Azure Monitor `ProcessedPromptTokens` per deployment over each run's window; umx's own estimate
  is deliberately high at 2 bytes a token): large 10,152,358 real tokens (umx estimate 24.3M), about $1.32 at $0.13 per
  1M, deployment `text-embedding-3-large-01`, 03:44 to 04:32 UTC; small (deployment `text-embedding-3-small-01`,
  04:33 to 05:48 UTC): small and large share a tokenizer and were sent the same texts, so about 10.15M tokens,
  about $0.20 at $0.02 per 1M. No 429s in either run (pacer at 1.5M estimated tokens a minute). embeddinggemma-2 ran
  locally (Metal), no cost.
- **Measured code**: the s26b runs record commit cdc19658 (s26b branch: the fixture gate, off main 94e641e0). The
  s26 runs (commit 06902fec) made the embeddings; the token and cost figures below are theirs.
- **Probe phrases** (unchanged by s26b, since large's floor did not move; text-embedding-3-large@256): 34 probe
  phrases ran against several private graphs; only counts are recorded. At the old 0.50 floor 16 of 34 probes had at
  least one vector row in their top 10; at the calibrated 0.409, 32 of 34. The two left are a phrase with no hits at
  all and one with a single exact hit. An earlier count of 41 probes (33 with no vector row) held 7 more phrases, none
  with a vector row at 0.50; even if all 7 still had none at 0.409, the result is 32 of 41, above the target of 30.
- **Raw evidence**: `logs/` holds each run's log (BEIR metrics and progress only) and the probe counts.

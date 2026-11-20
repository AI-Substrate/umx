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

## Strength cuts (PRD 002 search-10)

Method (embeddings-expert): `floor-calibration --strength`, from the rows of the key's floors run (its vector cache, offline: a text not in it fails the run). Whole-node cosines; z = (cosine - median) / MAD over the N nodes searched. Corpora: the three BEIR sets, the s03 fixture and `tight` (the 1,000 NFCorpus documents nearest the one with most near neighbours), each at N = 300, 2,000 and 20,000 (capped at its size). Off-topic queries are a subject the graph does not hold: another set's queries only for pairs whose domains are disjoint (spot-checked), and the fixture's written off-topic list. Strong: no cell's off-topic top hits strong more than 1 in 100; fair: none fair or strong more than 1 in 10. On-topic rates are reported, never targeted.

| key | run | z part | strong z (+ per ln N) | strong cosine | fair z (+ per ln N) | fair cosine | min population | max N measured (labels stop above 2x) |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `embeddinggemma-2@256+p1` | sc-2026-11-19-c5485aa | true | 5.302 (+0.5996) | 0.7382 | -1.5398 (+1.1069) | 0.7117 | 100 | 8674 |
| `text-embedding-3-large@256` | sc-2026-11-19-c5485aa | true | 3.3557 (+0.6124) | 0.4728 | -0.797 (+0.8843) | 0.409 | 200 | 8674 |
| `text-embedding-3-small@256` | sc-2026-11-19-c5485aa | true | 6.0899 (+0.5293) | 0.4883 | 1.0806 (+0.6599) | 0.4095 | 100 | 8674 |

### Shipped cuts per corpus and N

On-topic: share of targets. Off-topic: how many of the cell's off-topic queries' top hits are strong, and fair or strong (a cell of 45 queries cannot show 1 in 100: one is 2.2%, so none is allowed there).

| key | cell | on-topic strong | on-topic fair or strong | off-topic strong | off-topic fair or strong |
| --- | --- | --- | --- | --- | --- |
| `embeddinggemma-2@256+p1` | arguana@2000 | 0.147 | 0.7259 | 0 of 80 | 1 of 80 |
| `embeddinggemma-2@256+p1` | arguana@300 | 0.3119 | 0.9051 | 0 of 80 | 1 of 80 |
| `embeddinggemma-2@256+p1` | arguana@8674 | 0.0657 | 0.3919 | 0 of 80 | 0 of 80 |
| `embeddinggemma-2@256+p1` | fixture@300 | 0.2125 | 0.825 | 0 of 345 | 1 of 345 |
| `embeddinggemma-2@256+p1` | nfcorpus@2000 | 0.0774 | 0.387 | 0 of 45 | 1 of 45 |
| `embeddinggemma-2@256+p1` | nfcorpus@300 | 0.1827 | 0.6811 | 0 of 45 | 1 of 45 |
| `embeddinggemma-2@256+p1` | nfcorpus@3633 | 0.0526 | 0.2724 | 0 of 45 | 3 of 45 |
| `embeddinggemma-2@256+p1` | scifact@2000 | 0.27 | 0.73 | 0 of 125 | 12 of 125 |
| `embeddinggemma-2@256+p1` | scifact@300 | 0.4067 | 0.91 | 0 of 125 | 7 of 125 |
| `embeddinggemma-2@256+p1` | scifact@5183 | 0.1933 | 0.59 | 0 of 125 | 4 of 125 |
| `embeddinggemma-2@256+p1` | tight@1000 | 0.1237 | 0.4845 | 0 of 45 | 0 of 45 |
| `embeddinggemma-2@256+p1` | tight@300 | 0.1856 | 0.5876 | 0 of 45 | 0 of 45 |
| `text-embedding-3-large@256` | arguana@2000 | 0.5025 | 0.8915 | 0 of 80 | 8 of 80 |
| `text-embedding-3-large@256` | arguana@300 | 0.7552 | 0.9857 | 0 of 80 | 4 of 80 |
| `text-embedding-3-large@256` | arguana@8674 | 0.3019 | 0.6809 | 0 of 80 | 6 of 80 |
| `text-embedding-3-large@256` | fixture@300 | 0.45 | 0.9 | 0 of 345 | 1 of 345 |
| `text-embedding-3-large@256` | nfcorpus@2000 | 0.2291 | 0.5015 | 0 of 45 | 0 of 45 |
| `text-embedding-3-large@256` | nfcorpus@300 | 0.3096 | 0.6285 | 0 of 45 | 0 of 45 |
| `text-embedding-3-large@256` | nfcorpus@3633 | 0.1765 | 0.4303 | 0 of 45 | 0 of 45 |
| `text-embedding-3-large@256` | scifact@2000 | 0.27 | 0.62 | 0 of 125 | 7 of 125 |
| `text-embedding-3-large@256` | scifact@300 | 0.4467 | 0.8333 | 0 of 125 | 1 of 125 |
| `text-embedding-3-large@256` | scifact@5183 | 0.1933 | 0.49 | 0 of 125 | 3 of 125 |
| `text-embedding-3-large@256` | tight@1000 | 0.1707 | 0.3659 | 0 of 45 | 0 of 45 |
| `text-embedding-3-large@256` | tight@300 | 0.2195 | 0.4756 | 0 of 45 | 0 of 45 |
| `text-embedding-3-small@256` | arguana@2000 | 0.0935 | 0.848 | 0 of 80 | 8 of 80 |
| `text-embedding-3-small@256` | arguana@300 | 0.2541 | 0.9565 | 0 of 80 | 6 of 80 |
| `text-embedding-3-small@256` | arguana@8674 | 0.0514 | 0.6645 | 0 of 80 | 7 of 80 |
| `text-embedding-3-small@256` | fixture@300 | 0.1 | 0.7875 | 0 of 345 | 3 of 345 |
| `text-embedding-3-small@256` | nfcorpus@2000 | 0.0248 | 0.4365 | 0 of 45 | 0 of 45 |
| `text-embedding-3-small@256` | nfcorpus@300 | 0.1084 | 0.5728 | 0 of 45 | 0 of 45 |
| `text-embedding-3-small@256` | nfcorpus@3633 | 0.0155 | 0.3684 | 0 of 45 | 0 of 45 |
| `text-embedding-3-small@256` | scifact@2000 | 0.0667 | 0.5 | 0 of 125 | 7 of 125 |
| `text-embedding-3-small@256` | scifact@300 | 0.1067 | 0.69 | 0 of 125 | 5 of 125 |
| `text-embedding-3-small@256` | scifact@5183 | 0.0467 | 0.3733 | 0 of 125 | 7 of 125 |
| `text-embedding-3-small@256` | tight@1000 | 0.0233 | 0.3372 | 0 of 45 | 0 of 45 |
| `text-embedding-3-small@256` | tight@300 | 0.0581 | 0.4535 | 0 of 45 | 0 of 45 |

### z settling (embeddings-expert): strong hits per on-topic query, median / p90

| key | cell | queries | joint rule | cosine-only rule |
| --- | --- | --- | --- | --- |
| `embeddinggemma-2@256+p1` | fixture@300 | 80 | 0.0 / 1.0 | 1.0 / 2.0 |
| `embeddinggemma-2@256+p1` | scifact@300 | 300 | 0.0 / 1.0 | 2.0 / 9.0 |
| `embeddinggemma-2@256+p1` | tight@1000 | 97 | 0.0 / 1.0 | 12.0 / 231.0 |
| `embeddinggemma-2@256+p1` | tight@300 | 97 | 0.0 / 1.0 | 4.0 / 65.0 |
| `text-embedding-3-large@256` | fixture@300 | 80 | 0.0 / 2.0 | 1.0 / 3.0 |
| `text-embedding-3-large@256` | scifact@300 | 300 | 0.0 / 1.0 | 1.0 / 5.0 |
| `text-embedding-3-large@256` | tight@1000 | 82 | 0.0 / 2.0 | 3.0 / 52.0 |
| `text-embedding-3-large@256` | tight@300 | 82 | 0.0 / 1.0 | 2.0 / 16.0 |
| `text-embedding-3-small@256` | fixture@300 | 80 | 0.0 / 1.0 | 0.0 / 1.0 |
| `text-embedding-3-small@256` | scifact@300 | 300 | 0.0 / 1.0 | 1.0 / 1.0 |
| `text-embedding-3-small@256` | tight@1000 | 86 | 0.0 / 0.0 | 0.0 / 2.0 |
| `text-embedding-3-small@256` | tight@300 | 86 | 0.0 / 0.0 | 0.0 / 2.0 |

### s03 fixture by query kind (shipped cuts; N = 300)

| key | kind | queries | target strong | target fair or strong | no_strong_match fires | a strong-only rule fires |
| --- | --- | --- | --- | --- | --- | --- |
| `embeddinggemma-2@256+p1` | keyword | 15 | 0.5333 | 1.0 | 0.0 | 0.4667 |
| `embeddinggemma-2@256+p1` | near_miss | 10 | 0.4 | 0.9 | 0.0 | 0.6 |
| `embeddinggemma-2@256+p1` | off_topic_arguana | 1406 | 0.0 | 0.0021 | 0.9979 | 1.0 |
| `embeddinggemma-2@256+p1` | off_topic_nfcorpus | 323 | 0.0 | 0.0681 | 0.9319 | 1.0 |
| `embeddinggemma-2@256+p1` | off_topic_scifact | 300 | 0.0 | 0.0033 | 0.9967 | 1.0 |
| `embeddinggemma-2@256+p1` | off_topic_written | 45 | 0.0 | 0.0 | 1.0 | 1.0 |
| `embeddinggemma-2@256+p1` | paraphrase | 40 | 0.025 | 0.8 | 0.175 | 0.975 |
| `embeddinggemma-2@256+p1` | tail | 10 | 0.0 | 0.5 | 0.2 | 1.0 |
| `embeddinggemma-2@256+p1` | title | 5 | 0.8 | 1.0 | 0.0 | 0.0 |
| `text-embedding-3-large@256` | keyword | 15 | 0.6667 | 0.9333 | 0.0 | 0.3333 |
| `text-embedding-3-large@256` | near_miss | 10 | 0.7 | 1.0 | 0.0 | 0.3 |
| `text-embedding-3-large@256` | off_topic_arguana | 1406 | 0.0007 | 0.032 | 0.968 | 0.9993 |
| `text-embedding-3-large@256` | off_topic_nfcorpus | 323 | 0.0031 | 0.0557 | 0.9443 | 0.9969 |
| `text-embedding-3-large@256` | off_topic_scifact | 300 | 0.0 | 0.0033 | 0.9967 | 1.0 |
| `text-embedding-3-large@256` | off_topic_written | 45 | 0.0 | 0.0 | 1.0 | 1.0 |
| `text-embedding-3-large@256` | paraphrase | 40 | 0.3 | 0.95 | 0.05 | 0.7 |
| `text-embedding-3-large@256` | tail | 10 | 0.2 | 0.5 | 0.2 | 0.4 |
| `text-embedding-3-large@256` | title | 5 | 1.0 | 1.0 | 0.0 | 0.0 |
| `text-embedding-3-small@256` | keyword | 15 | 0.3333 | 0.8667 | 0.0667 | 0.6667 |
| `text-embedding-3-small@256` | near_miss | 10 | 0.0 | 1.0 | 0.0 | 1.0 |
| `text-embedding-3-small@256` | off_topic_arguana | 1406 | 0.0 | 0.0491 | 0.9509 | 1.0 |
| `text-embedding-3-small@256` | off_topic_nfcorpus | 323 | 0.0031 | 0.0402 | 0.9598 | 0.9969 |
| `text-embedding-3-small@256` | off_topic_scifact | 300 | 0.0 | 0.0067 | 0.9933 | 1.0 |
| `text-embedding-3-small@256` | off_topic_written | 45 | 0.0 | 0.0222 | 0.9778 | 1.0 |
| `text-embedding-3-small@256` | paraphrase | 40 | 0.0 | 0.725 | 0.25 | 1.0 |
| `text-embedding-3-small@256` | tail | 10 | 0.0 | 0.6 | 0.3 | 0.9 |
| `text-embedding-3-small@256` | title | 5 | 0.6 | 1.0 | 0.0 | 0.4 |

### z ablation (each family's cuts chosen alike; worst cell over the kept off-topic cells)

| key | family | off-topic strong rate | strong cuts | worst off-topic strong | worst off-topic fair or strong | fixture paraphrase strong |
| --- | --- | --- | --- | --- | --- | --- |
| `embeddinggemma-2@256+p1` | joint | 0.01 | z 5.302 + 0.5996 ln N, cosine 0.7382 | 0.0 | 0.096 | 0.025 |
| `embeddinggemma-2@256+p1` | cosine_only | 0.01 | z 0.0 + 0.0 ln N, cosine 0.7575 (no z) | 0.008 | 0.096 | 0.5 |
| `embeddinggemma-2@256+p1` | z_only | 0.01 | z 5.302 + 0.5996 ln N, cosine 0.0 | 0.0 | 0.096 | 0.025 |
| `text-embedding-3-large@256` | joint | 0.01 | z 3.3557 + 0.6124 ln N, cosine 0.4728 | 0.0 | 0.1 | 0.3 |
| `text-embedding-3-large@256` | cosine_only | 0.01 | z 0.0 + 0.0 ln N, cosine 0.514 (no z) | 0.0 | 0.1 | 0.825 |
| `text-embedding-3-large@256` | z_only | 0.01 | z 5.2077 + 0.4624 ln N, cosine 0.0 | 0.0 | 0.1 | 0.125 |
| `text-embedding-3-small@256` | joint | 0.01 | z 6.0899 + 0.5293 ln N, cosine 0.4883 | 0.0 | 0.1 | 0.0 |
| `text-embedding-3-small@256` | cosine_only | 0.01 | z 0.0 + 0.0 ln N, cosine 0.6104 (no z) | 0.0 | 0.1 | 0.15 |
| `text-embedding-3-small@256` | z_only | 0.01 | z 6.0899 + 0.5293 ln N, cosine 0.0 | 0.0 | 0.1 | 0.0 |

Caveat: the joint rule's median is 0 in these cells too, because strong is rare by design at the 1-in-100 off-topic rate; the table shows z prevents a flood of strong hits in tight graphs, not that z makes strong common; cosine-only has higher on-topic strong rates and floods tight graphs.

### Deployment drift check (hosted keys)

The floors run's deployments are gone; the same s03 fixture texts embedded through today's deployment against the floors run's cached vectors (10 queries, 10 whole-node texts). Pass: every cosine >= 0.999 (the int8 step count is reported, not a criterion).

| key | date | texts | min cosine | max int8 step | result |
| --- | --- | --- | --- | --- | --- |
| `text-embedding-3-large@256` | 2026-11-19 | 20 | 0.999659 | 3 | pass |
| `text-embedding-3-small@256` | 2026-11-19 | 20 | 0.999999 | 1 | pass |

### MIN_POPULATION bootstrap (N: p90 z half-width / half the strong-fair gap)

| key | rungs |
| --- | --- |
| `embeddinggemma-2@256+p1` | 10: 71.1506 / 2.8368, 15: 13.6928 / 2.734, 20: 10.5037 / 2.661, 30: 5.9277 / 2.5582, 50: 3.6588 / 2.4286, 75: 2.6254 / 2.3258, 100: 2.1936 / 2.2528, 150: 1.6822 / 2.15, 200: 1.4984 / 2.077, 300: 1.106 / 1.9741, 500: 0.8762 / 1.8446 |
| `text-embedding-3-large@256` | 10: 62.2692 / 1.7633, 15: 10.3293 / 1.7082, 20: 8.6608 / 1.6691, 30: 5.1907 / 1.614, 50: 3.3334 / 1.5445, 75: 2.2786 / 1.4894, 100: 2.1385 / 1.4503, 150: 1.6626 / 1.3952, 200: 1.3545 / 1.356, 300: 1.0791 / 1.3009, 500: 0.8024 / 1.2315 |
| `text-embedding-3-small@256` | 10: 55.3579 / 2.3543, 15: 11.3466 / 2.3278, 20: 8.8301 / 2.309, 30: 4.7387 / 2.2826, 50: 3.1819 / 2.2492, 75: 2.2627 / 2.2227, 100: 1.9144 / 2.2039, 150: 1.5772 / 2.1775, 200: 1.2396 / 2.1587, 300: 1.0263 / 2.1322, 500: 0.7768 / 2.0988 |

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

### Strength cuts (PRD 002 search-10), notes

- Run 2026-11-19 (UTC) with umx-bench `floor-calibration --strength` as it stands at the search-10 PR head (its commit
  is recorded after merge); `umx_commit` in each record (c5485aa) is the base the tool was built on, which does not
  hold the tool. Every key read its floors run's vector cache (`<cache>/floor-calibration/<key>.vec`, sha256 in the
  record) offline: 0 texts sent in the measuring run; a text not in the cache fails it. The cache holds only the BEIR
  sets and the s03 fixture's texts, as the floors run fc-2026-11-11-cdc1965 embedded them; it is never committed.
  Without it the tool cannot run again; the committed cuts stay as they are. Regenerate it with the floors run's own
  command.
- Off-topic sources: the PRD's "a subject the graph does not hold". Each candidate (corpus, other query set) pair's 20
  highest off-topic top hits were read side by side (`--spot`); 7 pairs kept, 13 dropped, both lists with what the
  spot check showed in each record.
- The written off-topic list (embed-bakeoff/offtopic.jsonl, 45 queries) has recorded rows for all three keys (the
  hosted keys' rows were made by one `--send-offtopic` call each, 45 texts, about 1,254 tokens; provenance beside the
  rows). Adding them moved the hosted cuts (large strong z 2.3138 + 0.7951 ln N to 3.3557 + 0.6124 ln N; small strong
  7.2939 + 0.3182 ln N, cosine 0.5313, to 6.0899 + 0.5293 ln N, cosine 0.4883).
- The tight graph and NFCorpus have only the written list as off-topic source. In tight none of its 45 queries' top
  hits passes the floor for any key, so tight shows on-topic rates and gives no off-topic evidence; NFCorpus has 2 to
  7 of 135 (over its three N) past the floor.
- A cell with 45 off-topic queries cannot show 1 in 100: one strong top hit is 2.2%, so the rule allows none there.
- A cosine cut never goes below the search floor (a hit below the floor is never labelled).
- Labels stop above twice `max_n_measured` (8,674, the largest set): about 17,000 nodes searched in one space, until a
  larger corpus is measured (follow-up: a 57k corpus such as FiQA sampled at 20k and 50k).
- The z ablation (the rows above, every rate in the `--raw` policy file): the cosine-only rule holds 1 in 100 in every
  kept cell and labels more on-topic targets strong; the z settling table shows what it costs in a near-duplicate
  graph (gemma: a median of 4 and 12 strong hits per on-topic query at N 300 and 1,000, against 0 for the joint rule).
  The prime ruled to keep z; the shipped rule is the joint one, and `z_part` in the record switches it.
- Rows per source (hosted keys): the BEIR and fixture rows are the floors run's (2026-11-11, deployments
  `text-embedding-3-large-01` and `text-embedding-3-small-01`, now gone); the 45 written off-topic rows were made on
  2026-11-19 through `text-embedding-3-large` and `text-embedding-3-small` (model version 1).
- Deployment drift check (embeddings expert's condition): 10 s03 fixture queries and 10 fixture whole-node texts
  (pinned by id and file sha256) embedded through today's deployment against the floors run's cached vectors; pass when
  every cosine >= 0.999 (the expert's rule of 2026-11-20: the cosine bar alone; the earlier "at most one int8 step"
  clause was withdrawn as too strict for hosted). text-embedding-3-small@256: min cosine 0.999999, 19 of 20 texts
  identical in int8. text-embedding-3-large@256: min cosine 0.999659 (one text 146 of 256 components and 3 steps
  apart); two more passes differ from each other on 3 of 10 whole-node texts (up to 89 components, 2 steps) and from
  the first pass on queries, with min cosine 0.9999 or above everywhere. The expert's reading: GPU nondeterminism, not
  a model change (which would move every text to about 0.9 to 0.99); its effect on a similarity score is at most about
  0.0003, a tenth of the 0.003 label tolerance. Both hosted keys pass and keep their cuts. A failed check (min cosine
  below 0.999) would withhold a key's cuts (`cuts_measured`, `cuts` null).
- Reproducibility (embeddinggemma-2@256+p1, Metal, this build): 20 fixture queries embedded 10 times gave identical
  int8 rows (0 of 180 differ). Against the query rows recorded on 2026-11-11 on the same machine and backend by an
  older umx build, 19 of 20 rows differ by one int8 step in 168 components in all, moving a cosine against the
  fixture's 900 document rows by at most 0.0023. Hosted keys: not measured.

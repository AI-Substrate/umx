---
id: 122
title: Similarity floors are calibrated per model; read floor_source before trusting a cut
route: explore
status: superseded
superseded_by: 138
source: ws004 s26 (plan 093); embeddings-expert ruling 2026-11-11 12:07; docs/project/benchmarks/floor-calibration/RESULT.md
reqs: [req-53, req-65]
supersedes: none
---
## Do
Read each search's `floors` entry and each similar_to answer's `floor_source`. `calibrated` (with `floor_calibrated: true` and a `floor_run`) means the cut was measured for that model. For similar_to, a score at or above it is higher than 99 in 100 unrelated document pairs. A search floor starts at that 1-in-100 noise level and may be lowered to keep labelled retrieval quality (BEIR nDCG@10 within 0.01 of no floor), so it is a quality-preserving cut, not a 1-in-100 bound: text-embedding-3-large@256's 0.409 sits below even its 1-in-20 noise level. Don't read a 99th-percentile guarantee into `calibrated: true` for search. `config` means the machine config replaced it. `graph` (similar_to only) means the graph's own `embedding.similarity.floor`. `none` means the model is not calibrated, so nothing is cut and only the limit (search) or k (similar_to) applies; loading such a graph warns `floor_uncalibrated` once.

Expect vector rows for paraphrases on a calibrated model: the calibrated search floor sits at the noise level of that model (0.409 for text-embedding-3-large@256, 0.4495 for text-embedding-3-small@256, 0.7401 for embeddinggemma-2@256+p1), not at a guessed 0.5.

To override a floor, set it in the MACHINE config only, then restart the daemon:

```bash
umx config set embedders.azure.search_floor 0.3
umx config set embedders.azure.similar_floor 0.5
umx daemon restart
```

Before you change a floor (or any graph setting such as the embedder, policy, reembed, model or field roles), tell the graph's owning seat in one line first.

To calibrate a model that has no floor, run umx-bench's `floor-calibration --embedder <instance>` (it sends the BEIR corpora through that instance, millions of tokens on a hosted model, so ask first) and commit `floors.json` and the table row it checks.

## Don't
Don't put `search_floor` or `similar_floor` in a repo's `.umx/config.toml`: a repo cannot set them (`config_repo_ignored`), and no env var or flag sets them either. Don't read a `config` or `none` floor as a relevance cut; it is whatever someone set, or nothing. Don't compare floors across models: each model's cosines live on their own scale. Notes 081 and 096 say `floor_calibrated` is false for every model; that no longer holds for the three calibrated keys.

## Example
```bash
umx search "project risks and concerns" --graph demo
# floors: [{"model": "text-embedding-3-large@256", "floor": 0.409, "calibrated": true,
#           "floor_source": "calibrated", "floor_run": "fc-2026-11-11-06902fe"}]
umx similarity rebuild --graph demo
# floor_source "config" after umx config set embedders.<name>.similar_floor ... and a restart
```

## Why
The uncalibrated 0.50 floor cut almost every paraphrase on text-embedding-3-large@256 (33 of 41 rollout probes had no vector row), because that model's cosines for related text sit around 0.5. Each model's similar_to floor is now the p99 of its own document-to-document noise. Its search floor starts at the p99 of query-to-document noise and is lowered in 0.01 steps until BEIR search quality is within 0.01 of no floor (both OpenAI models' were lowered, always because of NFCorpus). Unknown models get no floor rather than a guess. Changing the similar_to floor makes stored lists stale through `_meta.tsv`, so they are rebuilt.

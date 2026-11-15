---
id: 138
title: Search floors are cut to keep labelled quality on BEIR and umx's own fixture; read floor_source before trusting a cut
route: explore
status: current
source: ws004 s26b; embeddings-expert rulings 2026-11-11; docs/project/benchmarks/floor-calibration/RESULT.md
reqs: [req-53, req-65]
supersedes: 122
---
## Do
Read each search's `floors` entry and each similar_to answer's `floor_source`. `calibrated` (with `floor_calibrated: true` and a `floor_run`) means the cut was measured for that model. For similar_to, a score at or above it is higher than 99 in 100 unrelated document pairs. A search floor starts at that 1-in-100 noise level and is lowered, 0.01 at a time, until labelled retrieval quality holds on both sets: BEIR nDCG@10 within 0.01 of no floor, and umx's own synthetic fixture's search recall@10 within 0.02 of no floor. It never goes below the cosine half of unrelated pairs reach. So a search floor is a quality-preserving cut, not a 1-in-100 bound; don't read a 99th-percentile guarantee into `calibrated: true` for search. `config` means the machine config replaced it. `graph` (similar_to only) means the graph's own `embedding.similarity.floor`. `none` means the model is not calibrated, so nothing is cut and only the limit (search) or k (similar_to) applies; loading such a graph warns `floor_uncalibrated` once, naming the uses left without a floor.

Expect vector rows for paraphrases on a calibrated model. The calibrated search floors are now 0.7101 for embeddinggemma-2@256+p1 (was 0.7401), 0.409 for text-embedding-3-large@256 (unchanged) and 0.4095 for text-embedding-3-small@256 (was 0.4495); `umx docs get search` has the table. On a local embeddinggemma-2 graph more vector rows now pass than under note 122's floor.

To override a floor, set it in the MACHINE config only, then restart the daemon:

```bash
umx config set embedders.azure.search_floor 0.3
umx config set embedders.azure.similar_floor 0.5
umx daemon restart
```

Before you change a floor (or any graph setting such as the embedder, policy, reembed, model or field roles), tell the graph's owning seat in one line first.

To calibrate a model that has no floor, run umx-bench's `floor-calibration --embedder <instance>` (it sends the BEIR corpora and the fixture through that instance, millions of tokens on a hosted model, so ask first) and commit `floors.json` and the table row it checks. `--dry-run` estimates the tokens and sends nothing; `--record` (re-recording the fixture vectors CI uses) always embeds and is refused with `--dry-run`.

## Don't
Don't put `search_floor` or `similar_floor` in a repo's `.umx/config.toml`: a repo cannot set them (`config_repo_ignored`), and no env var or flag sets them either. Don't read a `config` or `none` floor as a relevance cut; it is whatever someone set, or nothing. Don't compare floors across models: each model's cosines live on their own scale. Notes 081 and 096 say `floor_calibrated` is false for every model; that no longer holds for the three calibrated keys.

## Example
```bash
umx search "project risks and concerns" --graph demo
# floors: [{"model": "text-embedding-3-large@256", "floor": 0.409, "calibrated": true,
#           "floor_source": "calibrated", "floor_run": "fc-2026-11-11-cdc1965"}]
umx similarity rebuild --graph demo
# floor_source "config" after umx config set embedders.<name>.similar_floor ... and a restart
```

## Why
Note 122's rule gated the search floor on BEIR alone, which left embeddinggemma-2 at 0.7401. That cut the paraphrases in umx's own fixture (hybrid recall@10 0.963 to 0.825 on whole-node rows), and CI's relative bound missed it. The rule now also gates on the fixture, measured in the same run through umx search, and CI checks absolute recall (hybrid at least 0.95, vector at least 0.94, within 0.02 of no floor) on fixture vectors recorded through the production path; a model or text-form change that would leave those vectors stale fails CI. Similar_to floors are unchanged. Unknown models still get no floor rather than a guess.

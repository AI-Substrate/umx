---
id: 111
title: Search words match their other forms and, from 4 letters, their prefixes; each hit says how
route: explore
status: current
source: ws004 s21 (plan 075); tester friction (a tester, on a private graph, 2026-11-11); embeddings-expert, graph-storage-expert and architect agreement (2026-11-11)
reqs: [req-92, req-68]
supersedes: none
---
## Do
Type the word the way it comes to mind: `umx search vendor` also finds "Pre-approve listed vendors" (stem), `umx search vendors` finds "Vendor onboarding", and `umx search approv` finds approve, approval, approved and "Pre-approve" (all share the stem `approv`). From 4 letters a word also matches the words that start with it: `vend` finds `vendor` and `vendorship`; `approva` finds `approval`.

Read each hit's `word_terms` to see how it matched: `{"query": "vendor", "term": "vendors", "form": "stem"}` (human output: `vendor ~ vendors (stem)`). Forms rank exact, then stem (0.7), then prefix (0.4), so with nodes alike the word as you typed it comes first.

On `word_prefix_capped`, the word starts more than 64 words and only the 64 most common were matched as prefixes: type more of it, or the whole word.

## Don't
Don't expect a 3-letter word to act as a prefix: `app` matches `app` (and its forms), not `apple`. Don't expect stems or prefixes for words with digits or letters outside a to z: `iso2700` does not find `iso27001`, and `café` matches only `café`. Don't expect typo tolerance from search (`aproval` still matches nothing); `umx node find` stays the typo-tolerant title lookup and compares words as written. Don't read a prefix hit as strong evidence: it counts 0.4 of an exact match and holds only 0.4 of its word's coverage in fusion.

## Example
```bash
umx search vendor --graph demo
# rows: "Vendor onboarding" (word_terms vendor exact), "Pre-approve listed vendors" (vendor ~ vendors, stem),
#       "Vendorship terms" (vendor ~ vendorship, prefix)
umx search approv --graph demo   # approve, approval, pre-approve (stem approv)
```

## Why
Agents type stems, singulars and the start of a word, and the word leg missed them (a tester: `vendor` missed "vendors"; `approv` found nothing). With no embedder the word leg is all there is, so it has to meet those forms. Stems use the English Porter2 algorithm on index and query alike; prefixes are capped so a short one cannot flood the list, and lower weights keep the exact word on top and protect hybrid fusion's recall.

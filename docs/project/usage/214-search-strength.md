---
id: 214
title: Read a search hit's strength, and stop on no_strong_match
route: explore
status: current
source: PRD 002 search-10 (project lead ruling 2026-11-16: strength in plain words)
reqs: [req-30, req-37, req-139]
supersedes: none
---
## Do
Read each hit's `strength` before opening it. A hit found by meaning is `strong`, `fair` or `weak`: the label measures it against what this query scored across every node it searched, in its own model's space, so it says how much this hit stands out in this graph for this query. Open `strong` hits first. Treat `fair` as plausibly relevant, weakly matched. Read the text of a `weak` hit before you trust it: it is near the query only about as much as many nodes in the graph are. An exact match is `exact`; a hit found by words only has `strength: null`. Strength reads the whole node: a hit found through a field vector can rank high and still be labelled weak. Labels stop (null) above about 17,000 nodes searched in one model's space, twice the largest graph the cuts were measured on, until larger graphs are measured.

When the reply carries the hint `no_strong_match` (no match it found, shown or not, is strong, fair or exact, and every match came from a graph whose meaning was measured), the graphs searched may not hold the subject. Reword the question, or add a phrasing with `--also <words>` (its next is the same search with one more `--also <words>`), or stop and tell the user the graph does not seem to hold it. It comes first among the hints.

## Don't
Don't read a label as a fact about the node: it is relative to this query and the nodes searched, so the same node can be `strong` for one question and `weak` for another, or change when the graph grows. Don't compare labels across models, and don't expect labels where none were measured: a model with no measured strength cuts, too few nodes searched in a space, or a search with no meaning leg gives no labels and no `no_strong_match` (the order and scores are the same either way). Don't expect labels to be the same across backends or umx releases: for a local model they are the same on any machine running the same model file on the same backend with the same umx release; across backends (CPU, Metal, CUDA) or releases a label can differ only for a hit whose cosine is within 0.003 of a cut. Hosted strength cuts are measured for one model version; if the provider changes the model behind a deployment, or the service is nondeterministic, labels near a cut can flip until re-measured. Hosted similarity scores are nondeterministic, measured on two hosted keys (small identical, large about 0.0003); a hit within about 0.003 of a cut can flip.

## Example
```bash
umx search "sign-off is slow" --graph demo
# rows[0]: title "Approval delays", legs vector, strength strong
# rows[3]: title "Vendor onboarding", legs vector, strength weak
umx search "tide tables for sailing" --graph demo
# hint no_strong_match  next: umx search <words> --also <words> --graph demo
umx search "tide tables for sailing" --also "harbour schedules" --graph demo
```

## Why
Cosines inside one graph sit close together, so a raw score does not say whether a hit is a real match. umx measures each hit against the median and spread of every node's score for this query (exact order statistics, the same on any machine for the same vectors) and against cuts measured per model on public data and umx's own fixture, chosen so a query about a subject the graph does not hold gets a strong best hit at most 1 in 100 times and a fair-or-strong one at most 1 in 10. `no_strong_match` turns that into a stop signal, so an agent rewords or stops instead of reading noise.

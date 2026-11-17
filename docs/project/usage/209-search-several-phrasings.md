---
id: 209
title: Give several phrasings of one question to one search with --also
route: explore
status: current
source: PRD 002 search-09 (project lead ruling 2026-11-16: fewer calls for agents)
reqs: [req-30, req-68, req-49, req-53]
supersedes: none
---
## Do
When one wording may miss what you are looking for, give the other wordings to the same search instead of running several searches: `umx search "<words>" --also "<words>" --also "<words>"`. Up to 4 `--also`, so 5 phrasings in all. Good second phrasings are the plain words beside the domain term, a paraphrase of the question, or the words a note would use.

Read the reply as one list. Each phrasing is searched alike (exact, words and meaning) over the same graphs, `--within`, `--along` and floor, and a node's `fused` sums its credit over the phrasings that found it, so the nodes several phrasings agree on rank first. Each hit's `phrasings` says which found it, by position: 1 is the main text, 2 to 5 each `--also` in order (`[1, 3]`; `by` in `--human`). A hit's `legs`, ranks, scores and text are those of its best phrasing: a node found by words in one phrasing and by meaning in another shows only the winner's leg in `legs`, while `phrasings` says both found it.

When the reply carries `search_results_cut`, its next keeps one `--also <words>` per phrasing: fill in your phrasings in the same order to see the rest ranked the same way.

## Don't
Don't put unrelated questions in one search: phrasings are fused as one question, so a node that answers only one of them ranks as if it half answered both. Ask separate questions separately. Don't repeat a phrasing with other case or spaces, give one with no letter or digit, or give more than 4 `--also`: each is refused `usage_invalid`, which names the phrasing's position, never its text. Don't put a secret in any phrasing: if one looks like a secret, no hosted embedder is asked for any of them (those graphs are searched by words only, with `similarity_unavailable`); a local embedder still gets them all. Don't expect a reply to repeat your phrasings: hints, nexts and errors name them by position (only single words can show: `word_terms` lists those a hit matched, and `word_prefix_capped` names one that starts too many words).

## Example
```bash
umx search approval delay --also "sign-off waits" --also "invoices stuck in review" --graph demo
# rows[].phrasings: [1, 2] on the node both wordings found, [3] on one only the third found
umx search approval delay --also "sign-off waits" --graph demo --limit 5
# hint search_results_cut  next: umx search <words> --also <words> --limit 9 --graph demo
umx search approval delay --also "sign-off waits" --graph demo --limit 9
umx search approval delay --also "Approval Delay"   # usage_invalid: phrasing 2 (the first --also) repeats an earlier phrasing
```

## Why
Agents often search the same question two or three ways because one wording misses. Each search is a separate call and a separate query embed, and the agent then merges the lists by hand. One search with `--also` makes one embedder call for every phrasing (one call per embedder instance, under the same 5 s bound), runs the legs per phrasing and sums reciprocal rank credit over them, so agreement between wordings counts. Exact matches stay a tier above, whichever phrasing matched. Phrasings are named by position because umx never repeats typed text back.

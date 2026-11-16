---
id: 078
title: umx search finds exact, word and meaning matches and says which found each hit
route: explore
status: current
source: ws004 s11 (plan 040); workshop 008 section 11; embeddings-expert and architect rulings (2026-11-10)
reqs: [req-30, req-37, req-53, req-68]
supersedes: 067
---
## Do
Ask "which nodes are about this?" with `umx search <query>`. Exact matches come first: the whole query equal to a node's title, id, type, status, created, updated or created_by, or a non-text field value (a number, date, yes/no). Then words (BM25 over title, note and string fields) and meaning (vectors, when the graph has them) are fused into one list. Read each hit's `legs` (`word`, `vector`, `both`, or `exact`), `exact` (the field that equals your query), `word_rank`/`bm25`/`word_field`/`coverage` and `vector_rank`/`cosine`/`vector_field`. A word hit's credit is its `coverage` (the share of your query's informative words it holds), so write queries with the words that matter; common words alone (`word_leg: no informative terms`) leave ranking to meaning.

Search one graph with `--graph demo`, several with `--graph demo,ops`, or every loaded open graph with neither; secured graphs are searched only when you name them (`secured_graphs_skipped` counts the rest, never names them). Narrow with `--within <node>` (an address `umx:<graph>/<id>` with several graphs).

When a hint (`similarity_unavailable`) or a warning (`embedder_mismatch`, `embedder_not_local_for_secured`) says meaning was not used, the results are still right for words: keep working, and read its next step if you need meaning. `similarity_incomplete` means some nodes have no vector yet; their words still find them.

## Don't
Don't read an empty vector leg as "nothing similar": a hint or a warning always says why it was not used. Don't compare `cosine` values between models, or `fused` between searches over different graphs. Don't expect typo tolerance from search; use `umx node find` to name a node by a half-remembered title. Don't expect a secured graph's nodes from a search that does not name it.

## Example
```bash
umx search "01JC8XA4H7" --graph demo        # exact: the node with that id, first
umx search "feline nutrition" --graph demo  # vector: "Cat food shortage", legs vector, cosine 0.71
umx search approval delay                   # both: word_field note and vector_field @node
```

## Why
Words find what a node says; vectors find what it means; exact matches find ids and values (req-30). Reciprocal rank fusion needs no score scaling between BM25 and cosines, and an exact match is never outranked by a near paraphrase. Search degrades to words and says so (req-53), compares vectors only within one model (req-37), and never sends a secured graph's query off the machine.

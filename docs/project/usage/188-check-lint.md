---
id: 188
title: Lint a shared graph with umx check --lint before others read it
route: doctor
status: current
source: F139 (pm-001 2026-11-13; security's ruling on the lint terms file)
reqs: []
supersedes: none
---
## Do
Before you share a graph, or after a long ingest, run `umx check --graph <alias> --lint`. It reads only and changes nothing. Each kind it finds gives one hint, with the count, the first node and a next to fix it. `data.lint.rows` lists every finding (graph, kind, id, and the place, named as a read's `secret_suspected` names it):
- `orphans`: nodes with no edge at all (std anchors such as meetings and segments may stand alone). Link each to what it is about.
- `proposed_keys`: a usable `entity_key` on a node still `proposed`. Settle the node or drop the key.
- `terms`: a place holding a word from this machine's lint terms file. Reword it. Every text a read shows is scanned, edge fields and the graph's title too. A list entry is named by its position, counted from 1 (`evidence:1` is the first pointer), and a custom field by its place among the record's custom fields as its line holds them, counted from 1 (`field 2`), never by its name; a read's `secret_suspected` and a redact's candidates number them alike, and the hint's first place reads the same (`<id> field 2`).
- `double_negatives`: a title that says "lacks no". Say it plainly.
- `raised_in_evidence`: a `raised_in` link whose node's evidence and anchor's evidence share a source but never overlap, one row per link. The row gives the nearest pair of pointers; fix the link or the pointer.

Each graph lists at most 100 rows per kind. `lint_truncated` names the next page as `umx check --graph <alias> --lint --kind <kind> --offset <n>`. `data.lint.skipped` names a kind that did not run, and why (`sensitivity_label` always: umx has no such field).

To scan for words that must stay out of graphs (a client's name, a code word), write `$UMX_HOME/lint/lint-terms.json` by hand: `{"umx_lint_terms": 1, "terms": [{"term": "<word>"}, {"term": "<word>", "graphs": ["<graph id>"]}]}`. Give it mode 0600 in a 0700 folder. Each term is 2 to 100 characters, with at most 256 terms, matched in any case.

## Don't
Don't put credentials in the terms file. It is a plain local file that any process of yours can read, and `secret_suspected` already covers secrets. Don't expect a hint to repeat the term or the text: it names the node and the place only. Don't make the file readable by others: umx then skips the scan with `lint_terms_unusable`. Don't treat a lint hint as an error: `check --lint` never refuses and never changes the exit code.

## Example
```bash
umx check --graph launch --lint
umx check --graph launch --lint --kind orphans --offset 100
```

## Why
A graph that others read should not carry loose nodes, unsettled identity keys, words that must not leave this machine's team, or links that contradict their own evidence. The lint finds these before a reader does, and names each place so the fix is one command away.

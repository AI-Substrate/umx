# text-embedding-3-small@256 rows for the s03 fixture's off-topic queries (synthetic text only)

PRD 002 search-10. The same 45 written off-topic queries as `embeddinggemma-2_256+p1.offtopic.rows.tsv`
(`../offtopic.jsonl`, sha256 `cc99b6ad389f8fb8737f2aa96b9790727ca4b37ffc1c14b0c4ebb2119e9a3a41`), embedded on this hosted key
so its strength cuts get written-list off-topic evidence (project lead's yes, 2026-11-20).

- File: this note's rows file (the key with `@` written `_`, `.offtopic.rows.tsv`), 45 lines, sha256 `41e72fefae34777c5f214b25db578fddbb3ef929e8ed5ab9f94050539d07ea35`. Line: `query TAB <umx TextHash of the
  query text> TAB <base64 of the int8 row>`, sorted, the format of the gemma rows.
- Producer: umx-bench `floor-calibration --strength --embedder small --offtopic-rows <this file> --send-offtopic`,
  2026-11-20 (UTC 2026-11-19), at the search-10 PR head (base umx c5485aa). Exactly one embed call of the 45 texts
  read from offtopic.jsonl (nothing else is sent: `--send-offtopic` is the only way the tool sends anything in
  strength mode), `TextKind::Query` through umx's own adapter (`azure-openai.embedder`), instance kind hosted,
  model text-embedding-3-small, dims 256. 45 texts sent, about 1,254 tokens (umx's estimate). The rest of the run read the floors
  run's vector cache offline.
- Deployment: `text-embedding-3-small` (model version 1). The floors run fc-2026-11-11-cdc1965 used the deployment
  `text-embedding-3-small-01`, which no longer exists on the endpoint; umx keys vectors by model and dims, not deployment.
- Used by: the strength measurement's `written` off-topic source for this key (floors.json `strength`).

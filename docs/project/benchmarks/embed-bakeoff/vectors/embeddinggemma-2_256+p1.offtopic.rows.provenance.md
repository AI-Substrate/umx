# embeddinggemma-2@256+p1 rows for the s03 fixture's off-topic queries (synthetic text only)

PRD 002 search-10. `../offtopic.jsonl` holds 45 queries on subjects no fixture node is about (astronomy, music, sport,
geology, history, wildlife and the like), written for the strength measurement and checked against the fixture's
titles: no title shares a subject with them; the words they share are generic (`years`, `team`, `winter`,
`workshops`, `migration`).

- File: `embeddinggemma-2_256+p1.offtopic.rows.tsv`, 45 lines, sha256
  `1913a5604ad85ef02a2ecba9c2917bb6d26834033c54c9c849ca4be265f29c8a`; `../offtopic.jsonl` sha256
  `cc99b6ad389f8fb8737f2aa96b9790727ca4b37ffc1c14b0c4ebb2119e9a3a41`.
- Line: `query TAB <umx TextHash of the query text> TAB <base64 of the int8 row>`, sorted, the same format as
  `embeddinggemma-2@256+p1.rows.tsv`; umx-services' hybrid_recall serves both files by text hash.
- Producer: umx-bench `floor-calibration --strength --embedder gemma ... --repeat 10 --offtopic-rows <this file>`,
  2026-11-19, on branch s8/search-10-strength (base umx 0222cfca), instance kind local, model embeddinggemma-2, dims
  256, umx's llama adapter, backend Metal, through the query embed (`TextKind::Query`) as umx search sends it. Three
  runs of the tool wrote the same bytes.
- Model: the pinned ggml-org/embeddinggemma-2-GGUF file (umx_core::models), as for the fixture rows.
- Used by: the strength measurement's `written` off-topic source for this key (floors.json `strength`), and
  hybrid_recall's `strength_labels_paraphrase_targets_strong_and_off_topic_queries_nothing`.

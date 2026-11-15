# embeddinggemma-2@256+p1 rows for the s03 fixture, every production row (synthetic text only)

Supersedes `embeddinggemma-2@256+p1.tsv` (s11: whole-node rows only, field rows were zero in hybrid_recall) as what
`crates/umx-services/tests/hybrid_recall.rs` reads (ws004 s26b, embeddings-expert ruling 2026-11-11, option 1).

- File: `embeddinggemma-2@256+p1.rows.tsv`, 980 lines (900 document: whole node, title and note for each of the 300
  nodes; 80 query), sha256 `0473ec0c263ee4c7a1405803c2e3b2d701c0580e1dedce9f123a7b8a2e42eaf0`.
- Line: `<document|query> TAB <umx TextHash of the embedded text: first 16 bytes of its sha256, hex> TAB <base64 of the
  int8 row>`, sorted. int8 = umx_core::vectors::quantise. hybrid_recall serves a row by the hash of the text it is asked
  to embed and counts any text with no row; CI requires none, so a changed text form fails loudly.
- Texts: text form v1 (`WHOLE_NODE_VERSION` 1, graph `text_form` 1), exactly as umx makes them: each fixture node added
  through umx's services, its texts planned by umx's embed planner (umx_core::vectors::node_texts), queries through
  umx search's own query embed. The fixture has no custom fields, so text form v2 gives the same texts.
- Producer: `umx-bench` bin `floor-calibration --embedder gemma --cache <fresh empty dir> --record <file>` at branch
  s004/s26b-gemma-floor commit b21ca71b, 2026-11-11, instance `embedders.gemma` (kind local, model embeddinggemma-2,
  dims 256), umx's llama adapter (`llamacpp.embedder`, default config), backend Metal. All 980 texts embedded in that
  run (empty cache), none truncated. The hash column was cut to TextHash's 16 bytes after the run (the build wrote the
  full sha256); the harness now writes TextHash directly.
- Model: ggml-org/embeddinggemma-2-GGUF @ 6ea3e3a309d12a4779cfda98b8c19800d2d5da71, embeddinggemma-2-BF16.gguf,
  sha256 68bae29d62fb8c7d23e98d21fd4662753ddd636e6b62b8a70dfe059a9844f216 (the pin in umx_core::models).
- Metal and CPU vectors differ in the last bits, so a CPU rerun may change a few int8 values by one.
- Machine-readable: `embeddinggemma-2@256+p1.rows.json`. hybrid_recall's
  `the_recorded_rows_match_the_current_model_and_text_form` refuses the rows when the pinned model (repo, commit,
  file, sha256), the whole-node version, the field form or the rows file's own sha256 differs, so a model or text-form
  change cannot leave CI measuring stale vectors. Re-record with `floor-calibration --record` and update both files.

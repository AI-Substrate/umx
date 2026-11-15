# embeddinggemma-2@256+p1 vectors for the s03 fixture (synthetic text only)

- File: embeddinggemma-2@256+p1.tsv (made as /tmp/s11-embeddinggemma-2@256+p1.tsv by an agent), 380 lines (300 document, 80 query), sha256 590cfd397c6f6d98945182ab75abae6e9385b8983a641e8d50e65dcda5f67b43.
- Line: `<fixture id> TAB <document|query> TAB <base64 of int8>`, int8 = umx_core::vectors::quantise (round_half_even(x * 127 / max|x|)), standard base64 with padding.
- Input: /tmp/s11-fixture-texts.jsonl, sha256 b04156a4e3bd634d79d45a8c78cd058fedbd071959b6250632176f30c8c7e154. Documents embedded as TextKind::Document (no prefix: the text is the v1 whole-node form); queries as TextKind::Query (the adapter adds "task: search result | query: ").
- Model: ggml-org/embeddinggemma-2-GGUF @ 6ea3e3a309d12a4779cfda98b8c19800d2d5da71, embeddinggemma-2-BF16.gguf, sha256 68bae29d62fb8c7d23e98d21fd4662753ddd636e6b62b8a70dfe059a9844f216.
- Producer: umx-adapter-llamacpp at faadf218, default config (Metal on macOS, 1 session x 4 threads, batch 16, width 256), through a throwaway crate: `s11-dump /tmp/s11-fixture-texts.jsonl <out.tsv> ~/.cache/umx/models`. Backend reported: Metal. No text truncated.
- Check (Python, an independent decoder): every line decodes to 256 values and re-encodes byte-identically; the query ids match s03; fixture recall@10 0.969, MRR@10 0.826 against s03's relevance labels.
- Metal and CPU vectors differ in the last bits (both pass parity >= 0.9998 against the s03 reference), so a CPU rerun may change a few int8 values by one.

Superseded for hybrid_recall by `embeddinggemma-2@256+p1.rows.tsv` (every production row; ws004 s26b). Kept as the
s11 record.

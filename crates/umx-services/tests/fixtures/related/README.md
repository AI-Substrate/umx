# Related-on-write fixture (req-123, ws004 s28; synthetic text only)

`nodes.jsonl` is a synthetic fixture shaped like the five reported pairs: 15 existing nodes (the five pair partners,
a random control partner `picnic`, a distractor `receivables` and background topics) and 6 new nodes. Pairs 1, 2 and 4
are the same meaning in other words and must come back as related; pairs 3 and 5 are the same topic with another
claim and are only printed; the control new node is about nothing an existing node is about and must list nothing.
`related_recorded.rs` asserts this at each key's calibrated floor.

Rows: `<document|query> TAB <umx TextHash of the embedded text, hex> TAB <base64 of the int8 row>`, sorted, the format
of the embed-bakeoff rows. Every text the fixture embeds: each existing node's whole node, title and note as documents
(umx's embed loop), each new node's whole-node text as a query (umx's related check). The test serves rows by text hash
and fails on any text with no row, so a changed text form must be re-recorded.

Each key's rows are in `<key>.rows.tsv`, the key's `@` written `_`.

| key | rows | sha256 | made by |
|---|---|---|---|
| `embeddinggemma-2` 256 dims, prompt 1 | 51 | `8f72ac37bf52f9d6e7c7c540e4ba47a2221c6908f676677cf32c8ae0246298b5` | umx's llama adapter, embeddinggemma-2-BF16.gguf at the pinned commit, backend Metal |
| `text-embedding-3-large` 256 dims | 51 | `08d28ca8e9e75145bbfb32d77060a19d7769ad6a456c7b99e76a64f4fe60cc74` | umx's Azure OpenAI adapter, text-embedding-3-large at 256 dims |

`nodes.jsonl` sha256 `76fe67f9c8c5f3291b8b9c3da534f8f6487e6e1fc57ed151f1bb4a44a134e1af`. Recorded 2026-11-17 on top of
umx 9bfa57c with text form v2, by:

    scripts/cargo-slot build -p umx-bench --bin related-record
    target/debug/related-record --embedder <instance> --fixture crates/umx-services/tests/fixtures/related/nodes.jsonl \
        --out crates/umx-services/tests/fixtures/related/<model>_<dims>[+pN].rows.tsv [--config-home <umx home>]

Metal and CPU vectors differ in the last bits, so a CPU re-record may move a few int8 values by one.

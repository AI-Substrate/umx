# lbug-conformance

Workstream 005: the Cypher subset is the set of queries umx and LadybugDB 0.22.3 both accept and answer with the
same rows (cypher-expert design input, section 0). This checker is LadybugDB's side. It loads a fixture folder
(`nodes.jsonl`, `edges.jsonl`, `queries.json`) into an in-memory LadybugDB and runs every query:

- a query marked `conformance: true` must return its `expected` rows, or the run exits 1;
- an alias query runs its `alias_of` twin in LadybugDB; umx runs the original spelling against the same rows;
- a probe with `lbug_error` must fail with an error containing that text, or the run exits 1;
- other probes are recorded only: their rows or LadybugDB's error.

`type(r)` on a single-hop relationship is an intentional umx departure: LadybugDB 0.22.3 rejects it, so its
conformance twin uses `label(r)`. A separate checked probe notices if LadybugDB starts accepting `type(r)`.

The fixture is a umx graph folder (`graph.json`, `nodes.jsonl`, `edges.jsonl`); `evidence_kinds` is derived from
each node's `evidence`, as umx does. umx keeps one edge of a reversed pair of a symmetric relation (D49, F48:
`conflicts_with`, `alternative_to`, `possibly_same_as` and graph.json's `ontology.symmetric`), and LadybugDB
would keep both, so the checker refuses a fixture that stores such a pair (exit 2) instead of comparing it.

umx's side runs the same file through its own executor (`crates/umx-cypher/tests/design.rs`, and
`crates/umx-query/tests` over umx-graph). Two engines, one
file: a mismatch on either side fails, and the diff says which engine moved.

This crate is its own workspace, listed in the root `Cargo.toml` `exclude`. No umx crate depends on it and
`just check` never builds it: umx never links LadybugDB (D18).

## Run

```sh
tools/lbug-conformance/run.sh                                  # the design fixture, result in target/design-lbug.json
tools/lbug-conformance/run.sh <fixture_dir> <out.json>
```

It prints one line per query and a summary with the engine version and `recursive_pattern_semantic` (LadybugDB's
walk setting, recorded on every run). CI runs it (`.github/workflows/conformance.yml`) on changes to the Cypher
crates or this folder.

## What it downloads, and what it links

The lbug crate's build script picks its library from the caller's env (`LBUG_LIBRARY_DIR`, `LBUG_SHARED`,
`LBUG_LINUX_VARIANT`, `LBUG_VERSION`, `LBUG_SOURCE_DIR` and more), from a legacy cache inside its registry folder
that nothing verifies, or from a download of the latest release with no checksum, and falls back to a source
build. `run.sh` allows none of that:

1. It fetches the v0.22.3 static archive for this platform from `github.com/LadybugDB/ladybug/releases` into
   `~/.cache/umx-lbug-prebuilt/v0.22.3/` (or `$LBUG_PREBUILT_CACHE_DIR`), checks the sha256 pinned in the script
   on every run (cross-checked with GitHub's asset digests on 2026-11-10), and unpacks it into a wiped folder.
2. It clears every inherited `LBUG_*` variable and `DOCS_RS`, and sets only `LBUG_LIBRARY_DIR` and
   `LBUG_INCLUDE_DIR` to that folder: the build script's first branch, before any cache, download or source build.
3. After the build it refuses to run unless lbug's build report names that folder as the library
   (`LBUG_PRECOMPILED_SOURCE=external`) and no other folder the link searches holds a liblbug.

A checksum mismatch, or a build that linked anything else, exits 1. A failed download, or a platform with no
prebuilt library, prints `skipped: lbug unavailable` and exits 0; CI puts that at the top of the job summary and
as a warning, so a skip never reads as a pass.

`test_run.sh` proves the guards with a fake cargo: inherited overrides never reach cargo, a build that linked
another folder or found a stray liblbug is refused, and (with the checker built) a float 3.3e-3 off makes a02
differ, a fixture holding a reversed symmetric pair is refused, and a checked refusal probe changed to an
accepted `label(r)` query fails. CI runs the real check with `LBUG_LINUX_VARIANT=perf`, `LBUG_SHARED=1`, `LBUG_LIBRARY_DIR=/nonexistent`
and `LBUG_BUILD_FROM_SOURCE=1` inherited, then `test_run.sh`.

## Comparing rows

Cells are compared by `crates/umx-cypher/tests/rows/mod.rs`, the same code umx's side uses: floats (printed at
full precision) within 1e-9, everything else exactly.

The checker never runs `LOAD`, so LadybugDB fetches no extension at run time (extensions would go to
`~/.lbdb/extension` and, on macOS, need the binary linked with `-Wl,-export_dynamic`).

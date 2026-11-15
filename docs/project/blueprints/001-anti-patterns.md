# Anti-patterns: Unimatrix (Concept Graph), companion to blueprint 001

Owner: role:architect. First draft by the architecture reviewer, 2026-11-10;
reviewers add rows as they find them. Blueprint 001 decision d-14 makes this list part of the architecture.

This is the list of ways this codebase has gone wrong, or could, against the project's architecture model.
Reviews check new work against it. A new catch gets a row in the same PR round it is found in.

## The model these are measured against

- Ports and adapters. A port is a trait in `umx-core`. An adapter is the implementation of one tool
  (files, clock, ids, later an embedding provider, a vector store, transcripts, the network).
- Adapters do not compose. An adapter wraps one tool and knows nothing of other adapters or of services.
- Services are the implementation of business capabilities, and they are the composition layer. A service
  may compose other services and ports. It never names a concrete adapter; only `umx-compose` does.
- The edges (CLI, daemon) translate. They hold no business rules.
- The tool informs and never polices (D7, D11). Little design up front, cheap to change (D16).

Each row says: what it looks like, why it hurts, where it was seen, and what guards it now.
`guard: review` means nothing mechanical stops it, so a reviewer has to look.

## A. Seen in this project

| id | anti-pattern | what it looks like | why it hurts | seen | guard now |
| --- | --- | --- | --- | --- | --- |
| ap-01 | A rule that can switch itself off | A check returns OK when the thing it checks is missing, renamed or not active yet. | The rule looks green while enforcing nothing. | `r-no-fakes-in-release` passed when `umx` or `umx-fakes` was absent. Checks not yet active were skipped without a word (review). Rule staging compares step names as strings, so workstream 002's `s01` would switch rules off; rules that name a renamed crate go quiet (review of 882fd36, C3, C6). | Missing crate is a violation on the whole workspace; inactive checks are printed by name (d-11). New rules: review. |
| ap-02 | A text scan that spelling can evade | A grep-level check matched one spelling of a thing. | One raw identifier, alias, comment or string gets past it. | `extern crate r#std` passed the no_std scan. A contract clause counted as "run" when its name only appeared in a comment. The no_std check reads the fixed path `src/lib.rs`, so a `[lib] path` or `#[path]` module gets past it (review of 882fd36, C5). | Token-based scan with plants for each spelling. Any new text scan: review, and ask for the evading plant. |
| ap-03 | Proof through the wrong enforcer | A rule is "proven" by a check that is not the one that enforces it, or by a probe that cannot fail. | The proof passes whether or not the rule holds. | A metadata check standing in for a compiler rule (review, led to d-10). A no-network probe that bound 127.0.0.1, which an empty namespace still allows. | d-10: every plant goes through the real enforcer beside a control that must pass. Tests and evidence in PRs: review, ask "what would make this fail?" |
| ap-04 | Purity by proxy | A pure crate calls a helper crate that is not held to the same purity. | The helper does the I/O on the pure code's behalf and the rule is met only on paper. | `umx-graph` was std while `umx-services` (no_std) called it directly (review). | `umx-graph` is no_std (d-3). Any new crate that core or services may depend on: review. |
| ap-05 | Two copies of one truth | A human copy and a machine copy, or docs and code, kept in step by hand. | They drift, and the wrong one gets believed. | d-4 table against `rules.toml`. Three commands with no docs example (first run). Workshop 007 still said human output was the default after D22. Workshop 007's hand-kept error catalogue (replaced by a pointer to the code's catalogue in the PR that adopted this list). PRD still shows req-63 as planned after the change merged. `rules.toml` still says `step = "s05a"`. | Rendered table check, docs coverage test, `describe()` against the matrix, catalogue-vs-CODES test. Prefer deleting the copy to keeping it in step. The wire and its PROTOCOL number are a pair: the wire fingerprint test (in place since blueprint d-18) fails a change without a bump. Anything not generated or tested: review. |
| ap-06 | Working copy and files disagree | The daemon builds or keeps in-memory state without settling it against what is on disk, or keys a graph by path instead of by its id. | Files are the truth (req-01). The next write erases or forks real data. | A graph loaded twice under two paths lost writes. `graph new` over a folder with records but no `graph.json` started empty, and the next write erased them. | Refusals and tests for those two cases. Every new load, reload, watch or registry path: review. |
| ap-07 | A multi-file change that is not all or nothing | An operation writes several files, or memory then files, with no rollback as a unit. | A failure half way leaves a state no command can produce. | A failed `graph new` left a half graph. | Per-file atomic write in the port; rollback on `write_failed` in services. Embedding shards and overlays will bring this back: review. |
| ap-08 | Whole-file rewrite on every write | Each `node add` rewrites and fsyncs all of `nodes.jsonl`. | Cost grows with graph size: 59 ms per add at 10k nodes. | Follow-ups and a benchmark on early changes. | Known and accepted for now (D16). Watch that it does not become the pattern for embedding shards. |
| ap-09 | Checking the label, not the thing | A check compares one hand-kept description with another and never looks at the real object. | Both sides can be wrong together and the check stays green. | `describe()` is a hand table unlinked to `compose()`; the wiring rule and `daemon status` both read it, so a stub wiring a real adapter passes (review of 882fd36, C1). | In place (E-1): describe reads `adapter_id()` from the objects compose built. Any check whose two sides are both hand-written: review. |
| ap-10 | Laundering through a re-export | An allowed crate re-exports a crate the importer may not depend on. | The allow-list sees only Cargo edges, so the layering rule is met on paper. | `umx-daemon` re-exports `umx-compose`'s Config and `umx-protocol`, and `umx` main uses them (review of 882fd36, C2). | In place (E-2): unlisted re-exports fail, use trees and aliases are expanded, library roots come from metadata and `#[path]` fails. Known gap: a nested module named like a root import shadows a `crate::` re-export (follow-up plant queued). Whole-crate re-exports of external crates are covered too (umx-core's `pub use serde_json` was removed in ws002 s05); items are API and are not checked. |
| ap-11 | Enforcer blind to a shipped configuration | A check runs on one host, one feature set or one OS, while the product ships another. | The unchecked configuration is the one that breaks. | Clippy runs without `--all-features`, so stub code is never linted; CI is Linux only while umx is built and used on macOS (review of 882fd36, C4). | In place (E-3): clippy --all-features, a macOS CI job, cargo tree --target all. Fix in place: r-gate-features (ws004 s19). |
| ap-12 | Schema quietly narrows on load | Unknown or repeated records are dropped on load, then written away by the next save. | A git merge that kept both lines, or a newer file, loses data without a word (req-01). | A repeated id in `nodes.jsonl` is dropped (last wins) and erased by the next write; graph.json's format version is never checked; unknown ontology keys are dropped (review of 882fd36, A1, A2). | In place (req-67): repeated ids and unknown keys are kept and warned, never dropped. Every new file format and loader: review. |
| ap-13 | Failure reported after the point of no return | An operation returns an error once its change is already durable, so the caller undoes or retries something that happened. | Disk and memory disagree, or a retry duplicates the write. | The fs adapter returns an error when the folder sync after a successful rename fails, and services then roll back memory (review of 882fd36, A3). | In place (E-5): an error means nothing changed; a failed sync after the rename is `Written::NotSynced`, a warning (gf_c13). |
| ap-14 | Reserved but not produced | A name is reserved in core for a value that only an edge computes, or that nobody computes. | The edge fakes it, and every other edge gets wrong data. | `evidence_kinds` is reserved in core and the daemon returns `[]` (review of 882fd36, B1). | In place (E-6). New reserved names: review that core produces them. |

## B. Watching for (from the model; not yet seen)

| id | anti-pattern | what it looks like | guard now |
| --- | --- | --- | --- |
| ap-20 | Business logic in an adapter | An adapter validates, decides, defaults or reshapes domain data beyond what its tool needs. | review |
| ap-21 | Adapter composing an adapter | One adapter calls or depends on another. | `r-adapters-isolated` |
| ap-22 | Service naming a concrete adapter | A service, or anything but `umx-compose`, depends on an adapter or a fake. | `r-only-compose-wires` |
| ap-23 | Business logic at the edge | A rule lives in the daemon's verb handlers or the CLI, so a second edge (UI, bench) would not get it. | review |
| ap-24 | Service that knows its edge | A service formats output or depends on wire types, flags or command text. | `umx-services` cannot depend on `umx-protocol`; the rest is review (see c-2). |
| ap-25 | Copy instead of compose | Two services carry the same capability because one did not call the other. | review |
| ap-26 | Service reaching into another's insides | A service uses another service's private state instead of its public calls, or two services depend on each other in a cycle. | review. All services are one crate, so nothing mechanical checks this (see c-1). |
| ap-27 | Port shaped like its first adapter | A port's types or calls leak the tool behind it (paths, HTTP details, a vendor's error type). | Contract suite per port; shape is review. |
| ap-28 | Port with nothing external behind it | A trait and fake for our own pure code, or a port added before a second implementation is in sight. | d-1 decided this for the graph. New ports: review against D16. |
| ap-29 | Lax fake | A fake passes where the real adapter would fail (shares references, ignores order or atomicity). | Shared contract suite; clause quality is review. |
| ap-30 | Policing | A write is refused for being off-vocabulary, where D7 and D11 say accept and warn. | review |
| ap-31 | Polling | A loop, tick or timer that runs when nothing has changed (req-06). | Idle CPU test; new background work is review. |
| ap-32 | Env or secrets read outside compose | | clippy and `r-env-in-compose` |
| ap-33 | Graph text in a message or a next step | Node text reaches `message` or `next`, where an agent may act on it (req-41). | Typed slots and fill tests. New error codes: review. |

## C. Candidates, with the architect's rulings

| id | observation | ruling |
| --- | --- | --- |
| c-1 | Every capability is an `impl Services` block on one struct that holds every port and every loaded graph (7 files). Services do not compose each other today; they are methods on one object. | **Fine for now; split before the next port** (blueprint d-12). With three ports one struct is the cheapest shape (D16). Before the next port (Embedder or SourceReader; VectorStore is no longer a port, d-15) lands, split by capability: a graph store service owns the loaded graphs and the writer-facing operations; each other capability is its own struct that takes only the ports it uses and composes other services through their public calls. Trigger: the PR that adds a fourth port must do the split first or in the same PR. |
| c-2 | The pure layers build CLI command text. `umx-core` has `Command::new("node add")` producing `umx ...`; `umx-services` adds `--graph` and `--reason`; the daemon rebuilds the caller's command for retries (`verbs.rs`, about 170 lines). | **Deliberate in substance, wrong in form** (blueprint d-13). A next step is part of the error contract (req-40) and must be built from typed parts (trust rule), so services choosing the next step is right. But `Command` is a `String` that already starts with `umx`, which is ap-24 in small. Before a second edge (the UI) consumes errors, `Command` becomes structured data (verb label plus typed parts: words, names, ids, addresses, placeholders) and only `umx-protocol` renders it to `umx ...` text. The builder calls already take typed parts, so the change stays inside `Command`. |

## D. Enforcement and data fixes queued (architect rulings on the review of 882fd36)

Owners are assigned by the project manager. Each lands with a planted failure that proves it (d-10).

| id | fix | for |
| --- | --- | --- |
| E-1 | Every port trait gets `fn adapter_id(&self) -> &'static str`; `describe(mode)` builds the ports through `compose` and reads the ids from them, so the wiring rule checks what was wired. | ap-09 Done. |
| E-2 | umx-arch fails on `pub use` of a workspace crate, or of an item from one, unless rules.toml lists that re-export by name. List `umx-daemon` re-exporting `Config`, or let `umx` depend on `umx-protocol` directly (a d-4 change the architect will make). | ap-10 Done. |
| E-3 | `just check` runs clippy with `--all-features`; CI adds a macOS job beside Linux; the fakes check also runs with the `stub` feature graph it claims to cover. | ap-11 Done. |
| E-4 | Superseded in part by req-67 (decision): a repeated id is kept, not refused. The last line is in use and earlier lines are written back byte for byte; a `duplicate_id` warning (at most 20 per load) and `umx check` report it (blueprint d-16). Unknown graph.json keys are kept and warned (`unknown_key`); a newer format version is refused with `graph_format_newer`. | ap-12 Done. |
| E-5 | After a successful rename, a failed folder sync is a warning on the result, not an error; the port contract says which errors mean "nothing changed". | ap-13 Done. |
| E-6 | `evidence_kinds` is computed in core from `evidence`, or its reservation is dropped. | ap-14 Done. |
| E-7 | Rule staging is removed now that every rule is active, every crate a rule names must exist, the no_std check reads the lib path from cargo metadata and bans `#[path]` in pure crates, and every trait under `umx-core/src/ports` must have a `[[port]]` row. | ap-01, ap-02 Done. |
| E-8 | `main` requires the `check` status before merge. A repository setting, for the project owner or the project lead. | ap-03 |

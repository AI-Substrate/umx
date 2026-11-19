//! The bench ruler (workstream 003 s14, plan 153; graph-storage-expert's s14 storage rows spec). Every row times the
//! product: the services through umx-compose's real adapters in a private temp folder, or the exact public umx-graph
//! function the services call, named in the row's `path`. Nothing here is a copy of product code, except the two
//! rows marked `stand-in`: the old bench's own walk and path, kept beside the product's so the gap shows (ap-34).
//!
//! Rows are `real/<name>` on the realistic fixture, `keyed/<name>` on its keyed variant, `keyed100k/<name>` on its
//! all-keyed variant (every node keyed, 100k at the ruler's size; ws003 s19) and `rich/<name>` on the old
//! rich fixture. Each row is `{name, unit, n, median, p95, max, path, fixture, notes}`; later steps diff result files
//! by row name, so a row is never renamed once merged. Memory comes from the counting allocator, never RSS.

use crate::fixture::{self, Fixture};
use crate::real::{self, Queries, Real};
use crate::{Args, alloc, storage};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;
use umx_compose::{Config, Mode, compose};
use umx_core::value::SetValue;
use umx_core::{GraphMeta, Id, codec};
use umx_graph::{Graph, NodeIx};
use umx_services::{
    AlongRequest, BatchLine, BatchOp, BatchRequest, Ctx, CypherRequest, Direction, EdgeEvidence,
    EdgeListRequest, NEAR_DUPLICATE, NEAR_DUPLICATE_OTHER_TYPE, NodeAddRequest, NodeEditRequest,
    NodeListRequest, PathRequest, Services, WalkRequest,
};

/// The type the near-duplicate row's new titles are added as (the write rows add `problem` too).
const NEW_KIND: &str = "problem";
/// The CLI's defaults (umx-daemon verbs.rs): `node find` shows 10, list verbs 100, `path` goes 6 deep.
const FIND_LIMIT: usize = 10;
const LIST_LIMIT: usize = 100;
const PATH_DEPTH: usize = 6;
/// `node list`'s default columns (umx-daemon verbs.rs NODE_COLUMNS), which the node list rows read per row.
const LIST_COLUMNS: [&str; 5] = ["id", "type", "title", "status", "updated"];
/// Batch sizes from the spec: D1's 1,000 edits and the review's rollback of 300.
const EDITS: usize = 1000;
const ROLLBACK: usize = 300;
/// The bench's existing q5 (cypher-expert): reads only evidence_kinds, three times per node.
const Q5: &str = "MATCH (n) WHERE size(n.evidence_kinds) > 0 AND NOT 'client' IN n.evidence_kinds AND NOT \
                  'transcript' IN n.evidence_kinds RETURN n.id AS id ORDER BY id";

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

/// Six decimals: a microsecond stand-in row in ms must not round to zero.
fn round6(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

/// One warm-up, then 9 runs when the warm-up took under 50 ms, else 5 (spec section 0).
fn runs_after(warm_ms: f64) -> usize {
    if warm_ms < 50.0 { 9 } else { 5 }
}

/// The result rows, in the order they are measured, and the rows left out because they had no samples.
#[derive(Default)]
struct Rows {
    rows: Vec<Value>,
    /// A row whose pick list is empty on a small fixture (no title past rank 1,000 gives no typo queries, say) has
    /// no samples and no median: it is named here and in the result's `skipped_rows`, not written as a row.
    skipped: Vec<String>,
}

impl Rows {
    fn push(&mut self, name: &str, unit: &str, samples: &[f64], path: &str, notes: Value) {
        if samples.is_empty() {
            eprintln!(
                "umx-bench ruler: {name} skipped, no samples (the fixture is too small for its picks)"
            );
            self.skipped.push(name.to_owned());
            return;
        }
        let fixture = name.split('/').next().unwrap_or("");
        let mut s = samples.to_vec();
        s.sort_by(f64::total_cmp);
        let at = |p: usize| s[(s.len() * p / 100).min(s.len() - 1)];
        eprintln!("umx-bench ruler: {name} = {:.3} {unit}", at(50));
        self.rows.push(json!({
            "name": name, "unit": unit, "n": s.len(),
            "median": round6(at(50)), "p95": round6(at(95)), "max": round6(s[s.len() - 1]),
            "path": path, "fixture": fixture, "notes": notes,
            "samples": samples.iter().map(|v| round6(*v)).collect::<Vec<_>>(),
        }));
    }

    fn median(&self, name: &str) -> f64 {
        self.rows
            .iter()
            .find(|r| r["name"] == name)
            .and_then(|r| r["median"].as_f64())
            .unwrap_or(f64::NAN)
    }
}

/// `f` once to warm up, then the runs it calls for; each sample in ms.
fn timed<T>(mut f: impl FnMut() -> T) -> Vec<f64> {
    let t = Instant::now();
    std::hint::black_box(f());
    let n = runs_after(ms(t));
    (0..n)
        .map(|_| {
            let t = Instant::now();
            std::hint::black_box(f());
            ms(t)
        })
        .collect()
}

/// Each item once to warm up, then each item timed once: one sample per item, in ms.
fn per_item<I, T>(items: &[I], mut f: impl FnMut(&I) -> T) -> Vec<f64> {
    for i in items {
        std::hint::black_box(f(i));
    }
    items
        .iter()
        .map(|i| {
            let t = Instant::now();
            std::hint::black_box(f(i));
            ms(t)
        })
        .collect()
}

/// A fixture written to a folder, with what the rows need about it.
struct Folder<'a> {
    prefix: &'static str,
    dir: PathBuf,
    fx: &'a Fixture,
    meta: GraphMeta,
}

impl Folder<'_> {
    fn path(&self) -> &str {
        self.dir.to_str().expect("the work folder is UTF-8")
    }

    fn row(&self, name: &str) -> String {
        format!("{}/{name}", self.prefix)
    }

    /// The nodes graph as the services' load builds it (`Graph::load_nodes_indexing_for_graph`, the ontology's text
    /// fields, the graph id kept aside), then its edges (`Graph::load_edges`).
    fn graph(&self) -> Graph {
        let (mut g, _) = Graph::load_nodes_indexing_for_graph(
            self.fx.nodes.clone(),
            self.meta.ontology.text_fields(),
            self.meta.id,
            self.meta.ontology.std_version().is_some(),
        )
        .expect("the fixture loads");
        g.load_edges(&self.fx.edges).expect("the edges load");
        g
    }
}

fn write_folder(dir: &Path, fx: &Fixture) -> Result<(), String> {
    std::fs::create_dir(dir).map_err(|e| format!("make {}: {e}", dir.display()))?;
    for (name, bytes) in [
        ("graph.json", &fx.graph),
        ("nodes.jsonl", &fx.nodes),
        ("edges.jsonl", &fx.edges),
    ] {
        std::fs::write(dir.join(name), bytes).map_err(|e| format!("write {name}: {e}"))?;
    }
    Ok(())
}

/// Composed services with a fresh home under `work` (the real adapters; never ~/.umx).
struct Composer<'a> {
    work: &'a Path,
    homes: usize,
}

impl Composer<'_> {
    fn services(&mut self) -> Services {
        self.homes += 1;
        let config = Config::at(self.work.join(format!("home{}", self.homes)), None)
            .roots([self.work.to_str().expect("the work folder is UTF-8")]);
        compose(Mode::Real, &config)
    }

    /// Services with `folder` loaded.
    fn loaded(&mut self, folder: &str) -> Result<Services, String> {
        let mut svc = self.services();
        svc.graph_load(&ctx(), folder, None)
            .map_err(|p| format!("load {folder}: {}", p.code))?;
        Ok(svc)
    }

    /// A fresh copy of `from` (copied outside any timer), loaded: each write run starts from the same files.
    fn fresh(&mut self, from: &Folder) -> Result<(Services, PathBuf), String> {
        self.homes += 1;
        let dir = self.work.join(format!("copy{}", self.homes));
        std::fs::create_dir(&dir).map_err(|e| format!("make {}: {e}", dir.display()))?;
        for name in ["graph.json", "nodes.jsonl", "edges.jsonl"] {
            std::fs::copy(from.dir.join(name), dir.join(name))
                .map_err(|e| format!("copy {name}: {e}"))?;
        }
        let svc = self.loaded(dir.to_str().expect("UTF-8"))?;
        Ok((svc, dir))
    }
}

fn ctx() -> Ctx {
    Ctx::new("umx-bench")
}

/// One measured span: ms, bytes allocated, and the allocator's high-water mark above the live bytes at its start.
struct Span {
    ms: f64,
    allocated: f64,
    peak_extra: f64,
}

fn span<T>(f: impl FnOnce() -> T) -> (Span, T) {
    let (live, before) = (alloc::live(), alloc::allocated());
    alloc::reset_peak();
    let t = Instant::now();
    let out = f();
    let took = ms(t);
    let s = Span {
        ms: took,
        allocated: (alloc::allocated() - before) as f64,
        peak_extra: alloc::peak().saturating_sub(live) as f64,
    };
    (s, out)
}

fn file_bytes(dir: &Path) -> Value {
    let size = |n: &str| std::fs::metadata(dir.join(n)).map(|m| m.len()).unwrap_or(0);
    json!({"nodes_jsonl": size("nodes.jsonl"), "edges_jsonl": size("edges.jsonl")})
}

/// A write row: each run on a fresh loaded copy (one warm-up run first), `op` timed with its memory. Returns the
/// spans and the files' bytes after the last run.
fn write_runs(
    c: &mut Composer,
    from: &Folder,
    op: impl FnMut(&mut Services, usize) -> Result<(), String>,
) -> Result<(Vec<Span>, Value), String> {
    write_runs_after(c, from, |_| Ok(()), op)
}

/// [`write_runs`], with `prep` run untimed on each fresh copy before `op`.
fn write_runs_after(
    c: &mut Composer,
    from: &Folder,
    mut prep: impl FnMut(&mut Services) -> Result<(), String>,
    mut op: impl FnMut(&mut Services, usize) -> Result<(), String>,
) -> Result<(Vec<Span>, Value), String> {
    let mut spans = Vec::new();
    let mut runs = 1;
    let mut written = Value::Null;
    let mut k = 0;
    while k <= runs {
        let (mut svc, dir) = c.fresh(from)?;
        prep(&mut svc)?;
        let (s, done) = span(|| op(&mut svc, k));
        done?;
        if k == 0 {
            runs = runs_after(s.ms);
        } else {
            spans.push(s);
        }
        written = file_bytes(&dir);
        drop(svc);
        let _ = std::fs::remove_dir_all(&dir);
        k += 1;
    }
    Ok((spans, written))
}

fn edit_line(line: usize, node: Id, title: String) -> BatchLine {
    BatchLine {
        line: line as u32,
        op: BatchOp::NodeEdit {
            node: Some(node.to_text()),
            node_ref: None,
            req: NodeEditRequest {
                title: Some(title),
                ..Default::default()
            },
        },
    }
}

/// A line that fails when applied: an edit of a node the graph does not hold.
fn failing_line(line: usize) -> BatchLine {
    edit_line(line, Id(1), "never applied".into())
}

fn batch(svc: &mut Services, lines: Vec<BatchLine>) -> Result<usize, Vec<String>> {
    svc.batch(
        &ctx(),
        BatchRequest {
            idempotency: None,
            raised_in: None,
            reason: None,
            lines,
            dry_run: false,
            echo_sentences: false,
            refs: Default::default(),
            refs_graph: None,
        },
    )
    .map(|d| d.value.rows.len())
    .map_err(|e| e.iter().map(|p| p.code.to_owned()).collect())
}

/// The files of `dir`, for the rollback rows' byte-identity checks.
fn files(dir: &Path) -> Vec<Vec<u8>> {
    ["graph.json", "nodes.jsonl", "edges.jsonl"]
        .iter()
        .map(|n| std::fs::read(dir.join(n)).unwrap_or_default())
        .collect()
}

/// Load split (B1, B5) and heap: services load end to end, then the umx-graph functions it calls, on `f`.
fn load_rows(rows: &mut Rows, c: &mut Composer, f: &Folder) -> Result<(), String> {
    let mut totals = Vec::new();
    let mut peaks = Vec::new();
    let mut runs = 1;
    let mut k = 0;
    while k <= runs {
        let mut svc = c.services();
        let (s, done) = span(|| svc.graph_load(&ctx(), f.path(), None));
        done.map_err(|p| format!("load: {}", p.code))?;
        if k == 0 {
            runs = runs_after(s.ms);
        } else {
            totals.push(s.ms);
            peaks.push(s.peak_extra);
        }
        k += 1;
    }
    rows.push(
        &f.row("load_total_ms"),
        "ms",
        &totals,
        "Services::graph_load (fs adapter, registry write)",
        Value::Null,
    );
    rows.push(
        &f.row("load_peak_extra_bytes"),
        "bytes",
        &peaks,
        "Services::graph_load",
        json!("allocator high-water mark above live bytes at start"),
    );
    let (mut with_index, mut words) = (Vec::new(), Vec::new());
    for k in 0..=runs {
        let bytes = f.fx.nodes.clone();
        let t = Instant::now();
        let (mut g, _) = Graph::load_nodes_indexing_for_graph(
            bytes,
            f.meta.ontology.text_fields(),
            f.meta.id,
            f.meta.ontology.std_version().is_some(),
        )
        .map_err(|e| format!("load nodes: {e:?}"))?;
        let all = ms(t);
        let t = Instant::now();
        g.rebuild_words();
        let w = ms(t);
        if k > 0 {
            with_index.push(all);
            words.push(w);
        }
    }
    rows.push(
        &f.row("load_nodes_with_index_ms"),
        "ms",
        &with_index,
        "Graph::load_nodes_indexing_for_graph",
        json!("node bytes cloned outside the timer"),
    );
    rows.push(
        &f.row("word_index_build_ms"),
        "ms",
        &words,
        "Graph::rebuild_words",
        json!("proxy, upper bound (includes a second derive): rebuild_words runs index_row per row, which derives each line again before indexing it"),
    );
    let mut edges = Vec::new();
    for k in 0..=runs {
        let (mut g, _) = Graph::load_nodes_indexing_for_graph(
            f.fx.nodes.clone(),
            f.meta.ontology.text_fields(),
            f.meta.id,
            f.meta.ontology.std_version().is_some(),
        )
        .map_err(|e| format!("load nodes: {e:?}"))?;
        let t = Instant::now();
        g.load_edges(&f.fx.edges)
            .map_err(|e| format!("load edges: {e:?}"))?;
        if k > 0 {
            edges.push(ms(t));
        }
    }
    rows.push(
        &f.row("load_edges_ms"),
        "ms",
        &edges,
        "Graph::load_edges",
        json!("onto a freshly loaded nodes graph"),
    );
    let base = alloc::live();
    let g = f.graph();
    let live = alloc::live_since(base);
    let p = g.memory_bytes();
    rows.push(&f.row("graph_heap_bytes"), "bytes", &[p.total() as f64], "Graph::memory_bytes", json!({
        "allocator_live_bytes": live, "node_columns": p.node_columns, "edge_columns": p.edge_columns,
        "adjacency": p.adjacency, "field_columns": p.field_columns, "image": p.image, "cold": p.cold,
        "word_index": p.word_index, "external": p.external, "locators": p.locators, "entity_keys": p.entity_keys,
        "evidence_kinds": p.evidence_kinds, "evidence_kinds_forward": p.evidence_kinds_forward,
        "date_columns": p.date_columns,
    }));
    // s21: the word index's bytes per distinct term, and how many terms are title words (title find walks those).
    let (terms, title_terms) = g.word_terms();
    rows.push(
        &f.row("word_index_bytes_per_term"),
        "bytes",
        &[p.word_index as f64 / terms.max(1) as f64],
        "Graph::memory_bytes().word_index / Graph::word_terms().0",
        json!({"word_index": p.word_index, "terms": terms, "title_terms": title_terms}),
    );
    // s22: the evidence kinds column (review A4).
    rows.push(
        &f.row("evidence_kinds_bytes"),
        "bytes",
        &[p.evidence_kinds as f64],
        "Graph::memory_bytes().evidence_kinds",
        json!({"nodes": g.node_count()}),
    );
    // s29: the evidence kinds forward column, against graph-storage's bar of 2% of node_columns.
    rows.push(
        &f.row("evidence_kinds_forward_bytes"),
        "bytes",
        &[p.evidence_kinds_forward as f64],
        "Graph::memory_bytes().evidence_kinds_forward",
        json!({
            "nodes": g.node_count(), "layout": g.evidence_kinds_layout(),
            "share_of_node_columns_pct": round6(p.evidence_kinds_forward as f64 / p.node_columns as f64 * 100.0),
        }),
    );
    // D71 step 10: the typed time column, 16 bytes a date value.
    let mut dates = 0usize;
    g.each_date(real::DATE_FIELD, true, |_, _, _| dates += 1);
    rows.push(
        &f.row("date_columns_bytes"),
        "bytes",
        &[p.date_columns as f64],
        "Graph::memory_bytes().date_columns",
        json!({"nodes": g.node_count(), "dates": dates}),
    );
    Ok(())
}

/// Reads (A1 to A4), path and walk with their stand-ins, and find (C1, C2, C3, C5), through loaded services.
fn read_rows(
    rows: &mut Rows,
    svc: &Services,
    f: &Folder,
    g: &Graph,
    q: &Queries,
) -> Result<(), String> {
    let c = ctx();
    let edge_page = |offset| {
        svc.edge_list(
            &c,
            EdgeListRequest {
                limit: LIST_LIMIT,
                offset,
                ..Default::default()
            },
        )
        .expect("edge list")
    };
    // s16: each list row notes the bytes one run allocates, the decode and row building it avoids or pays.
    let allocated = |f: &dyn Fn()| json!({"allocated_bytes": span(f).0.allocated});
    rows.push(
        &f.row("edge_list_100_ms"),
        "ms",
        &timed(|| edge_page(0)),
        "Services::edge_list limit 100",
        allocated(&|| drop(edge_page(0))),
    );
    rows.push(
        &f.row("edge_list_100_offset150k_ms"),
        "ms",
        &timed(|| edge_page(150_000)),
        "Services::edge_list limit 100 offset 150000",
        allocated(&|| drop(edge_page(150_000))),
    );
    // s16: a node list row is the list plus `node list`'s five default columns read per row, so it times what the
    // daemon renders (review A3), not only the selection.
    let listed = |req: &NodeListRequest| {
        let page = svc.node_list(&c, req.clone()).expect("node list").value;
        page.rows
            .iter()
            .map(|n| LIST_COLUMNS.map(|col| n.field(col)))
            .collect::<Vec<_>>()
    };
    let all = NodeListRequest {
        limit: usize::MAX,
        ..Default::default()
    };
    let s = timed(|| listed(&all));
    rows.push(
        &f.row("node_list_all_ms"),
        "ms",
        &s,
        "Services::node_list no limit, then id, type, title, status, updated per row",
        allocated(&|| drop(listed(&all))),
    );
    let kind = NodeListRequest {
        kind: Some("problem".into()),
        ..Default::default()
    };
    let total = svc
        .node_list(&c, kind.clone())
        .map_err(|p| p.code.to_owned())?
        .value
        .total;
    let s = timed(|| listed(&kind));
    rows.push(
        &f.row("node_list_type_ms"),
        "ms",
        &s,
        "Services::node_list --type problem, limit 100, then the five default columns per row",
        json!({"matched": total, "allocated_bytes": span(|| listed(&kind)).0.allocated}),
    );
    let mut found = 0;
    let mut missed = Vec::new();
    let s = per_item(&q.pairs, |(a, b)| {
        let r = svc.path(
            &c,
            PathRequest {
                from: a.clone(),
                to: b.clone(),
                max_depth: PATH_DEPTH,
                relation: None,
                direction: Direction::Both,
            },
        );
        r.map(|p| p.value.rows.len()).map_err(|p| p.code)
    });
    for (a, b) in &q.pairs {
        match svc.path(
            &c,
            PathRequest {
                from: a.clone(),
                to: b.clone(),
                max_depth: PATH_DEPTH,
                relation: None,
                direction: Direction::Both,
            },
        ) {
            Ok(p) if !p.value.rows.is_empty() => found += 1,
            Ok(_) => missed.push("empty".to_owned()),
            Err(p) => missed.push(p.code.to_owned()),
        }
    }
    rows.push(
        &f.row("path_ms"),
        "ms",
        &s,
        "Services::path relation none (all stored relations), both directions, max depth 6",
        json!({"pairs": q.pairs.len(), "found": found, "not_found": missed}),
    );
    let s = per_item(&q.starts, |n| {
        svc.walk(
            &c,
            WalkRequest {
                no_tags: Default::default(),
                tags: Default::default(),
                node: n.clone(),
                depth: 3,
                relation: None,
                direction: Direction::Both,
                within: None,
                limit: usize::MAX,
                offset: 0,
            },
        )
        .expect("walk")
        .value
        .rows
        .len()
    });
    let reached: usize = q
        .starts
        .iter()
        .map(|n| {
            svc.walk(
                &c,
                WalkRequest {
                    no_tags: Default::default(),
                    tags: Default::default(),
                    node: n.clone(),
                    depth: 3,
                    relation: None,
                    direction: Direction::Both,
                    within: None,
                    limit: usize::MAX,
                    offset: 0,
                },
            )
            .map_or(0, |p| p.value.rows.len())
        })
        .sum();
    rows.push(
        &f.row("walk_d3_ms"),
        "ms",
        &s,
        "Services::walk depth 3, relation none, both directions, no limit",
        json!({"starts": q.starts.len(), "rows_avg": reached as f64 / q.starts.len() as f64}),
    );
    // What one path and one walk allocate, rows included (workstream 003 s17).
    let bytes: Vec<f64> = q
        .pairs
        .iter()
        .map(|(a, b)| {
            span(|| {
                svc.path(
                    &c,
                    PathRequest {
                        from: a.clone(),
                        to: b.clone(),
                        max_depth: PATH_DEPTH,
                        relation: None,
                        direction: Direction::Both,
                    },
                )
                .ok()
            })
            .0
            .allocated
        })
        .collect();
    rows.push(
        &f.row("path_allocated_bytes"),
        "bytes",
        &bytes,
        "Services::path as path_ms",
        json!("allocated per pair, the result included"),
    );
    let bytes: Vec<f64> = q
        .starts
        .iter()
        .map(|n| {
            span(|| {
                svc.walk(
                    &c,
                    WalkRequest {
                        no_tags: Default::default(),
                        tags: Default::default(),
                        node: n.clone(),
                        depth: 3,
                        relation: None,
                        direction: Direction::Both,
                        within: None,
                        limit: usize::MAX,
                        offset: 0,
                    },
                )
                .ok()
            })
            .0
            .allocated
        })
        .collect();
    rows.push(
        &f.row("walk_d3_allocated_bytes"),
        "bytes",
        &bytes,
        "Services::walk as walk_d3_ms",
        json!("allocated per start, the rows included"),
    );
    // The stand-ins: the old bench's own breadth-first walk and path (storage.rs) on the same graph and picks.
    let ix = |t: &String| -> NodeIx {
        g.node(t.parse::<Id>().expect("an id"))
            .expect("in the graph")
            .ix()
    };
    let mut scratch = storage::Scratch::new(g);
    let pairs: Vec<(NodeIx, NodeIx)> = q.pairs.iter().map(|(a, b)| (ix(a), ix(b))).collect();
    let starts: Vec<NodeIx> = q.starts.iter().map(ix).collect();
    let s = per_item(&pairs, |&(a, b)| storage::hops(g, &mut scratch, a, b));
    rows.push(
        &f.row("path_bench_copy_ms"),
        "ms",
        &s,
        "stand-in: umx-bench storage::hops",
        json!("the old bench row's own search, not the product; beside path_ms"),
    );
    let s = per_item(&starts, |&a| storage::walk(g, &mut scratch, a, 3));
    rows.push(
        &f.row("walk_d3_bench_copy_ms"),
        "ms",
        &s,
        "stand-in: umx-bench storage::walk",
        json!("the old bench row's own walk, not the product; beside walk_d3_ms"),
    );
    let q5 = CypherRequest {
        text: Q5.into(),
        limit: None,
        offset: None,
        params: Vec::new(),
        along: None,
    };
    let s = timed(|| svc.query(&c, &q5).expect("q5 runs"));
    rows.push(
        &f.row("evidence_kinds_q5_ms"),
        "ms",
        &s,
        "Services::query (q5, cypher-expert)",
        json!(Q5),
    );
    // Property reads (ws005 s14, cypher-expert): a hot standard read, a custom string read on every node, and w06;
    // and a date filter over a field of days, months and date-times (D71 step 10, step 12's baseline).
    let custom = format!(
        "MATCH (n) WHERE n.{} = 'x' RETURN n.id AS id",
        real::TEXT_FIELD
    );
    let dated = format!(
        "MATCH (n) WHERE n.{} < '2000-01-01' RETURN count(*) AS c",
        real::DATE_FIELD
    );
    for (name, text, what) in [
        (
            "query_hot_read_ms",
            "MATCH (n) RETURN n.title AS t",
            "a hot standard read",
        ),
        (
            "query_custom_read_ms",
            custom.as_str(),
            "a custom string read",
        ),
        (
            "query_w06_ms",
            "MATCH (n) WHERE 'analyst' IN n.evidence_kinds AND size(n.evidence_kinds) = 1 RETURN n.id AS id \
             ORDER BY id",
            "w06 (design set)",
        ),
        ("query_date_filter_ms", dated.as_str(), "a date filter"),
    ] {
        let req = CypherRequest {
            text: text.into(),
            limit: None,
            offset: None,
            params: Vec::new(),
            along: None,
        };
        let s = timed(|| svc.query(&c, &req).expect("the query runs"));
        rows.push(
            &f.row(name),
            "ms",
            &s,
            &format!("Services::query ({what}, ws005 s14)"),
            json!(text),
        );
    }
    for (name, kind) in [
        ("evidence_kind_rare_ms", real::RARE_KIND),
        ("evidence_kind_common_ms", real::COMMON_KIND),
    ] {
        let req = NodeListRequest {
            wheres: vec![format!("evidence_kinds={kind}")],
            ..Default::default()
        };
        let matched = svc
            .node_list(&c, req.clone())
            .map_err(|p| p.code.to_owned())?
            .value
            .total;
        let s = timed(|| svc.node_list(&c, req.clone()).expect("node list"));
        rows.push(
            &f.row(name),
            "ms",
            &s,
            &format!("Services::node_list --where evidence_kinds={kind}, limit 100"),
            json!({"matched": matched}),
        );
    }
    for (name, set) in [
        ("find_rare", &q.rare),
        ("find_common", &q.common),
        ("find_typo", &q.typo),
        ("find_wide", &q.wide),
    ] {
        let s = per_item(set, |t| {
            svc.node_find(&c, t, None, None, FIND_LIMIT)
                .map(|p| p.value.rows.len())
                .ok()
        });
        let cands: usize = set.iter().map(|t| g.title_candidates(t).0).sum();
        let avg = cands as f64 / set.len().max(1) as f64;
        rows.push(
            &f.row(&format!("{name}_ms")),
            "ms",
            &s,
            "Services::node_find limit 10",
            json!({"queries": set.len(), "candidates_avg": avg}),
        );
        rows.push(
            &f.row(&format!("{name}_candidates_avg")),
            "candidates",
            &[avg],
            "Graph::title_candidates (bench)",
            Value::Null,
        );
    }
    let s = per_item(&q.near, |t| {
        g.near_duplicates(
            t,
            Some(NEW_KIND),
            NEAR_DUPLICATE,
            NEAR_DUPLICATE_OTHER_TYPE,
            3,
        )
        .len()
    });
    rows.push(
        &f.row("near_duplicate_ms"),
        "ms",
        &s,
        &format!(
            "Graph::near_duplicates(title, Some(\"{NEW_KIND}\"), {NEAR_DUPLICATE}, \
             {NEAR_DUPLICATE_OTHER_TYPE}, 3), as services node add passes"
        ),
        json!({"titles": q.near.len()}),
    );
    let scratch: Vec<f64> = q
        .common
        .iter()
        .map(|t| {
            span(|| svc.node_find(&c, t, None, None, FIND_LIMIT).ok())
                .0
                .allocated
        })
        .collect();
    rows.push(
        &f.row("find_scratch_bytes"),
        "bytes",
        &scratch,
        "Services::node_find (common set)",
        json!("allocated per query"),
    );
    Ok(())
}

/// `real/node_evidence_redact` (F197): `Services::node_evidence_redact` on a fresh copy of the realistic fixture, one
/// warm-up then nine runs, each on a different node's first pointer whose source no earlier run took (the fixture
/// draws sources from 20,000, so each is held by a few nodes and every run scrubs for real). Notes give the nodes and
/// pointers the timed runs scrubbed.
fn redact_rows(rows: &mut Rows, c: &mut Composer, f: &Folder) -> Result<(), String> {
    let (mut svc, _) = c.fresh(f)?;
    let records = codec::decode_nodes(&f.fx.nodes)
        .map_err(|e| format!("decode: {e:?}"))?
        .records;
    let mut seen = std::collections::BTreeSet::new();
    // The fixture's pointers keep `seq` beside the source, not under `selector`, so no pointer text names them. Each
    // picked node gets `<source>#L1` first, outside the timer: the redact matches by source, so it scrubs that
    // pointer and the fixture's pointers with the same source on every node that holds one.
    let picks: Vec<(String, String)> = records
        .iter()
        .filter_map(|n| {
            let source = n.evidence.first()?.get("source")?.as_str()?.to_owned();
            seen.insert(source.clone())
                .then(|| (n.id.to_text(), format!("{source}#L1")))
        })
        .take(10)
        .collect();
    for (id, pointer) in &picks {
        svc.node_evidence_add(&ctx(), id, pointer, None)
            .map_err(|p| format!("evidence add {id}: {}", p.code))?;
    }
    let (mut samples, mut scrubbed) = (Vec::new(), (0, 0));
    for (i, (id, pointer)) in picks.iter().enumerate() {
        let t = Instant::now();
        let done = svc
            .node_evidence_redact(&ctx(), id, pointer, Some("bench redact"))
            .map_err(|p| format!("redact {id}: {}", p.code))?;
        let took = ms(t);
        if i > 0 {
            let r = done.value.redacted.unwrap_or_default();
            samples.push(took);
            scrubbed = (scrubbed.0 + r.nodes, scrubbed.1 + r.pointers);
        }
    }
    rows.push(
        &f.row("node_evidence_redact"),
        "ms",
        &samples,
        "Services::node_evidence_redact (fs adapter)",
        json!({"nodes_scrubbed": scrubbed.0, "pointers_scrubbed": scrubbed.1}),
    );
    Ok(())
}

/// What [`evidence_rows`] runs on one fixture.
struct EvidenceSpec {
    /// The q5 row (the realistic fixture has it among its reads already).
    q5: bool,
    /// The kind of the `--where evidence_kinds=` row, when the fixture's reads do not have one already.
    eq: Option<&'static str>,
    /// The kind of the scanning `--where evidence_kinds!=` row.
    ne: &'static str,
    /// The kind each of the 1,000 edits adds to node `h`'s evidence.
    added: fn(usize) -> String,
}

/// ws003 s29: reads through each row's evidence kinds (q5, a `--where evidence_kinds!=` that scans every node), the
/// index-driven `=` filter, 1,000 in-memory edits that each add an evidence item (applied, prepared, rolled back),
/// and a node added with a kind that sorts before every other (in a mask, every rank moves up one), on `f`.
fn evidence_rows(
    rows: &mut Rows,
    svc: &Services,
    f: &Folder,
    g: &mut Graph,
    spec: &EvidenceSpec,
) -> Result<(), String> {
    let c = ctx();
    let layout = g.evidence_kinds_layout();
    if spec.q5 {
        let q5 = CypherRequest {
            text: Q5.into(),
            limit: None,
            offset: None,
            params: Vec::new(),
            along: None,
        };
        let s = timed(|| svc.query(&c, &q5).expect("q5 runs"));
        rows.push(
            &f.row("evidence_kinds_q5_ms"),
            "ms",
            &s,
            "Services::query (q5, cypher-expert)",
            json!({"query": Q5, "layout": layout}),
        );
    }
    let wheres = spec
        .eq
        .map(|k| ("evidence_kind_eq_ms", "=", k))
        .into_iter()
        .chain([("evidence_kinds_ne_ms", "!=", spec.ne)]);
    for (name, op, kind) in wheres {
        let req = NodeListRequest {
            wheres: vec![format!("evidence_kinds{op}{kind}")],
            ..Default::default()
        };
        let matched = svc
            .node_list(&c, req.clone())
            .map_err(|p| p.code.to_owned())?
            .value
            .total;
        let s = timed(|| svc.node_list(&c, req.clone()).expect("node list"));
        rows.push(
            &f.row(name),
            "ms",
            &s,
            &format!("Services::node_list --where evidence_kinds{op}{kind}, limit 100"),
            json!({"matched": matched, "layout": layout}),
        );
    }
    let ids: Vec<Id> = g.nodes().map(|n| n.id()).collect();
    let n = ids.len();
    let edited: Vec<umx_core::Node> = (0..EDITS.min(n))
        .map(|i| {
            let h = i * 97 % n;
            let mut node = g.node(ids[h]).expect("in the graph").to_node();
            node.evidence
                .push(json!({"source": "s29.md", "kind": (spec.added)(h)}));
            node
        })
        .collect();
    let image = g.encode_nodes();
    let (mut total, mut undo) = (Vec::new(), Vec::new());
    let mut runs = 1;
    let mut k = 0;
    while k <= runs {
        let t = Instant::now();
        let mut b = g.batch();
        for node in &edited {
            b.insert_node(node.clone()).expect("fits");
        }
        b.prepare().expect("prepares");
        let u = Instant::now();
        b.rollback();
        let (rolled, all) = (ms(u), ms(t));
        if k == 0 {
            runs = runs_after(all);
        } else {
            total.push(all);
            undo.push(rolled);
        }
        k += 1;
    }
    assert!(
        g.encode_nodes() == image,
        "the rolled-back batch must leave encode_nodes byte-identical"
    );
    rows.push(
        &f.row("evidence_edit_1000_mem_ms"),
        "ms",
        &total,
        "umx_graph::Batch insert_node x1000 (one evidence item added to each), prepare, rollback",
        json!({"rollback_ms_median": round6(median(undo)), "layout": layout}),
    );
    // A node after the last with one evidence kind: one that sorts first (new), and one the graph holds (control).
    let mut add = |kind: &str| {
        let mut node = g.node(ids[0]).expect("in the graph").to_node();
        node.id = Id(ids[n - 1].0 + 1);
        node.evidence = vec![json!({"source": "s29.md", "kind": kind})];
        let mut s = Vec::new();
        for k in 0..=runs_after(1.0) {
            let t = Instant::now();
            g.insert_node(node.clone()).expect("fits");
            let took = ms(t);
            g.remove_node(node.id).expect("removes");
            if k > 0 {
                s.push(took);
            }
        }
        s
    };
    let control = add(real::RARE_KIND);
    let first = add("!first");
    rows.push(
        &f.row("evidence_kind_rank_shift_ms"),
        "ms",
        &first,
        "Graph::insert_node, a node after the last whose one evidence kind is new and sorts first",
        json!({"control_ms_median": round6(median(control)), "control": "the same node with a kind the graph holds", "layout": layout}),
    );
    Ok(())
}

/// Writes (D1, D2, B3, s13) on the realistic fixture.
fn write_rows(
    rows: &mut Rows,
    c: &mut Composer,
    f: &Folder,
    r: &Real,
    g: &mut Graph,
) -> Result<(), String> {
    let n = r.ids.len();
    let picks: Vec<usize> = (0..EDITS.min(n)).map(|i| i * 97 % n).collect();
    let lines = |count: usize| -> Vec<BatchLine> {
        picks[..count]
            .iter()
            .enumerate()
            .map(|(i, &h)| edit_line(i + 1, r.ids[h], format!("{} revised", r.titles[h])))
            .collect()
    };
    let (spans, written) = write_runs(c, f, |svc, _| {
        let done = batch(svc, lines(picks.len())).map_err(|e| format!("batch edit: {e:?}"))?;
        assert_eq!(done, picks.len());
        Ok(())
    })?;
    rows.push(
        &f.row("batch_edit_1000_ms"),
        "ms",
        &spans.iter().map(|s| s.ms).collect::<Vec<_>>(),
        "Services::batch, 1000 node edit lines (handles i*97 mod n), committed",
        json!({"files_after": written}),
    );
    // The rollback: one loaded copy for every run, since each must leave it unchanged (asserted every run).
    let (mut svc, dir) = c.fresh(f)?;
    let before_files = files(&dir);
    let all = NodeListRequest {
        limit: usize::MAX,
        ..Default::default()
    };
    let before_nodes = svc
        .node_list(&ctx(), all.clone())
        .map_err(|p| p.code.to_owned())?
        .value
        .rows
        .into_iter()
        .map(|n| n.to_node())
        .collect::<Vec<_>>();
    let mut rb = Vec::new();
    for k in 0..=runs_after(1e9) {
        let mut l = lines(ROLLBACK.min(picks.len()));
        l.push(failing_line(l.len() + 1));
        let t = Instant::now();
        let refused = batch(&mut svc, l);
        let took = ms(t);
        assert!(refused.is_err(), "line 301 must fail");
        assert!(
            files(&dir) == before_files,
            "a refused batch must leave the files byte-identical"
        );
        if k > 0 {
            rb.push(took);
        }
    }
    let after_nodes = svc
        .node_list(&ctx(), all)
        .map_err(|p| p.code.to_owned())?
        .value
        .rows
        .into_iter()
        .map(|n| n.to_node())
        .collect::<Vec<_>>();
    assert!(
        after_nodes == before_nodes,
        "a refused batch must leave every node as it was"
    );
    drop(svc);
    let _ = std::fs::remove_dir_all(&dir);
    rows.push(
        &f.row("batch_rollback_300_ms"),
        "ms",
        &rb,
        "Services::batch, 300 node edit lines then a failing line 301, timed to the refusal",
        json!("asserted: files byte-identical after every run, every node equal after all runs"),
    );
    // In memory: the same 1,000 edits through umx-graph's batch scope, prepared and rolled back.
    let edited: Vec<umx_core::Node> = picks
        .iter()
        .map(|&h| {
            let mut node = g.node(r.ids[h]).expect("in the graph").to_node();
            node.title.push_str(" revised");
            node
        })
        .collect();
    let image = g.encode_nodes();
    // The span the spec names (edit, prepare, rollback), with its apply-and-prepare and rollback parts in the notes.
    let (mut total, mut apply, mut undo) = (Vec::new(), Vec::new(), Vec::new());
    let mut runs = 1;
    let mut k = 0;
    while k <= runs {
        let t = Instant::now();
        let mut b = g.batch();
        for node in &edited {
            b.insert_node(node.clone()).expect("fits");
        }
        b.prepare().expect("prepares");
        let applied = ms(t);
        let u = Instant::now();
        b.rollback();
        let (rolled, all) = (ms(u), ms(t));
        if k == 0 {
            runs = runs_after(all);
        } else {
            total.push(all);
            apply.push(applied);
            undo.push(rolled);
        }
        k += 1;
    }
    assert!(
        g.encode_nodes() == image,
        "the rolled-back batch must leave encode_nodes byte-identical"
    );
    rows.push(&f.row("batch_edit_1000_mem_ms"), "ms", &total, "umx_graph::Batch insert_node x1000, prepare, rollback", json!({
        "disk": "none; encode_nodes asserted byte-identical after",
        "apply_and_prepare_ms_median": round6(median(apply)), "rollback_ms_median": round6(median(undo)),
    }));
    let (spans, written) = write_runs(c, f, |svc, k| {
        svc.node_add(
            &ctx(),
            NodeAddRequest {
                kind: "problem".into(),
                title: r.titles[(k * 7919) % n]
                    .split(' ')
                    .rev()
                    .collect::<Vec<_>>()
                    .join(" ")
                    + " added",
                ..Default::default()
            },
        )
        .map(drop)
        .map_err(|p| format!("node add: {}", p.code))
    })?;
    rows.push(
        &f.row("node_add_ms"),
        "ms",
        &spans.iter().map(|s| s.ms).collect::<Vec<_>>(),
        "Services::node_add, committed",
        json!({"files_after": written}),
    );
    rows.push(
        &f.row("node_add_allocated_bytes"),
        "bytes",
        &spans.iter().map(|s| s.allocated).collect::<Vec<_>>(),
        "Services::node_add",
        json!("bytes allocated over one add"),
    );
    rows.push(
        &f.row("node_add_peak_extra_bytes"),
        "bytes",
        &spans.iter().map(|s| s.peak_extra).collect::<Vec<_>>(),
        "Services::node_add",
        json!("allocator high-water mark above live bytes at start"),
    );
    let mid = r.ids[n / 2].to_text();
    let (spans, written) = write_runs(c, f, |svc, _| {
        let done = svc
            .node_edit(
                &ctx(),
                &mid,
                NodeEditRequest {
                    sets: vec![(real::NUMBER_FIELD.into(), SetValue::Raw("1000".into()))],
                    ..Default::default()
                },
            )
            .map_err(|p| format!("node edit: {}", p.code))?;
        assert!(
            !done.value.changed.is_empty(),
            "the edit must change the node"
        );
        Ok(())
    })?;
    rows.push(
        &f.row("node_edit_ms"),
        "ms",
        &spans.iter().map(|s| s.ms).collect::<Vec<_>>(),
        "Services::node_edit --set weight=1000, mid-file node, committed",
        json!({"files_after": written}),
    );
    let ext = format!(
        "umx:{}/{}",
        real::external_graph().to_text(),
        real::external_node(5000).to_text()
    );
    let leaves = &r.leaves;
    let cases = [
        (
            "edge_add_ms",
            r.ids[n / 3].to_text(),
            "supports",
            r.ids[2 * n / 3].to_text(),
            "Services::edge_add supports between two local nodes, committed",
        ),
        (
            "edge_add_new_parent_ms",
            leaves[leaves.len() / 2].to_text(),
            "part_of",
            leaves[leaves.len() / 3].to_text(),
            "Services::edge_add part_of to a node with no children, committed",
        ),
        (
            "edge_add_external_ms",
            r.ids[n / 5].to_text(),
            "supports",
            ext,
            "Services::edge_add to an external node (unloaded graph), committed",
        ),
    ];
    for (name, from, rel, to, path) in cases {
        let (spans, written) = write_runs(c, f, |svc, _| {
            let done = svc
                .edge_add(&ctx(), &from, rel, &to, &[])
                .map_err(|p| format!("{name}: {}", p.code))?;
            if done.value.existed {
                return Err(already_linked(name));
            }
            Ok(())
        })?;
        rows.push(&f.row(name), "ms", &spans.iter().map(|s| s.ms).collect::<Vec<_>>(), path, json!({"files_after": written, "peak_extra_bytes_median": median(spans.iter().map(|s| s.peak_extra).collect())}));
        if name == "edge_add_ms" {
            edge_add_memory(rows, f, "edge_add", path, &spans);
        }
    }
    edge_write_split(rows, c, f, r, g)?;
    let s = timed(|| g.rebuild_adjacency());
    rows.push(
        &f.row("rebuild_adjacency_ms"),
        "ms",
        &s,
        "Graph::rebuild_adjacency (bench)",
        Value::Null,
    );
    Ok(())
}

fn edge_add_memory(rows: &mut Rows, f: &Folder, prefix: &str, path: &str, spans: &[Span]) {
    rows.push(
        &f.row(&format!("{prefix}_allocated_bytes")),
        "bytes",
        &spans.iter().map(|s| s.allocated).collect::<Vec<_>>(),
        path,
        json!("bytes allocated over one add"),
    );
    rows.push(
        &f.row(&format!("{prefix}_peak_extra_bytes")),
        "bytes",
        &spans.iter().map(|s| s.peak_extra).collect::<Vec<_>>(),
        path,
        json!("allocator high-water mark above live bytes at start"),
    );
}

/// Where one edge write's time goes (ws003 s25 phase 0): the edge encode, the encode with its external scan, the
/// bare checked write of edges.jsonl, and an edge add whose session already exists (so the batch writes no node).
fn edge_write_split(
    rows: &mut Rows,
    c: &mut Composer,
    f: &Folder,
    r: &Real,
    g: &Graph,
) -> Result<(), String> {
    let s = timed(|| g.encode_edges());
    rows.push(
        &f.row("encode_edges_ms"),
        "ms",
        &s,
        "Graph::encode_edges",
        json!({"bytes": g.encode_edges().len()}),
    );
    let s = timed(|| umx_services::bench_encode_full_edges(&f.meta, g).expect("the edges encode"));
    rows.push(
        &f.row("encode_full_edges_ms"),
        "ms",
        &s,
        "services encode_full_edges (bench hook): encode_edges, then graph.json's external list and format",
        Value::Null,
    );
    let s = timed(|| g.external_end_graphs().count());
    rows.push(
        &f.row("external_ends_ms"),
        "ms",
        &s,
        "Graph::external_end_graphs: the column walk that replaces the _graph scan in services' load and edge writes",
        json!({"external_ends": g.external_end_graphs().count()}),
    );
    let (spans, written) = write_runs(c, f, |svc, _| {
        let alias = svc.aliases().next().expect("one graph loaded").to_string();
        svc.bench_write_edges(&alias, &f.fx.edges)
            .map_err(|p| format!("write edges: {}", p.code))
    })?;
    rows.push(
        &f.row("write_edges_file_ms"),
        "ms",
        &spans.iter().map(|s| s.ms).collect::<Vec<_>>(),
        "GraphFiles::write_set_checked of edges.jsonl alone, no encode (services bench hook, fs adapter)",
        json!({"files_after": written}),
    );
    let n = r.ids.len();
    let (from, to) = (r.ids[n / 3].to_text(), r.ids[2 * n / 3].to_text());
    let (first_from, first_to) = (r.ids[n / 4].to_text(), r.ids[3 * n / 4].to_text());
    let path = "Services::edge_add supports between two local nodes, committed, after one untimed edge add made \
                the session: the set writes edges.jsonl only";
    let (spans, written) = write_runs_after(
        c,
        f,
        |svc| {
            svc.edge_add(&ctx(), &first_from, "supports", &first_to, &[])
                .map(drop)
                .map_err(|p| format!("first edge add: {}", p.code))
        },
        |svc, _| {
            let done = svc
                .edge_add(&ctx(), &from, "supports", &to, &[])
                .map_err(|p| format!("edge add: {}", p.code))?;
            if done.value.existed {
                return Err(already_linked("edge_add_session_known_ms"));
            }
            Ok(())
        },
    )?;
    rows.push(
        &f.row("edge_add_session_known_ms"),
        "ms",
        &spans.iter().map(|s| s.ms).collect::<Vec<_>>(),
        path,
        json!({"files_after": written}),
    );
    edge_add_memory(rows, f, "edge_add_session_known", path, &spans);
    Ok(())
}

/// An edge row's two picked nodes are already linked in the fixture, so its add would add nothing: on a small fixture
/// the picks crowd together. An error naming the row, not a panic.
fn already_linked(row: &str) -> String {
    format!(
        "{row}: the fixture already holds the edge this row adds; run with more --nodes or another --seed"
    )
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v.get(v.len() / 2).copied().unwrap_or(f64::NAN)
}

/// Keyed adds then a failing line, timed to the refusal on one loaded copy; files and the new keys asserted gone.
fn keyed_rollback(
    rows: &mut Rows,
    c: &mut Composer,
    f: &Folder,
    adds: usize,
    name: &str,
) -> Result<(), String> {
    let (mut svc, dir) = c.fresh(f)?;
    let before = files(&dir);
    let mut samples = Vec::new();
    for k in 0..=runs_after(1e9) {
        let mut lines: Vec<BatchLine> = (0..adds)
            .map(|i| BatchLine {
                line: i as u32 + 1,
                op: BatchOp::NodeAdd {
                    id: None,
                    created: None,
                    ref_name: None,
                    req: NodeAddRequest {
                        kind: "actor".into(),
                        title: format!("Keyed rollback {k} {i}"),
                        sets: vec![(
                            umx_core::entity::ENTITY_KEY.into(),
                            SetValue::Json(json!([format!("email:rollback-{k}-{i}@example.org")])),
                        )],
                        ..Default::default()
                    },
                    parent_ref: None,
                },
            })
            .collect();
        lines.push(failing_line(adds + 1));
        let t = Instant::now();
        let refused = batch(&mut svc, lines);
        let took = ms(t);
        assert!(refused.is_err(), "the last line must fail");
        assert!(
            files(&dir) == before,
            "a refused batch must leave the files byte-identical"
        );
        let left = svc
            .node_list(
                &ctx(),
                NodeListRequest {
                    wheres: vec![format!("entity_key=email:rollback-{k}-0@example.org")],
                    ..Default::default()
                },
            )
            .map_err(|p| p.code.to_owned())?
            .value
            .total;
        assert_eq!(
            left,
            Some(0),
            "a rolled-back keyed add must leave no node holding its key"
        );
        if k > 0 {
            samples.push(took);
        }
    }
    drop(svc);
    let _ = std::fs::remove_dir_all(&dir);
    rows.push(
        &f.row(name),
        "ms",
        &samples,
        &format!(
            "Services::batch, {adds} keyed node adds then a failing line, timed to the refusal"
        ),
        json!("asserted every run: files byte-identical, no node holds a rolled-back key"),
    );
    Ok(())
}

fn sha256(parts: &[&[u8]]) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn uptime() -> String {
    Command::new("uptime")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default()
}

fn meta_of(fx: &Fixture) -> GraphMeta {
    codec::decode_graph(&fx.graph).expect("the fixture's graph.json decodes")
}

pub fn run(a: &Args) -> Result<PathBuf, String> {
    let load_start = uptime();
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let bench_sha256 =
        sha256(&[&std::fs::read(&exe).map_err(|e| format!("read {}: {e}", exe.display()))?]);
    let work = crate::work_dir()?;
    let _cleanup = crate::RemoveOnDrop(work.clone());
    let mut c = Composer {
        work: &work,
        homes: 0,
    };
    let mut rows = Rows::default();
    let t = Instant::now();
    let r = real::generate(a.seed, a.nodes);
    if a.redact_only {
        let real_f = Folder {
            prefix: "real",
            dir: work.join("real"),
            fx: &r.fx,
            meta: r.meta.clone(),
        };
        write_folder(&real_f.dir, real_f.fx)?;
        redact_rows(&mut rows, &mut c, &real_f)?;
        return Ok(work.clone());
    }
    if a.along_only {
        let real_f = Folder {
            prefix: "real",
            dir: work.join("real"),
            fx: &r.fx,
            meta: r.meta.clone(),
        };
        write_folder(&real_f.dir, real_f.fx)?;
        along_rows(&mut rows, &mut c, &real_f)?;
        return Ok(work.clone());
    }
    if a.strength_only {
        let real_f = Folder {
            prefix: "real",
            dir: work.join("real"),
            fx: &r.fx,
            meta: r.meta.clone(),
        };
        strength_rows(&mut rows, &real_f, a.seed)?;
        return Ok(work.clone());
    }
    let keyed_fx = real::keyed(a.seed, &r, a.nodes / 10);
    let (keyed_all_fx, keyed_all_duplicates) = real::keyed_all(a.seed, &r);
    let generate_ms = ms(t);
    let q = real::queries(a.seed, &r);
    let rich_fx = fixture::generate(a.seed, a.nodes, 3, true);
    let real_f = Folder {
        prefix: "real",
        dir: work.join("real"),
        fx: &r.fx,
        meta: r.meta.clone(),
    };
    let keyed_f = Folder {
        prefix: "keyed",
        dir: work.join("keyed"),
        fx: &keyed_fx,
        meta: r.meta.clone(),
    };
    let keyed_all_f = Folder {
        prefix: "keyed100k",
        dir: work.join("keyed100k"),
        fx: &keyed_all_fx,
        meta: r.meta.clone(),
    };
    let rich_f = Folder {
        prefix: "rich",
        dir: work.join("rich"),
        fx: &rich_fx,
        meta: meta_of(&rich_fx),
    };
    let distinct_fx = real::distinct(&r);
    let distinct_f = Folder {
        prefix: "distinct",
        dir: work.join("distinct"),
        fx: &distinct_fx,
        meta: r.meta.clone(),
    };
    for f in [&real_f, &keyed_f, &keyed_all_f, &rich_f, &distinct_f] {
        write_folder(&f.dir, f.fx)?;
    }
    // Each fixture loads once through the services: its warnings are reported, and only the keyed ones' deliberate
    // duplicate keys are expected.
    let mut warnings = serde_json::Map::new();
    for f in [&real_f, &keyed_f, &keyed_all_f, &rich_f, &distinct_f] {
        let mut svc = c.services();
        let done = svc
            .graph_load(&ctx(), f.path(), None)
            .map_err(|p| format!("load {}: {}", f.prefix, p.code))?;
        let mut codes = std::collections::BTreeMap::<&str, usize>::new();
        for p in &done.notes {
            *codes.entry(p.code).or_default() += 1;
        }
        warnings.insert(f.prefix.into(), json!(codes));
    }
    eprintln!(
        "umx-bench ruler: fixtures written in {generate_ms:.0} ms; load warnings {}",
        Value::Object(warnings.clone())
    );

    load_rows(&mut rows, &mut c, &real_f)?;
    load_rows(&mut rows, &mut c, &keyed_f)?;
    rows.push(
        "keyed/minus_real_load_nodes_ms_derived",
        "ms",
        &[rows.median("keyed/load_nodes_with_index_ms")
            - rows.median("real/load_nodes_with_index_ms")],
        "derived",
        json!("derived: keyed minus real load_nodes_with_index medians"),
    );
    let index_bytes = |rows: &mut Rows, f: &Folder| {
        let p = f.graph().memory_bytes();
        rows.push(
            &f.row("unique_index_bytes"),
            "bytes",
            &[(p.locators + p.entity_keys) as f64],
            "Graph::memory_bytes locators + entity_keys",
            json!({"locators": p.locators, "entity_keys": p.entity_keys}),
        );
    };
    index_bytes(&mut rows, &keyed_f);
    keyed_rollback(
        &mut rows,
        &mut c,
        &keyed_f,
        1,
        "batch_add_keyed_rollback_ms",
    )?;
    keyed_rollback(
        &mut rows,
        &mut c,
        &keyed_f,
        1000,
        "batch_add_keyed_rollback_1000_ms",
    )?;
    load_rows(&mut rows, &mut c, &keyed_all_f)?;
    index_bytes(&mut rows, &keyed_all_f);
    load_rows(&mut rows, &mut c, &rich_f)?;
    {
        let svc = c.loaded(real_f.path())?;
        let g = real_f.graph();
        read_rows(&mut rows, &svc, &real_f, &g, &q)?;
        spine_rows(&mut rows, &svc, &real_f)?;
    }
    along_rows(&mut rows, &mut c, &real_f)?;
    strength_rows(&mut rows, &real_f, a.seed)?;
    inbound_rows(&mut rows, &mut c, &real_f, &r, a.seed)?;
    {
        let svc = c.loaded(rich_f.path())?;
        let g = rich_f.graph();
        let rich_ids: Vec<Id> = g.nodes().map(|n| n.id()).collect();
        let rich_titles: Vec<String> = g.nodes().map(|n| n.title().to_owned()).collect();
        let rich_q = real::rich_picks(a.seed, &rich_ids, &rich_titles);
        read_rows(&mut rows, &svc, &rich_f, &g, &rich_q)?;
    }
    {
        let mut g = real_f.graph();
        write_rows(&mut rows, &mut c, &real_f, &r, &mut g)?;
        redact_rows(&mut rows, &mut c, &real_f)?;
    }
    // ws003 s29: the evidence kinds reads and edits on the realistic fixture, then its one-kind-per-node variant.
    {
        let svc = c.loaded(real_f.path())?;
        let mut g = real_f.graph();
        let spec = EvidenceSpec {
            q5: false,
            eq: None,
            ne: real::COMMON_KIND,
            added: |_| real::RARE_KIND.into(),
        };
        evidence_rows(&mut rows, &svc, &real_f, &mut g, &spec)?;
    }
    load_rows(&mut rows, &mut c, &distinct_f)?;
    {
        let svc = c.loaded(distinct_f.path())?;
        let mut g = distinct_f.graph();
        let spec = EvidenceSpec {
            q5: true,
            eq: Some(real::DISTINCT_KIND),
            ne: real::DISTINCT_KIND,
            added: |h| format!("k{h}-b"),
        };
        evidence_rows(&mut rows, &svc, &distinct_f, &mut g, &spec)?;
    }
    let fixture_of = |f: &Folder| {
        json!({
            "nodes": f.fx.node_count, "edges": f.fx.edge_count,
            "nodes_jsonl_bytes": f.fx.nodes.len(), "edges_jsonl_bytes": f.fx.edges.len(),
            "sha256": sha256(&[&f.fx.nodes, &f.fx.edges]),
        })
    };
    let mut real_fixture = fixture_of(&real_f);
    real_fixture["vocabulary"] = json!(r.vocab.len());
    real_fixture["distinct_words_used"] = json!(r.distinct_words);
    real_fixture["external_edges"] = json!(r.external_edges);
    real_fixture["evidence_kind_nodes"] = json!(r.kind_nodes);
    real_fixture["no_evidence_nodes"] = json!(r.no_evidence);
    real_fixture["top_degree"] = json!(r.top_degree);
    real_fixture["top_degree_share_of_edges"] = json!(round6(
        r.top_degree as f64 / r.fx.edge_count.max(1) as f64 * 100.0
    ));
    let mut keyed_fixture = fixture_of(&keyed_f);
    keyed_fixture["keyed_nodes"] = json!(a.nodes / 10);
    keyed_fixture["duplicate_keys"] = json!(real::DUPLICATE_KEYS);
    let mut keyed_all_fixture = fixture_of(&keyed_all_f);
    keyed_all_fixture["keyed_nodes"] = json!(keyed_all_fx.node_count);
    keyed_all_fixture["duplicate_keys"] = json!(keyed_all_duplicates);
    let date = umx_core::time::iso_from_ms(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis() as u64,
    );
    let result = json!({
        "umx_bench_ruler": 1,
        "date": date,
        "commit": a.commit,
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "conditions": a.conditions,
        "machine": crate::report::machine(),
        "load_average_start": load_start,
        "load_average_end": uptime(),
        "bench_sha256": bench_sha256,
        "seed": a.seed,
        "generate_ms": round6(generate_ms),
        "load_warnings": warnings,
        "fixture": {
            "real": real_fixture, "keyed": keyed_fixture, "keyed100k": keyed_all_fixture,
            "rich": fixture_of(&rich_f), "distinct": fixture_of(&distinct_f),
        },
        "rows": rows.rows,
        "skipped_rows": rows.skipped,
    });
    write(&a.out, &result, &q)
}

/// `umx graph spine propose` (ws005 s07): the whole proposal under the services read lock, its time and the bytes
/// it allocates (graph-storage-expert's cost terms).
/// The nodes the strength rows search: PRD 002 search-10's cost is measured at 20k.
const STRENGTH_NODES: usize = 20_000;

/// `real/search_vector_leg_ms` and `real/search_strength_ms` (PRD 002 search-10): one phrasing's vector leg over the
/// first [`STRENGTH_NODES`] nodes of the realistic fixture with one seeded whole-node vector each (256 dims), without
/// and with strength: `umx_services::vector_candidates`, then `vector_candidates_scored` (the same scan filling each
/// node's cosine), `Population::of` (two order-statistic selections) and `similarity::strength` for every candidate,
/// as search finish does. The cuts are embeddinggemma-2@256+p1's (their values do not change the cost).
fn strength_rows(rows: &mut Rows, f: &Folder, seed: u64) -> Result<(), String> {
    use umx_core::similarity::{Population, strength, strength_cuts};
    let key = umx_core::ports::ModelKey::new("embeddinggemma-2", 256)
        .map_err(|e| format!("{e:?}"))?
        .with_prompt_version(1);
    let cuts = strength_cuts(&key).ok_or("embeddinggemma-2@256+p1 has no strength cuts")?;
    let mut g = f.graph();
    let ids: Vec<Id> = g.nodes().map(|n| n.id()).take(STRENGTH_NODES).collect();
    let files = crate::vectors::shards(&ids, 1, seed);
    let held = crate::vectors::load(&mut g, &files);
    let space = crate::vectors::SPACE;
    let query = g
        .vector(space, g.node_ix(ids[ids.len() / 2]).unwrap(), None)
        .ok_or("no query vector")?
        .to_vec();
    let path = "umx_services::vector_candidates";
    let notes = json!({ "nodes_with_vectors": held, "dims": 256, "floor": 0.0 });
    let without = timed(|| umx_services::vector_candidates(&g, space, &query, None, 0.0).len());
    rows.push(
        &format!("{}/search_vector_leg_ms", f.prefix),
        "ms",
        &without,
        path,
        notes.clone(),
    );
    let mut whole = Vec::new();
    let with = timed(|| {
        let hits =
            umx_services::vector_candidates_scored(&g, space, &query, None, 0.0, Some(&mut whole));
        let mut scores: Vec<f32> = whole.iter().copied().filter(|s| !s.is_nan()).collect();
        let pop = Population::of(&mut scores).expect("scores");
        hits.iter()
            .filter(|(id, _, _)| {
                let s = whole[g.node_ix(*id).unwrap().index()];
                strength(&pop, &cuts, s).is_some()
            })
            .count()
    });
    rows.push(
        &format!("{}/search_strength_ms", f.prefix),
        "ms",
        &with,
        "umx_services::vector_candidates_scored + umx_core::similarity::{Population::of, strength}",
        notes,
    );
    Ok(())
}

fn spine_rows(rows: &mut Rows, svc: &Services, f: &Folder) -> Result<(), String> {
    let c = ctx();
    let alias = f.meta.alias.as_str();
    let propose = || svc.graph_spine_propose(&c, alias, None, None, 100, None);
    let p = propose().map_err(|p| p.code.to_owned())?.value;
    let offered = p.candidates.iter().filter(|c| c.offered).count();
    let notes = json!({
        "candidates": p.candidates.len(), "offered": offered, "in_the_way": p.in_the_way.len(),
        "best": p.candidates.first().filter(|c| c.offered).map(|c| c.name.clone()),
    });
    let s = timed(|| propose().expect("spine propose"));
    rows.push(
        &f.row("spine_propose_ms"),
        "ms",
        &s,
        "Services::graph_spine_propose",
        notes,
    );
    let bytes = span(|| propose().ok()).0;
    rows.push(
        &f.row("spine_propose_bytes"),
        "bytes",
        &[bytes.allocated],
        "Services::graph_spine_propose",
        json!({"peak_extra": bytes.peak_extra}),
    );
    Ok(())
}

/// Anchors added to a fresh copy for the `--along` rows, and every how-many-th node raised in one of them.
const ALONG_ANCHORS: usize = 40;
const ALONG_EVERY: usize = 10;

/// `--along` on node list (D71 step 12, graph-storage's terms): a fresh copy of the fixture given 40 dated segments,
/// every tenth node raised in one, and the spine `day` over them (place names `evidence`, so the evidence pass runs:
/// the costlier read). Each filtered read runs one `Facts` read and one `place()` under the read lock, with no
/// cache; the row is that whole read, beside `node_list_all_ms` (the same list unfiltered). The built-in row reads
/// `recorded` (one column walk).
fn along_rows(rows: &mut Rows, c: &mut Composer, f: &Folder) -> Result<(), String> {
    let (mut svc, dir) = c.fresh(f)?;
    let ids: Vec<Id> = f.graph().nodes().map(|n| n.id()).collect();
    let mut lines = Vec::new();
    for s in 0..ALONG_ANCHORS {
        lines.push(BatchLine {
            line: lines.len() as u32 + 1,
            op: BatchOp::NodeAdd {
                id: None,
                created: None,
                ref_name: Some(format!("s{s}")),
                req: NodeAddRequest {
                    kind: "segment".into(),
                    title: format!("Segment {s}"),
                    sets: vec![
                        (
                            "starts_on".into(),
                            SetValue::Raw(format!("2026-11-{:02}T09:00:00Z", 1 + s % 28)),
                        ),
                        ("step".into(), SetValue::Raw(format!("{}", 1 + s))),
                    ],
                    ..Default::default()
                },
                parent_ref: None,
            },
        });
    }
    batch(&mut svc, lines).map_err(|e| format!("along anchors: {e:?}"))?;
    let segs: Vec<String> = svc
        .node_list(
            &ctx(),
            NodeListRequest {
                kind: Some("segment".into()),
                limit: ALONG_ANCHORS,
                ..Default::default()
            },
        )
        .map_err(|p| format!("along anchors: {}", p.code))?
        .value
        .rows
        .iter()
        .map(|n| n.id().to_text())
        .collect();
    let edges: Vec<BatchLine> = ids
        .iter()
        .step_by(ALONG_EVERY)
        .enumerate()
        .map(|(k, id)| BatchLine {
            line: k as u32 + 1,
            op: BatchOp::EdgeAdd {
                id: None,
                from: Some(id.to_text()),
                from_ref: None,
                relation: "raised_in".into(),
                to: Some(segs[k % segs.len()].clone()),
                to_ref: None,
                sets: Vec::new(),
                evidence: EdgeEvidence::default(),
            },
        })
        .collect();
    let linked = edges.len();
    for chunk in edges.chunks(umx_core::batch::MAX_BATCH_OPS) {
        batch(&mut svc, chunk.to_vec()).map_err(|e| format!("along links: {e:?}"))?;
    }
    let decl = json!({
        "name": "day", "title": "segment by starts_on", "main": true,
        "anchors": { "type": "segment", "inside": null },
        "order": ["starts_on", "step"], "place": ["raised_in", "evidence"],
    });
    svc.graph_spine_set(&ctx(), &f.meta.alias, &decl)
        .map_err(|p| format!("along spine: {}", p.code))?;
    // The same anchors placed by links only: no evidence pass (graph-storage's skip).
    let links = json!({
        "name": "day_links", "title": "segment by starts_on, links only", "main": false,
        "anchors": { "type": "segment", "inside": null },
        "order": ["starts_on", "step"], "place": ["raised_in"],
    });
    svc.graph_spine_set(&ctx(), &f.meta.alias, &links)
        .map_err(|p| format!("along spine: {}", p.code))?;
    let c = ctx();
    let listed = |spine: &str, after: Option<&str>| {
        let page = svc
            .node_list(
                &c,
                NodeListRequest {
                    limit: usize::MAX,
                    along: Some(AlongRequest {
                        spine: spine.into(),
                        after: after.map(str::to_owned),
                        before: None,
                    }),
                    ..Default::default()
                },
            )
            .expect("node list --along")
            .value;
        let along = page.along.expect("along");
        let rows: Vec<_> = page
            .rows
            .iter()
            .map(|n| LIST_COLUMNS.map(|col| n.field(col)))
            .collect();
        (rows.len(), along.not_placed, along.by_rule)
    };
    // The same list unfiltered, on this copy: the baseline graph-storage's terms compare against.
    let plain = || {
        let page = svc
            .node_list(
                &c,
                NodeListRequest {
                    limit: usize::MAX,
                    ..Default::default()
                },
            )
            .expect("node list")
            .value;
        let rows: Vec<_> = page
            .rows
            .iter()
            .map(|n| LIST_COLUMNS.map(|col| n.field(col)))
            .collect();
        rows.len()
    };
    let mut unfiltered = timed(plain);
    unfiltered.sort_by(f64::total_cmp);
    let unfiltered_ms = unfiltered[unfiltered.len() / 2];
    let unfiltered_bytes = span(plain).0.allocated;
    for (row, spine, after, path) in [
        (
            "along_declared_ms",
            "day",
            None,
            "Services::node_list --along day (Facts read with evidence, place())",
        ),
        (
            "along_declared_links_ms",
            "day_links",
            None,
            "Services::node_list --along day_links (Facts read with no evidence pass, place())",
        ),
        (
            "along_builtin_ms",
            "recorded",
            Some("2000"),
            "Services::node_list --along recorded --after 2000",
        ),
    ] {
        let (kept, not_placed, by_rule) = listed(spine, after);
        let s = timed(|| listed(spine, after));
        let bytes = span(|| listed(spine, after)).0;
        let notes = json!({
            "kept": kept, "not_placed": not_placed, "by_rule": by_rule, "anchors": ALONG_ANCHORS,
            "linked": linked, "allocated_bytes": bytes.allocated, "peak_extra_bytes": bytes.peak_extra,
            "unfiltered_median_ms": unfiltered_ms, "unfiltered_allocated_bytes": unfiltered_bytes,
        });
        eprintln!("umx-bench ruler: {} notes {notes}", f.row(row));
        rows.push(&f.row(row), "ms", &s, path, notes);
    }
    drop(svc);
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// The time bits of the inbound graph's ids: a day after the external graph's, so they meet no fixture id.
const INBOUND_MS: u128 = 1_794_182_400_000 + 2 * 86_400_000;

/// `edge list --from` on a node with many incoming external links (plan 196, review). A second loaded
/// graph, `inbound`, holds a tenth of the real fixture's node count, each with one `supports` edge to the same real
/// node (the hub). Edge list gathers every incident link, the inbound ones included, before it filters by `--from`;
/// each inbound link once cost a policy read before the clock hook.
fn inbound_rows(
    rows: &mut Rows,
    c: &mut Composer,
    f: &Folder,
    r: &Real,
    seed: u64,
) -> Result<(), String> {
    let count = (r.ids.len() / 10).max(1);
    let hub = r.ids[r.ids.len() / 2];
    let base = fixture::generate(seed, count, 0, false);
    let mut nodes = codec::decode_nodes(&base.nodes)
        .map_err(|e| format!("inbound nodes: {e}"))?
        .records;
    let inbound_id = |n: usize| Id((INBOUND_MS << 80) | n as u128);
    for (i, n) in nodes.iter_mut().enumerate() {
        n.id = inbound_id(i + 1);
    }
    let edges: Vec<umx_core::Edge> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| umx_core::Edge {
            id: inbound_id(count + 1 + i),
            kind: "supports".into(),
            from_graph: None,
            from: n.id,
            to_graph: Some(f.meta.id),
            to: hub,
            created: n.created.clone(),
            created_by: "umx-bench".into(),
            authored_by_session: None,
            edited_by_session: None,
            evidence: Vec::new(),
            fields: Default::default(),
        })
        .collect();
    let fx = Fixture {
        nodes: codec::encode_nodes(nodes.iter()),
        node_count: nodes.len(),
        edge_count: 0,
        ..base
    };
    let dir = crate::external::write_fixture(
        c.work,
        "inbound",
        inbound_id(0),
        &fx,
        &codec::encode_edges(edges.iter()),
    )?;
    let mut svc = c.loaded(f.path())?;
    svc.graph_load(&ctx(), dir.to_str().expect("UTF-8"), None)
        .map_err(|p| format!("load inbound: {}", p.code))?;
    let c = ctx().graph(f.meta.alias.as_str());
    let req = EdgeListRequest {
        from: Some(hub.to_text()),
        limit: LIST_LIMIT,
        ..Default::default()
    };
    let list = || svc.edge_list(&c, req.clone()).expect("edge list --from");
    let into = svc
        .edge_list(
            &c,
            EdgeListRequest {
                to: Some(hub.to_text()),
                limit: 1,
                ..Default::default()
            },
        )
        .map_err(|p| p.code.to_owned())?
        .value
        .total;
    let total = list().value.total;
    rows.push(
        &f.row("edge_list_from_inbound_hub_ms"),
        "ms",
        &timed(&list),
        "Services::edge_list --from a node with a tenth of the node count incoming external links, limit 100",
        json!({
            "incoming_external": count, "listed_to_hub": into, "total": total,
            "allocated_bytes": span(|| drop(list())).0.allocated,
        }),
    );
    Ok(())
}

/// Writes `<date>-<HHMMSS>-<commit>-ruler-<nodes>n.json`, its `.md` and `-queries.json`; returns the JSON's path.
fn write(out: &Path, r: &Value, q: &Queries) -> Result<PathBuf, String> {
    use std::io::Write;
    std::fs::create_dir_all(out).map_err(|e| format!("make {}: {e}", out.display()))?;
    let date = r["date"].as_str().unwrap_or("");
    let time: String = date
        .get(11..19)
        .unwrap_or("000000")
        .chars()
        .filter(char::is_ascii_digit)
        .collect();
    let base = format!(
        "{}-{time}-{}-ruler-{}n",
        date.get(..10).unwrap_or("unknown"),
        r["commit"].as_str().unwrap_or("unknown"),
        r["fixture"]["real"]["nodes"],
    );
    let mut bytes = serde_json::to_vec_pretty(r).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    let (stem, path) = (1..)
        .map(|n| {
            if n == 1 {
                base.clone()
            } else {
                format!("{base}-{n}")
            }
        })
        .find_map(|stem| {
            let path = out.join(format!("{stem}.json"));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut f) => Some(
                    f.write_all(&bytes)
                        .map(|()| (stem, path.clone()))
                        .map_err(|e| format!("write {}: {e}", path.display())),
                ),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(e) => Some(Err(format!("write {}: {e}", path.display()))),
            }
        })
        .expect("an unused name exists")?;
    let queries = json!({
        "rare": q.rare, "common": q.common, "typo": q.typo, "wide": q.wide, "near": q.near,
        "pairs": q.pairs, "starts": q.starts,
    });
    let mut qb = serde_json::to_vec_pretty(&queries).map_err(|e| e.to_string())?;
    qb.push(b'\n');
    std::fs::write(out.join(format!("{stem}-queries.json")), qb)
        .map_err(|e| format!("write queries: {e}"))?;
    std::fs::write(out.join(format!("{stem}.md")), markdown(r))
        .map_err(|e| format!("write {stem}.md: {e}"))?;
    Ok(path)
}

fn markdown(r: &Value) -> String {
    let s = |v: &Value| {
        v.as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| v.to_string())
    };
    let f = &r["fixture"];
    let mut md = format!(
        "# umx-bench ruler {}\n\nCommit `{}`, {} build, bench binary sha256 `{}`. Conditions: {}.\n\n\
Load average at start: `{}`. At end: `{}`.\n\nMachine: {} {} {}, {} cores.\n\n\
## Fixtures (seed {}, synthetic, no client data)\n\n\
| fixture | nodes | edges | nodes.jsonl bytes | edges.jsonl bytes | sha256 (nodes + edges) |\n|---|---:|---:|---:|---:|---|\n",
        s(&r["date"]),
        s(&r["commit"]),
        s(&r["profile"]),
        s(&r["bench_sha256"]),
        r["conditions"]
            .as_str()
            .filter(|c| !c.is_empty())
            .unwrap_or("not recorded"),
        s(&r["load_average_start"]),
        s(&r["load_average_end"]),
        s(&r["machine"]["cpu"]),
        s(&r["machine"]["os"]),
        s(&r["machine"]["arch"]),
        r["machine"]["cores"],
        r["seed"],
    );
    for k in ["real", "keyed", "keyed100k", "rich"] {
        let x = &f[k];
        md.push_str(&format!(
            "| {k} | {} | {} | {} | {} | `{}` |\n",
            x["nodes"],
            x["edges"],
            x["nodes_jsonl_bytes"],
            x["edges_jsonl_bytes"],
            s(&x["sha256"])
        ));
    }
    let real = &f["real"];
    md.push_str(&format!(
        "\nReal: {} vocabulary words, {} distinct words used, {} external edges, top degree {} ({}% of edges), \
evidence kind nodes {}, {} nodes without evidence. Keyed: {} keyed nodes, {} deliberate duplicate keys. \
Keyed100k: {} keyed nodes, {} deliberate duplicate keys. Load warnings: {}.\n\n",
        real["vocabulary"], real["distinct_words_used"], real["external_edges"], real["top_degree"],
        real["top_degree_share_of_edges"], real["evidence_kind_nodes"], real["no_evidence_nodes"],
        f["keyed"]["keyed_nodes"], f["keyed"]["duplicate_keys"], f["keyed100k"]["keyed_nodes"],
        f["keyed100k"]["duplicate_keys"], r["load_warnings"],
    ));
    let rows = r["rows"].as_array().cloned().unwrap_or_default();
    let row = |name: &str| {
        rows.iter()
            .find(|x| x["name"] == name)
            .cloned()
            .unwrap_or(Value::Null)
    };
    md.push_str("## Stand-in against product (ap-34)\n\n| fixture | bench's own path (ms) | product path (ms) | bench's own walk d3 (ms) | product walk d3 (ms) |\n|---|---:|---:|---:|---:|\n");
    for k in ["real", "rich"] {
        md.push_str(&format!(
            "| {k} | {} | {} | {} | {} |\n",
            row(&format!("{k}/path_bench_copy_ms"))["median"],
            row(&format!("{k}/path_ms"))["median"],
            row(&format!("{k}/walk_d3_bench_copy_ms"))["median"],
            row(&format!("{k}/walk_d3_ms"))["median"],
        ));
    }
    md.push_str("\n## Rows\n\n| row | unit | n | median | p95 | max | path |\n|---|---|---:|---:|---:|---:|---|\n");
    for x in &rows {
        md.push_str(&format!(
            "| `{}` | {} | {} | {} | {} | {} | {} |\n",
            s(&x["name"]),
            s(&x["unit"]),
            x["n"],
            x["median"],
            x["p95"],
            x["max"],
            s(&x["path"])
        ));
    }
    if let Some(skipped) = r["skipped_rows"].as_array().filter(|s| !s.is_empty()) {
        let names: Vec<String> = skipped.iter().map(|n| format!("`{}`", s(n))).collect();
        md.push_str(&format!(
            "\nSkipped, no samples (the fixture is too small for their picks): {}.\n",
            names.join(", ")
        ));
    }
    md.push_str("\nNumbers report, they do not gate (D8). Samples and notes are in the .json beside this file; the query lists are in the -queries.json.\n");
    md
}

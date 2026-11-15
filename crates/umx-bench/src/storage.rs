//! Storage measurements (workstream 003 s01): a graph built straight through the codec and umx-graph, not through
//! the composed services, so the adapters' buffers are not counted. Walk and path are written here against
//! umx-graph's public reads (s02: handles and `edges_out`/`edges_in`); the check values prove a port does the same
//! work.

use crate::alloc;
use crate::fixture::{Fixture, Rng};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::hint::black_box;
use std::time::Instant;
use umx_core::{Edge, Id, Node};
use umx_graph::{Graph, NodeFilter, NodeIx, NodeRef, NodeSet, Op, Where, WordQuery};
use umx_services::{NEAR_DUPLICATE, NEAR_DUPLICATE_OTHER_TYPE};

const STARTS: usize = 1000;
const PAIRS: usize = 200;

/// Reusable walk state over node handles: a visit stamp per node (`NodeIx::index`), so starting a walk costs nothing.
pub struct Scratch {
    stamp: Vec<u32>,
    epoch: u32,
    cur: Vec<NodeIx>,
    next: Vec<NodeIx>,
}

impl Scratch {
    pub fn new(g: &Graph) -> Scratch {
        Scratch {
            stamp: vec![0; g.node_bound()],
            epoch: 0,
            cur: Vec::new(),
            next: Vec::new(),
        }
    }

    /// Starts a walk from `start`: nothing is seen but `start`.
    fn begin(&mut self, start: NodeIx) {
        self.epoch += 1;
        self.cur.clear();
        self.cur.push(start);
        self.stamp[start.index()] = self.epoch;
    }

    /// Marks `n` seen; true the first time in this walk.
    #[inline]
    fn see(&mut self, n: NodeIx) -> bool {
        let fresh = self.stamp[n.index()] != self.epoch;
        self.stamp[n.index()] = self.epoch;
        fresh
    }
}

/// Nodes reached from `start` breadth first to `depth`, every relation, both directions, start not counted.
pub fn walk(g: &Graph, s: &mut Scratch, start: NodeIx, depth: u32) -> usize {
    s.begin(start);
    let mut count = 0;
    for _ in 0..depth {
        s.next.clear();
        for i in 0..s.cur.len() {
            let id = s.cur[i];
            for o in g.edges_out(id).chain(g.edges_in(id)).map(|st| st.other) {
                if s.see(o) {
                    s.next.push(o);
                }
            }
        }
        count += s.next.len();
        if s.next.is_empty() {
            break;
        }
        std::mem::swap(&mut s.cur, &mut s.next);
    }
    count
}

/// Shortest hop count from `a` to `b`, breadth first, both directions, stopping when found.
pub fn hops(g: &Graph, s: &mut Scratch, a: NodeIx, b: NodeIx) -> Option<u32> {
    if a == b {
        return Some(0);
    }
    s.begin(a);
    let mut d = 0;
    while !s.cur.is_empty() {
        d += 1;
        s.next.clear();
        for i in 0..s.cur.len() {
            let id = s.cur[i];
            let mut found = false;
            for o in g.edges_out(id).chain(g.edges_in(id)).map(|st| st.other) {
                found |= o == b;
                if s.see(o) {
                    s.next.push(o);
                }
            }
            if found {
                return Some(d);
            }
        }
        std::mem::swap(&mut s.cur, &mut s.next);
    }
    None
}

/// Seeded picks, made once before timing: start nodes, then node pairs, uniform over the nodes in id order.
pub struct Picks {
    pub starts: Vec<NodeIx>,
    pub pairs: Vec<(NodeIx, NodeIx)>,
}

pub fn picks(seed: u64, ids: &[NodeIx]) -> Picks {
    let mut rng = Rng(seed);
    let n = ids.len();
    let starts = (0..STARTS).map(|_| ids[rng.below(n)]).collect();
    let pairs = (0..PAIRS)
        .map(|_| (ids[rng.below(n)], ids[rng.below(n)]))
        .collect();
    Picks { starts, pairs }
}

fn walk_total(g: &Graph, s: &mut Scratch, p: &Picks) -> usize {
    p.starts.iter().map(|&st| walk(g, s, st, 3)).sum()
}

fn hops_sum(g: &Graph, s: &mut Scratch, p: &Picks) -> u64 {
    p.pairs
        .iter()
        .map(|&(a, b)| u64::from(hops(g, s, a, b).unwrap_or(0)))
        .sum()
}

/// The services' load path (s05): the nodes file's bytes become the image, then the edges file is read into columns.
/// The copy of the nodes bytes is the graph's, so it is counted.
pub fn build(fx: &Fixture) -> Graph {
    let (mut g, nodes) = Graph::load_nodes(fx.nodes.clone()).expect("fixture nodes load");
    let edges = g.load_edges(&fx.edges).expect("fixture edges load");
    assert!(nodes.repeats.is_empty() && edges.repeats.is_empty() && edges.dangling.is_empty());
    g
}

/// Adds one node and 3 edges from it in memory, as `node add` with edges does, `samples` times; each sample is
/// undone (untimed) so the graph is the same for the next. Median and minimum in ms. The new ids are larger than
/// every id in the fixture, as a freshly minted id is.
fn add_node_and_3_edges(g: &mut Graph, samples: usize, targets: &[Id]) -> Value {
    let last = g.nodes().last().expect("fixture has nodes").to_node();
    let top = g
        .edges()
        .map(|e| e.id())
        .max()
        .unwrap_or(last.id)
        .max(last.id)
        .0;
    let relation = g
        .edges()
        .next()
        .expect("fixture has edges")
        .kind()
        .to_owned();
    let node = Node {
        id: Id(top + 1),
        ..last
    };
    let edges: Vec<Edge> = targets
        .iter()
        .zip(2..)
        .map(|(&to, i)| Edge {
            id: Id(top + i),
            kind: relation.clone(),
            from_graph: None,
            from: node.id,
            to_graph: None,
            to,
            created: node.created.clone(),
            created_by: "umx-bench".into(),
            authored_by_session: None,
            edited_by_session: None,
            evidence: Vec::new(),
            fields: Default::default(),
        })
        .collect();
    let (nodes, edge_count) = (g.node_count(), g.edge_count());
    let mut all: Vec<f64> = (0..samples)
        .map(|_| {
            let (n, es) = (node.clone(), edges.clone());
            let t = Instant::now();
            g.insert_node(n).expect("fits");
            for e in es {
                g.insert_edge(e);
            }
            let took = t.elapsed().as_secs_f64() * 1e3;
            for e in &edges {
                g.remove_edge(e.id);
            }
            g.remove_node(node.id);
            assert_eq!(
                (g.node_count(), g.edge_count()),
                (nodes, edge_count),
                "the add was undone"
            );
            round3(took)
        })
        .collect();
    all.sort_by(f64::total_cmp);
    json!({ "median": all[all.len() / 2], "min": all[0], "runs": samples })
}

/// Replaces one node in the middle of the graph with a longer title and note (one write in memory: the line, the
/// columns derived from it and everything after it moves), `samples` times; each sample is put back (untimed).
/// Median and minimum in ms.
fn replace_one_line(g: &mut Graph, samples: usize) -> Value {
    let mid = g
        .nodes()
        .nth(g.node_count() / 2)
        .expect("fixture has nodes")
        .id();
    let old = g.node(mid).expect("the node").to_node();
    let mut new = old.clone();
    new.title.push_str(" (edited)");
    new.note.push_str(" Edited again.");
    let before = g.encode_nodes().len();
    let mut all: Vec<f64> = (0..samples)
        .map(|_| {
            let n = new.clone();
            let t = Instant::now();
            let replaced = g.insert_node(n).expect("fits");
            let took = t.elapsed().as_secs_f64() * 1e3;
            assert_eq!(replaced.as_ref(), Some(&old), "the old record comes back");
            g.insert_node(old.clone()).expect("fits");
            round3(took)
        })
        .collect();
    assert_eq!(g.encode_nodes().len(), before, "the replace was undone");
    all.sort_by(f64::total_cmp);
    json!({ "median": all[all.len() / 2], "min": all[0], "runs": samples })
}

/// What `node add` does in memory before its write (s06): the near-duplicate check (the best 3 title matches) and
/// the insert, `samples` times; each sample is undone (untimed). The title is the one the CLI rows add.
fn near_duplicate_check_and_insert(g: &mut Graph, samples: usize) -> Value {
    let last = g.nodes().last().expect("fixture has nodes").to_node();
    let top = g
        .edges()
        .map(|e| e.id())
        .max()
        .unwrap_or(last.id)
        .max(last.id)
        .0;
    let node = Node {
        id: Id(top + 1),
        title: "Bench addition 0".into(),
        ..last
    };
    let count = g.node_count();
    let mut all: Vec<f64> = (0..samples)
        .map(|_| {
            let n = node.clone();
            let t = Instant::now();
            black_box(
                g.near_duplicates(
                    &n.title,
                    Some(&n.kind),
                    NEAR_DUPLICATE,
                    NEAR_DUPLICATE_OTHER_TYPE,
                    3,
                )
                .len(),
            );
            g.insert_node(n).expect("fits");
            let took = t.elapsed().as_secs_f64() * 1e3;
            g.remove_node(node.id);
            assert_eq!(g.node_count(), count, "the add was undone");
            round3(took)
        })
        .collect();
    all.sort_by(f64::total_cmp);
    json!({ "median": all[all.len() / 2], "min": all[0], "runs": samples })
}

/// Word search over the whole graph or a scope, figures included, as `umx search` runs it on one graph.
fn word_search(g: &Graph, q: &WordQuery, scope: Option<&NodeSet<'_>>) -> usize {
    let stats = g.word_stats(q);
    g.word_search(q, &stats, scope, 100).len()
}

/// Word search keys (ws004 s10, req-68): the index's build, memory and one-node update, and queries of four very
/// common words (the worst case), the same scoped to about a tenth of the nodes, one uncommon word with one common,
/// and two uncommon words. Words are picked by how many nodes hold them, from the first 1,000 nodes' texts.
fn word_keys(g: &mut Graph, runs: usize) -> Value {
    let n = g.node_count();
    let mut seen = std::collections::BTreeSet::new();
    for node in g.nodes().take(1000) {
        let note = node
            .field("note")
            .and_then(|v| v.as_str().map(str::to_owned));
        seen.extend(umx_graph::words(node.title()));
        seen.extend(umx_graph::words(note.as_deref().unwrap_or("")));
    }
    let mut by_df: Vec<(u64, String)> = seen
        .into_iter()
        .map(|w| {
            let q = WordQuery::new(&w).expect("a word");
            (g.word_stats(&q).df[0], w)
        })
        .collect();
    by_df.sort();
    let common: Vec<&str> = by_df.iter().rev().take(4).map(|x| x.1.as_str()).collect();
    let rare: Vec<&str> = by_df.iter().take(2).map(|x| x.1.as_str()).collect();
    let q = |words: &[&str]| WordQuery::new(&words.join(" ")).expect("words");
    let (q_common, q_mixed, q_rare) = (q(&common), q(&[rare[0], common[0]]), q(&rare));
    // A scope of about a tenth: the subtree, among the first 200 nodes', closest to n / 10 nodes.
    let tenth = g
        .nodes()
        .take(200)
        .map(|r| (g.within(r.id()).len().abs_diff(n / 10), r.id()))
        .min()
        .expect("fixture has nodes")
        .1;
    let scope = g.within(tenth);
    let scope_len = scope.len();
    let (common_ms, common_hits) = per_op(runs, 1, 1e3, || word_search(g, &q_common, None));
    let (scoped_ms, scoped_hits) = per_op(runs, 1, 1e3, || word_search(g, &q_common, Some(&scope)));
    let (mixed_ms, _) = per_op(runs, 1, 1e3, || word_search(g, &q_mixed, None));
    let (rare_ms, _) = per_op(runs, 1, 1e3, || word_search(g, &q_rare, None));
    drop(scope);
    // ws004 s21: four 4-letter prefixes of common words, and the 4-letter prefix that starts the most words. The rich
    // fixture's vocabulary has no stem siblings; stems and capped prefixes are timed on [`word_forms`].
    let words: Vec<&str> = by_df.iter().rev().map(|x| x.1.as_str()).collect();
    let letters = |w: &str| w.len() > 4 && w.bytes().all(|b| b.is_ascii_lowercase());
    let prefixes: Vec<&str> = words
        .iter()
        .filter(|w| letters(w))
        .map(|w| &w[..4])
        .take(4)
        .collect();
    let q_prefix = q(&prefixes);
    let mut starts: BTreeMap<&str, usize> = BTreeMap::new();
    for w in words.iter().filter(|w| letters(w)) {
        *starts.entry(&w[..4]).or_default() += 1;
    }
    let widest = starts
        .iter()
        .max_by_key(|(p, n)| (**n, std::cmp::Reverse(**p)))
        .map(|(p, _)| *p)
        .expect("fixture has words");
    let q_widest = q(&[widest]);
    let widest_terms = g.word_stats(&q_widest).variants[0].len();
    let (prefix_ms, prefix_hits) = per_op(runs, 1, 1e3, || word_search(g, &q_prefix, None));
    let (widest_ms, _) = per_op(runs, 1, 1e3, || word_search(g, &q_widest, None));
    // One node's texts changed and changed back: two updates per run.
    let mid = g.nodes().nth(n / 2).expect("fixture has nodes").to_node();
    let changed = Node {
        title: format!("{} revised", mid.title),
        note: format!("{} Revised with new words: zygote quorum.", mid.note),
        ..mid.clone()
    };
    let (replace_ms, _) = per_op(runs.max(9), 2, 1e3, || {
        g.insert_node(changed.clone()).expect("the image fits");
        g.insert_node(mid.clone()).expect("the image fits");
        g.node_count()
    });
    // The index's own part of that update (out, then in), apart from the image splice every replace does.
    let (update_ms, _) = per_op(runs.max(9), 1, 1e3, || {
        g.reindex_words(changed.id);
        g.node_count()
    });
    let (build_ms, _) = per_op(runs.max(9), 1, 1e3, || {
        g.rebuild_words();
        g.memory_bytes().word_index
    });
    let mem = g.memory_bytes().word_index;
    json!({
        "word_index_build_ms": build_ms,
        "mem_word_index_per_node_bytes": (mem as f64 / n as f64).round(),
        "word_query_common4_ms": common_ms,
        "word_query_common4_hits": common_hits,
        "word_query_common4_scoped_ms": scoped_ms,
        "word_query_scope_nodes": scope_len,
        "word_query_scoped_hits": scoped_hits,
        "word_query_uncommon_common_ms": mixed_ms,
        "word_query_uncommon2_ms": rare_ms,
        "word_update_one_node_ms": update_ms,
        "replace_one_node_ms": replace_ms,
        "word_query_words": { "common": common, "rare": rare },
        "word_query_prefix4_ms": prefix_ms,
        "word_query_prefix4_hits": prefix_hits,
        "word_query_widest_prefix_ms": widest_ms,
        "word_query_s21_words": {
            "prefix4": prefixes, "widest_prefix": widest, "widest_prefix_terms": widest_terms,
        },
    })
}

pub fn round3(v: f64) -> f64 {
    (v * 1e3).round() / 1e3
}

/// Times `runs` calls of `f` doing `ops` operations each; median, 95th percentile and minimum per operation, in
/// seconds times `scale` (1e3 for ms, 1e6 for us, 1e9 for ns). Every call must return the same value, which is returned.
pub fn per_op<R: PartialEq + std::fmt::Debug>(
    runs: usize,
    ops: usize,
    scale: f64,
    mut f: impl FnMut() -> R,
) -> (Value, R) {
    let mut first = None;
    let mut all: Vec<f64> = (0..runs)
        .map(|_| {
            let t = Instant::now();
            let r = black_box(f());
            let took = t.elapsed().as_secs_f64() * scale / ops as f64;
            match &first {
                None => first = Some(r),
                Some(x) => assert_eq!(*x, r, "a timed operation changed its answer between runs"),
            }
            round3(took)
        })
        .collect();
    all.sort_by(f64::total_cmp);
    (
        json!({
            "median": all[all.len() / 2], "p95": all[(all.len() * 95 / 100).min(all.len() - 1)], "min": all[0],
            "runs": runs,
        }),
        first.expect("runs >= 1"),
    )
}

/// The s01 storage keys, merged into the result's `in_process` object.
pub fn measure(runs: usize, seed: u64, fx: &Fixture) -> Value {
    // The file bytes are already held in `fx`, so neither figure counts them.
    let base = alloc::live();
    alloc::reset_peak();
    let mut g = build(fx);
    let load_peak = alloc::peak_since(base);
    let mem = alloc::live_since(base);
    let n = g.node_count();
    let ids: Vec<NodeIx> = g.nodes().map(NodeRef::ix).collect();
    let p = picks(seed, &ids);
    let (root, one_pct) = (g.node_at(ids[0]).id(), g.node_at(ids[n / 100]).id());

    let parts = g.memory_bytes();
    let mut s = Scratch::new(&g);
    let (walk_us, walked) = per_op(runs, STARTS, 1e6, || walk_total(&g, &mut s, &p));
    let (path_us, hop_sum) = per_op(runs, PAIRS, 1e6, || hops_sum(&g, &mut s, &p));
    let (within_root_ms, root_count) = per_op(runs, 1, 1e3, || g.within(root).len());
    let (within_1pct_ms, pct_count) = per_op(runs, 1, 1e3, || g.within(one_pct).len());
    let (get_title_ns, _) = per_op(runs, STARTS, 1e9, || {
        p.starts
            .iter()
            .map(|&s| g.node_at(s).title().len())
            .sum::<usize>()
    });
    let (get_node_ns, _) = per_op(runs, STARTS, 1e9, || {
        p.starts
            .iter()
            .map(|&s| black_box(g.node_at(s).to_node()).note.len())
            .sum::<usize>()
    });
    let by_type_status = NodeFilter {
        kind: Some("problem".into()),
        status: Some("open".into()),
        ..Default::default()
    };
    let (list_type_status_ms, listed) =
        per_op(runs, 1, 1e3, || g.list_nodes(&by_type_status).len());
    let by_type_field = NodeFilter {
        kind: Some("problem".into()),
        wheres: vec![Where {
            field: "likelihood".into(),
            op: Op::Ge,
            value: json!(3),
        }],
        ..Default::default()
    };
    let (list_type_field_ms, listed_field) =
        per_op(runs, 1, 1e3, || g.list_nodes(&by_type_field).len());
    let (encode_nodes_ms, nodes_len) = per_op(runs, 1, 1e3, || g.encode_nodes().len());
    let (encode_edges_ms, edges_len) = per_op(runs, 1, 1e3, || g.encode_edges().len());
    assert_eq!(
        (nodes_len, edges_len),
        (fx.nodes.len(), fx.edges.len()),
        "the loaded graph must encode back to the fixture's sizes"
    );
    let targets: Vec<Id> = p.starts[..3].iter().map(|&s| g.node_at(s).id()).collect();
    let add_node_3_edges_ms = add_node_and_3_edges(&mut g, runs.max(9), &targets);
    let replace_one_line_ms = replace_one_line(&mut g, runs.max(9));
    let near_duplicate_insert_ms = near_duplicate_check_and_insert(&mut g, runs.max(9));
    let (rebuild_adjacency_ms, _) = per_op(runs.max(9), 1, 1e3, || g.rebuild_adjacency());
    let words = word_keys(&mut g, runs);
    let mut out = json!({
        "walk_depth3_us": walk_us,
        "walk_depth3_reached": walked as f64 / STARTS as f64,
        "path_us": path_us,
        "path_hops_sum": hop_sum,
        "within_root_ms": within_root_ms,
        "within_root_count": root_count,
        "within_1pct_ms": within_1pct_ms,
        "within_1pct_count": pct_count,
        "get_title_ns": get_title_ns,
        "get_node_ns": get_node_ns,
        "mem_graph_bytes": mem,
        "mem_per_node_bytes": (mem as f64 / n as f64).round(),
        "load_peak_bytes": load_peak,
        "encode_nodes_ms": encode_nodes_ms,
        "encode_edges_ms": encode_edges_ms,
        "list_type_status_ms": list_type_status_ms,
        "list_type_status_count": listed,
        "list_type_field_ms": list_type_field_ms,
        "list_type_field_count": listed_field,
        "add_node_3_edges_ms": add_node_3_edges_ms,
        "replace_one_line_ms": replace_one_line_ms,
        "near_duplicate_insert_ms": near_duplicate_insert_ms,
        "rebuild_adjacency_ms": rebuild_adjacency_ms,
        "mem_parts_bytes": {
            "node_columns": parts.node_columns,
            "edge_columns": parts.edge_columns,
            "adjacency": parts.adjacency,
            "field_columns": parts.field_columns,
            "image": parts.image,
            "cold": parts.cold,
            "word_index": parts.word_index,
        },
        "mem_hot_per_node_bytes": (parts.hot() as f64 / n as f64).round(),
        "mem_not_image_per_node_bytes": ((mem as f64 - parts.image as f64) / n as f64).round(),
        "memory_bytes_over_alloc": round3(parts.total() as f64 / mem as f64),
    });
    out.as_object_mut()
        .expect("an object")
        .extend(words.as_object().expect("an object").clone());
    out["title_find"] = title_find_keys(&mut g, runs.max(9));
    out["title_find_large_vocabulary"] = large_vocabulary(runs.max(9), seed);
    out["title_find_names"] = title_find_names(runs.max(9), seed);
    out["word_forms"] = word_forms(runs.max(9), seed);
    out["entity_keys"] = entity_keys(runs.max(3), seed, 100_000, 5_000);
    out
}

/// ws008 E1 (graph-storage-expert's S208-1): loading a rich `n`-node graph with no keys and with `keyed` nodes
/// holding one `email:` key each, and one keyed node's edit (median, each undone). A keyless row never scans the
/// key index, so the two loads should cost about the same.
pub(crate) fn entity_keys(runs: usize, seed: u64, n: usize, keyed: usize) -> Value {
    let fx = crate::fixture::generate(seed, n, 3, true);
    let (plain, _) = Graph::load_nodes(fx.nodes.clone()).expect("fixture nodes load");
    let step = (n / keyed.max(1)).max(1);
    let mut keyed_g = plain.clone();
    let mut first = None;
    for (i, id) in plain.nodes().map(|r| r.id()).enumerate() {
        if i % step != 0 || i / step >= keyed {
            continue;
        }
        let mut node = keyed_g.node(id).expect("the node").to_node();
        node.fields.insert(
            umx_core::entity::ENTITY_KEY.into(),
            json!([format!("email:p{i}@example.com")]),
        );
        keyed_g.insert_node(node).expect("fits");
        first.get_or_insert(id);
    }
    let keyed_bytes = keyed_g.encode_nodes();
    let (load_plain_ms, plain_n) = per_op(runs, 1, 1e3, || {
        Graph::load_nodes(fx.nodes.clone())
            .expect("loads")
            .0
            .node_count()
    });
    let (load_keyed_ms, keyed_n) = per_op(runs, 1, 1e3, || {
        Graph::load_nodes(keyed_bytes.clone())
            .expect("loads")
            .0
            .node_count()
    });
    assert_eq!(plain_n, keyed_n);
    let (mut g, _) = Graph::load_nodes(keyed_bytes).expect("loads");
    let id = first.expect("a keyed node");
    let old = g.node(id).expect("the node").to_node();
    let mut new = old.clone();
    new.title.push_str(" (edited)");
    new.fields.insert(
        umx_core::entity::ENTITY_KEY.into(),
        json!(["email:edited@example.com"]),
    );
    let mut all: Vec<f64> = (0..runs.max(9))
        .map(|_| {
            let t = Instant::now();
            g.insert_node(new.clone()).expect("fits");
            let took = t.elapsed().as_secs_f64() * 1e3;
            g.insert_node(old.clone()).expect("fits");
            round3(took)
        })
        .collect();
    all.sort_by(f64::total_cmp);
    json!({
        "nodes": n, "keyed": keyed.min(n.div_ceil(step)),
        "load_no_keys_ms": load_plain_ms, "load_keyed_ms": load_keyed_ms,
        "edit_keyed_node_ms": { "median": all[all.len() / 2], "min": all[0], "runs": all.len() },
        "duplicates": g.duplicate_entity_keys().count(),
    })
}

/// One character of `text` replaced (the middle one), for a typo query.
fn typo(text: &str) -> String {
    let mut c: Vec<char> = text.chars().collect();
    let i = c.len() / 2;
    c[i] = if c[i] == 'x' { 'y' } else { 'x' };
    c.into_iter().collect()
}

/// ws004 s15: title find on the word index against s06's scan, on the same graph and queries. `branch` is
/// `Graph::find` (candidates from the word index); `s06` scans every title's kept normalised text, as s06 did.
/// `worst` is a query holding a word of every title, so every node is a candidate; `common_word` is the word in the
/// most titles, alone (ws004 s24: every title holding it is found).
fn title_find_keys(g: &mut Graph, runs: usize) -> Value {
    let n = g.node_count();
    let titles = g.scan_titles();
    let exact = g.nodes().nth(n / 2).expect("nodes").title().to_owned();
    let mut first_words = std::collections::BTreeSet::new();
    for node in g.nodes() {
        if let Some(w) = umx_graph::words(node.title()).into_iter().next() {
            first_words.insert(w);
        }
    }
    // The worst case for the bound (ws004 s15): of a seeded set (every first word, each alone, all of them joined,
    // and 300 titles whole, typoed and joined), the query that keeps the most titles as candidates.
    let first_words: Vec<String> = first_words.into_iter().collect();
    let mut tries: Vec<String> = first_words.clone();
    tries.push(first_words.join(" "));
    let step = (n / 300).max(1);
    for node in g.nodes().step_by(step) {
        let t = node.title().to_owned();
        tries.push(typo(&t));
        tries.push(t.replace(' ', ""));
        tries.push(t);
    }
    let worst = tries
        .into_iter()
        .max_by_key(|q| g.title_candidates(q).0)
        .expect("queries");
    // ws004 s24's worst case: the word in the most titles, alone, so every title holding it is a candidate.
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for node in g.nodes() {
        for w in umx_graph::words(node.title()) {
            *counts.entry(w).or_default() += 1;
        }
    }
    let common = counts
        .into_iter()
        .max_by_key(|(w, c)| (*c, std::cmp::Reverse(w.clone())))
        .map(|(w, _)| w)
        .expect("words");
    let mut rows = serde_json::Map::new();
    for (name, q) in [
        ("exact", exact.clone()),
        ("typo", typo(&exact)),
        ("worst", worst),
        ("common_word", common),
    ] {
        let (branch, b) = per_op(runs, 1, 1e3, || {
            g.find(&q, None, 20)
                .iter()
                .map(|(r, s)| (r.id(), *s))
                .collect::<Vec<_>>()
        });
        let (s06, a) = per_op(runs, 1, 1e3, || g.find_scan(&titles, &q, 20));
        let (candidates, _) = g.title_candidates(&q);
        rows.insert(
            name.into(),
            json!({ "query": q, "branch_ms": branch, "s06_ms": s06, "candidates": candidates, "same_top20": a == b }),
        );
    }
    // The near-duplicate check alone for a title already in the graph (a real duplicate), branch and s06.
    let (near_existing, a) = per_op(runs, 1, 1e3, || {
        g.near_duplicates(&exact, None, 0.85, 0.85, 3)
            .iter()
            .map(|(r, s)| (r.id(), *s))
            .collect::<Vec<_>>()
    });
    let (near_existing_s06, _) = per_op(runs, 1, 1e3, || {
        let mut v = g.find_scan(&titles, &exact, 3);
        v.retain(|(_, s)| *s >= 0.85);
        v
    });
    // s06's answer with ws004 s30's keep-rule, chosen before the best 3 (the timed s06 row above is the scan alone).
    let mut want = g.find_scan(&titles, &exact, usize::MAX);
    want.retain(|(id, s)| {
        *s >= 0.85 && umx_graph::near_duplicate_keeps(&exact, g.node(*id).expect("held").title())
    });
    want.truncate(3);
    assert_eq!(a, want, "near-duplicate answers differ");
    // ws004 s18: the candidate step alone at 0.85, this build's and s15's (its term scan), in the same run.
    let (cand_s15, c15) = per_op(runs, 1, 1e3, || g.title_candidates_s15_at(&exact, 0.85));
    let (cand, c) = per_op(runs, 1, 1e3, || g.title_candidates_near(&exact));
    assert_eq!(c, c15, "s18's near-duplicate candidates differ from s15's");
    rows.insert(
        "near_duplicate_existing".into(),
        json!({
            "query": exact, "branch_ms": near_existing, "s06_ms": near_existing_s06, "candidates": c,
            "candidates_ms": cand, "candidates_s15_ms": cand_s15,
        }),
    );
    // node add's near-duplicate check (best 3) plus the insert, as s06 timed it, scan side.
    let near_s06 = {
        let last = g.nodes().last().expect("nodes").to_node();
        let node = Node {
            id: Id(last.id.0 + 1_000_000),
            title: "Bench addition 0".into(),
            ..last
        };
        let mut all: Vec<f64> = (0..runs)
            .map(|_| {
                let t = Instant::now();
                black_box(g.find_scan(&titles, &node.title, 3).len());
                g.insert_node(node.clone()).expect("fits");
                let took = t.elapsed().as_secs_f64() * 1e3;
                g.remove_node(node.id);
                round3(took)
            })
            .collect();
        all.sort_by(f64::total_cmp);
        json!({ "median": all[all.len() / 2], "min": all[0], "runs": runs })
    };
    let (_, terms) = g.title_candidates("x");
    json!({
        "queries": rows,
        "near_duplicate_insert_s06_ms": near_s06,
        "terms": terms,
        "s06_title_text_per_node_bytes": (titles.heap_bytes() as f64 / n as f64).round(),
    })
}

/// ws004 s30's worst case for the short-query word rule (graph-storage-expert): 100k people-like titles, a first and
/// a last name from 80 each, and ten two-word queries from ten other first and last names, so no query is held and
/// most hits are found on whole-title Dice alone (each then checked word by word). Times the ten queries together.
fn title_find_names(runs: usize, seed: u64) -> Value {
    const FIRST: &[&str] = &[
        "james",
        "mary",
        "robert",
        "patricia",
        "john",
        "jennifer",
        "michael",
        "linda",
        "david",
        "elizabeth",
        "william",
        "barbara",
        "richard",
        "susan",
        "joseph",
        "jessica",
        "thomas",
        "sarah",
        "charles",
        "karen",
        "daniel",
        "lisa",
        "matthew",
        "nancy",
        "anthony",
        "betty",
        "mark",
        "sandra",
        "donald",
        "ashley",
        "steven",
        "kimberly",
        "paul",
        "emily",
        "andrew",
        "donna",
        "joshua",
        "michelle",
        "kenneth",
        "carol",
        "kevin",
        "amanda",
        "brian",
        "melissa",
        "george",
        "deborah",
        "timothy",
        "stephanie",
        "ronald",
        "rebecca",
        "jason",
        "sharon",
        "edward",
        "laura",
        "jeffrey",
        "cynthia",
        "ryan",
        "dorothy",
        "jacob",
        "amy",
        "gary",
        "kathleen",
        "nicholas",
        "angela",
        "eric",
        "shirley",
        "jonathan",
        "emma",
        "stephen",
        "brenda",
        "larry",
        "pamela",
        "justin",
        "nicole",
        "scott",
        "anna",
        "brandon",
        "samantha",
        "benjamin",
        "katherine",
        // Query names only.
        "jacqueline",
        "madison",
        "teresa",
        "gerald",
        "abigail",
        "harold",
        "gloria",
        "janice",
        "bryan",
        "kathryn",
    ];
    const LAST: &[&str] = &[
        "smith",
        "johnson",
        "williams",
        "brown",
        "jones",
        "garcia",
        "miller",
        "davis",
        "rodriguez",
        "martinez",
        "hernandez",
        "lopez",
        "gonzalez",
        "wilson",
        "anderson",
        "thomas",
        "taylor",
        "moore",
        "jackson",
        "martin",
        "lee",
        "perez",
        "thompson",
        "white",
        "harris",
        "sanchez",
        "clark",
        "ramirez",
        "lewis",
        "robinson",
        "walker",
        "young",
        "allen",
        "king",
        "wright",
        "scott",
        "torres",
        "nguyen",
        "hill",
        "flores",
        "green",
        "adams",
        "nelson",
        "baker",
        "hall",
        "rivera",
        "campbell",
        "mitchell",
        "carter",
        "roberts",
        "gomez",
        "phillips",
        "evans",
        "turner",
        "diaz",
        "parker",
        "cruz",
        "edwards",
        "collins",
        "reyes",
        "stewart",
        "morris",
        "morales",
        "murphy",
        "cook",
        "rogers",
        "gutierrez",
        "ortiz",
        "morgan",
        "cooper",
        "peterson",
        "bailey",
        "reed",
        "kelly",
        "howard",
        "ramos",
        "kim",
        "cox",
        "ward",
        "richardson",
        // Query names only.
        "moreno",
        "salazar",
        "cunningham",
        "hamilton",
        "fernandez",
        "washington",
        "kennedy",
        "mcdonald",
        "gibson",
        "vargas",
    ];
    let mut rng = Rng(seed ^ 0x5300);
    let base = umx_core::Id(0x0195_0000_0000_u128 << 80);
    let nodes: Vec<Node> = (0..100_000u128)
        .map(|i| Node {
            id: Id(base.0 | i),
            kind: "person".into(),
            title: format!("{} {}", FIRST[rng.below(80)], LAST[rng.below(80)]),
            status: "open".into(),
            note: String::new(),
            evidence: Vec::new(),
            status_history: Vec::new(),
            created: "2026-11-12T00:00:00Z".into(),
            updated: "2026-11-12T00:00:00Z".into(),
            created_by: "bench".into(),
            locator: None,
            authored_by_session: None,
            edited_by_session: None,
            fields: Default::default(),
        })
        .collect();
    let g = Graph::from_records(nodes, Vec::new());
    let queries: Vec<String> = (0..10)
        .map(|i| format!("{} {}", FIRST[80 + i], LAST[80 + (i * 3) % 10]))
        .collect();
    let (ten_ms, found) = per_op(runs, 1, 1e3, || {
        queries
            .iter()
            .map(|q| g.find(q, None, 20).len())
            .sum::<usize>()
    });
    let candidates: usize = queries.iter().map(|q| g.title_candidates(q).0).sum();
    json!({ "nodes": 100_000, "queries": queries.len(), "ten_queries_ms": ten_ms, "found": found, "candidates": candidates })
}

/// A large vocabulary (the rich fixture's is tiny): 100k nodes whose titles are three words drawn from 60k seeded
/// pseudo-words, so the term table, not the titles, is what a typo word scans.
fn large_vocabulary(runs: usize, seed: u64) -> Value {
    let mut rng = Rng(seed ^ 0x5115);
    let vocab: Vec<String> = (0..60_000)
        .map(|_| {
            (0..4 + rng.below(7))
                .map(|_| (b'a' + rng.below(26) as u8) as char)
                .collect()
        })
        .collect();
    let base = umx_core::Id(0x0194_0000_0000_u128 << 80);
    let nodes: Vec<Node> = (0..100_000u128)
        .map(|i| {
            let title = (0..3)
                .map(|_| vocab[rng.below(vocab.len())].as_str())
                .collect::<Vec<_>>()
                .join(" ");
            Node {
                id: Id(base.0 | i),
                kind: "problem".into(),
                title,
                status: "open".into(),
                note: String::new(),
                evidence: Vec::new(),
                status_history: Vec::new(),
                created: "2026-11-10T00:00:00Z".into(),
                updated: "2026-11-10T00:00:00Z".into(),
                created_by: "bench".into(),
                locator: None,
                authored_by_session: None,
                edited_by_session: None,
                fields: Default::default(),
            }
        })
        .collect();
    // Load with this vocabulary (ws004 s21: Porter2 and the order run once per new term at load).
    let (load_ms, _) = per_op(3, 1, 1e3, || {
        Graph::from_records(nodes.clone(), Vec::new()).node_count()
    });
    let g = Graph::from_records(nodes, Vec::new());
    let titles = g.scan_titles();
    let exact = g.nodes().nth(50_000).expect("nodes").title().to_owned();
    let mut rows = serde_json::Map::new();
    for (name, q) in [("exact", exact.clone()), ("typo", typo(&exact))] {
        let (branch, b) = per_op(runs, 1, 1e3, || {
            g.find(&q, None, 20)
                .iter()
                .map(|(r, s)| (r.id(), *s))
                .collect::<Vec<_>>()
        });
        let (s06, a) = per_op(runs, 1, 1e3, || g.find_scan(&titles, &q, 20));
        let (term_scan, (candidates, _)) = per_op(runs, 1, 1e3, || g.title_candidates(&q));
        let (term_scan_s15, s15) = per_op(runs, 1, 1e3, || g.title_candidates_s15(&q));
        assert_eq!(candidates, s15, "s18's candidates differ from s15's");
        rows.insert(
            name.into(),
            json!({
                "branch_ms": branch, "s06_ms": s06, "term_scan_ms": term_scan, "term_scan_s15_ms": term_scan_s15,
                "candidates": candidates, "same_top20": a == b,
            }),
        );
    }
    let (_, terms) = g.title_candidates("x");
    // ws004 s18: the term-side bigram figures (kept bigrams, masks, d), per bigram held and in total.
    let (bytes, bigrams) = g.term_grams_bytes();
    json!({
        "nodes": 100_000, "terms": terms, "queries": rows,
        "term_grams": { "bytes": bytes, "bigrams": bigrams, "bytes_per_bigram": round3(bytes as f64 / bigrams as f64) },
        "load_ms": load_ms,
        "s21": s21_bytes(&g),
    })
}

/// ws004 s21 (graph-storage-expert's gates): 100k nodes whose titles are three words drawn from 20,000 seeded
/// pseudo-words in four English forms each (`w`, `ws`, `wing`, `wed`: one Porter2 class), 500 of them starting
/// `plan`, so `plan` starts 2,000 words and is capped at 64. Times four words that expand to their four-form classes
/// against the same words matched exactly only, four 4-letter prefixes, the capped prefix, load, and the order's and
/// stem classes' bytes per term.
pub(crate) fn word_forms(runs: usize, seed: u64) -> Value {
    let mut rng = Rng(seed ^ 0x5210);
    let letters = |rng: &mut Rng, n: usize| -> String {
        (0..n)
            .map(|_| (b'a' + rng.below(26) as u8) as char)
            .collect()
    };
    let bases: Vec<String> = (0..20_000)
        .map(|i| {
            let n = 3 + rng.below(4);
            let tail = letters(&mut rng, n);
            // Ending every base in `k` keeps its four forms in one Porter2 class (no Step 2 to 5 suffix ends in k)
            // when it has a vowel and is not short; the stem words below are picked from those that do.
            if i < 500 {
                format!("plan{tail}k")
            } else {
                format!("{}{tail}k", letters(&mut rng, 2))
            }
        })
        .collect();
    let forms = ["", "s", "ing", "ed"];
    let base = umx_core::Id(0x0194_0000_0000_u128 << 80);
    let nodes: Vec<Node> = (0..100_000u128)
        .map(|i| {
            let title = (0..3)
                .map(|_| format!("{}{}", bases[rng.below(bases.len())], forms[rng.below(4)]))
                .collect::<Vec<_>>()
                .join(" ");
            Node {
                id: Id(base.0 | i),
                kind: "problem".into(),
                title,
                status: "open".into(),
                note: String::new(),
                evidence: Vec::new(),
                status_history: Vec::new(),
                created: "2026-11-10T00:00:00Z".into(),
                updated: "2026-11-10T00:00:00Z".into(),
                created_by: "bench".into(),
                locator: None,
                authored_by_session: None,
                edited_by_session: None,
                fields: Default::default(),
            }
        })
        .collect();
    let (load_ms, _) = per_op(3, 1, 1e3, || {
        Graph::from_records(nodes.clone(), Vec::new()).node_count()
    });
    let g = Graph::from_records(nodes, Vec::new());
    let q = |text: &str| WordQuery::new(text).expect("words");
    let stem_words: Vec<&str> = bases[1000..]
        .iter()
        .map(String::as_str)
        .filter(|w| {
            let stats = g.word_stats(&q(w));
            stats.variants[0].values().filter(|x| x.stem).count() == forms.len()
        })
        .take(4)
        .collect();
    let q_stem = q(&stem_words.join(" "));
    let class_sizes: Vec<usize> = g
        .word_stats(&q_stem)
        .variants
        .iter()
        .map(|v| v.values().filter(|x| x.stem).count())
        .collect();
    let (stem_ms, stem_hits) = per_op(runs, 1, 1e3, || word_search(&g, &q_stem, None));
    let (exact_ms, exact_hits) = per_op(runs, 1, 1e3, || {
        let mut stats = g.word_stats(&q_stem);
        for (word, v) in q_stem.terms().iter().zip(&mut stats.variants) {
            v.retain(|k, _| k == word);
        }
        g.word_search(&q_stem, &stats, None, 100).len()
    });
    let prefixes: Vec<&str> = bases[2000..2004].iter().map(|b| &b[..4]).collect();
    let q_prefix = q(&prefixes.join(" "));
    let (prefix_ms, prefix_hits) = per_op(runs, 1, 1e3, || word_search(&g, &q_prefix, None));
    let q_capped = q("plan");
    let capped = g
        .word_stats(&q_capped)
        .capped(&q_capped)
        .first()
        .map_or(0, |c| c.1);
    let (capped_ms, capped_hits) = per_op(runs, 1, 1e3, || word_search(&g, &q_capped, None));
    let (bytes, terms) = g.word_order_and_stem_bytes();
    json!({
        "nodes": 100_000, "terms": terms, "load_ms": load_ms,
        "stem4_ms": stem_ms, "stem4_hits": stem_hits, "stem4_class_sizes": class_sizes,
        "stem4_exact_only_ms": exact_ms, "stem4_exact_only_hits": exact_hits,
        "prefix4_ms": prefix_ms, "prefix4_hits": prefix_hits, "prefix4": prefixes,
        "capped_prefix_ms": capped_ms, "capped_prefix_hits": capped_hits, "capped_prefix_terms": capped,
        "s21": { "bytes": bytes, "terms": terms, "bytes_per_term": round3(bytes as f64 / terms as f64) },
    })
}

/// ws004 s21: the term table's order and stem classes, per distinct term (graph-storage-expert's gate: 64 B).
fn s21_bytes(g: &Graph) -> Value {
    let (bytes, terms) = g.word_order_and_stem_bytes();
    json!({ "bytes": bytes, "terms": terms, "bytes_per_term": round3(bytes as f64 / terms as f64) })
}

#[cfg(test)]
mod title_find_differential {
    //! ws004 s15 on the rich fixture at 10k: `Graph::find` (word-index candidates) against s06's scan
    //! (`Graph::find_scan`): the same answer, to the bit, with split and joined words in the queries.

    use super::*;
    use crate::fixture::generate;

    fn typo(rng: &mut Rng, w: &str) -> String {
        let mut c: Vec<char> = w.chars().collect();
        let i = rng.below(c.len());
        match rng.below(3) {
            0 if c.len() > 3 => {
                c.remove(i);
            }
            1 => c.insert(i, c[i]),
            _ => c[i] = (b'a' + rng.below(26) as u8) as char,
        }
        c.into_iter().collect()
    }

    /// ws004 s24's reference: every title scored by `title_score`, and one holding each query word (repeats
    /// counted) lifted to max(d, 0.5 + 0.2 d); best first, then by id.
    fn rule_scan(g: &Graph, q: &str, kind: Option<&str>) -> Vec<(Id, f64)> {
        let qw = umx_graph::words(q);
        let mut v: Vec<(Id, f64)> = g
            .nodes()
            .filter(|n| kind.is_none_or(|k| n.kind() == k))
            .map(|n| {
                let d = umx_graph::title_score(q, n.title());
                let mut tw = umx_graph::words(n.title());
                let holds = !qw.is_empty()
                    && qw.iter().all(|w| match tw.iter().position(|x| x == w) {
                        Some(i) => {
                            tw.swap_remove(i);
                            true
                        }
                        None => false,
                    });
                let lifted = ((0.5 + 0.2 * d) * 1000.0 + 0.5) as u64 as f64 / 1000.0;
                let s = if holds && lifted > d { lifted } else { d };
                (n.id(), s, umx_graph::find_keeps(q, n.title(), s, holds))
            })
            .filter(|(_, s, keeps)| *s >= 0.4 && *keeps)
            .map(|(id, s, _)| (id, s))
            .collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        v
    }

    #[test]
    fn rich_10k_gives_s06s_answer() {
        let fx = generate(7, 10_000, 3, true);
        let g = build(&fx);
        let titles = g.scan_titles();
        let all: Vec<(String, String)> = g
            .nodes()
            .map(|n| (n.title().to_owned(), n.kind().to_owned()))
            .collect();
        let mut rng = Rng(46);
        let mut hits = 0;
        let count = 400;
        for _ in 0..count {
            let (title, kind) = &all[rng.below(all.len())];
            let mut words: Vec<String> = title.split_whitespace().map(str::to_owned).collect();
            match rng.below(5) {
                0 if words.len() > 1 => {
                    let i = rng.below(words.len());
                    words.remove(i);
                }
                1 => words.reverse(),
                _ => {}
            }
            let per_word = rng.below(3);
            let words: Vec<String> = words
                .iter()
                .map(|w| {
                    let mut w = w.clone();
                    for _ in 0..per_word {
                        if w.chars().count() > 3 {
                            w = typo(&mut rng, &w);
                        }
                    }
                    w
                })
                .collect();
            let mut q = words.join(" ");
            match rng.below(4) {
                // Split: a space inside a word.
                0 => {
                    let c: Vec<char> = q.chars().collect();
                    let i = 1 + rng.below(c.len().max(2) - 1);
                    let (a, b) = c.split_at(i.min(c.len()));
                    q = a.iter().chain([&' ']).chain(b).collect();
                }
                // Joined: spaces between words taken out.
                1 => q = q.replacen(' ', "", 1 + rng.below(3)),
                _ => {}
            }
            let filter = (rng.below(4) == 0).then_some(kind.as_str());
            let bits = |v: Vec<(Id, f64)>| -> Vec<(Id, u64)> {
                v.into_iter().map(|(id, s)| (id, s.to_bits())).collect()
            };
            let s06 = bits(
                g.find_scan(&titles, &q, usize::MAX)
                    .into_iter()
                    .filter(|(id, _)| {
                        filter.is_none_or(|k| g.node(*id).is_some_and(|n| n.kind() == k))
                    })
                    .collect(),
            );
            let branch = bits(
                g.find(&q, filter, usize::MAX)
                    .into_iter()
                    .map(|(n, s)| (n.id(), s))
                    .collect(),
            );
            assert_eq!(
                branch,
                bits(rule_scan(&g, &q, filter)),
                "{q:?} {filter:?}: the full list differs from the reference"
            );
            if filter.is_none() {
                let near: Vec<(Id, u64)> = s06
                    .iter()
                    .filter(|(_, b)| f64::from_bits(*b) >= 0.85)
                    .filter(|(id, _)| {
                        umx_graph::near_duplicate_keeps(&q, g.node(*id).expect("held").title())
                    })
                    .take(3)
                    .copied()
                    .collect();
                let checked = bits(
                    g.near_duplicates(&q, None, 0.85, 0.85, 3)
                        .into_iter()
                        .map(|(n, s)| (n.id(), s))
                        .collect(),
                );
                assert_eq!(checked, near, "{q:?}: near_duplicates differs from s06");
            }
            hits += usize::from(!branch.is_empty());
        }
        assert!(hits >= count / 2, "{hits} of {count} queries hit");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::generate;
    use umx_core::codec;

    /// ws008 E1: the entity key bench row runs at a small size and finds every key once.
    #[test]
    fn the_entity_key_row_loads_keyed_and_plain_graphs() {
        let v = entity_keys(1, 1, 2_000, 100);
        assert_eq!(
            (v["nodes"].as_u64(), v["keyed"].as_u64()),
            (Some(2_000), Some(100))
        );
        assert_eq!(v["duplicates"], 0);
        assert!(
            v["load_keyed_ms"]["median"].as_f64().is_some()
                && v["load_no_keys_ms"]["median"].as_f64().is_some()
                && v["edit_keyed_node_ms"]["median"].as_f64().is_some(),
            "{v}"
        );
    }

    /// The entity key row at its bench size (100k rich nodes, 5k keyed), printed for a result note. Run it with
    /// `cargo test -p umx-bench --release --bin umx-bench -- --ignored --nocapture the_entity_key_row_at_bench_size`.
    #[test]
    #[ignore = "release-only measurement"]
    fn the_entity_key_row_at_bench_size() {
        println!("{}", entity_keys(5, 1, 100_000, 5_000));
    }

    /// The check values at 1k nodes, seed 1: (walk total over 1,000 starts, hop sum over 200 pairs, within root,
    /// within 1% in). A change here means the fixture or the walk changed, so later results are not comparable.
    fn checks(rich: bool) -> (usize, u64, usize, usize) {
        let fx = generate(1, 1000, 3, rich);
        let g = build(&fx);
        let ids: Vec<NodeIx> = g.nodes().map(NodeRef::ix).collect();
        let p = picks(1, &ids);
        let mut s = Scratch::new(&g);
        (
            walk_total(&g, &mut s, &p),
            hops_sum(&g, &mut s, &p),
            g.within(g.node_at(ids[0]).id()).len(),
            g.within(g.node_at(ids[10]).id()).len(),
        )
    }

    /// s04 F4: `Graph::memory_bytes()` against the allocator when odd times and edge fields are common (the bench
    /// fixtures have none), so its map estimates are held to the same 5% band.
    #[test]
    fn memory_bytes_matches_the_allocator_with_odd_times_and_edge_fields() {
        let fx = generate(1, 1000, 3, true);
        let odd = [
            "2026-11-09T00:00:00.5Z",
            "",
            "not \"time\"",
            "2026-11-09T10:00:00+10:00",
        ];
        let before = alloc::thread_net();
        let mut nodes = codec::decode_nodes(&fx.nodes).unwrap().records;
        let mut edges = codec::decode_edges(&fx.edges).unwrap().records;
        for (i, n) in nodes.iter_mut().enumerate().filter(|(i, _)| i % 2 == 0) {
            n.created = odd[i % 4].into();
            n.updated = odd[i / 2 % 4].into();
        }
        for (i, e) in edges.iter_mut().enumerate() {
            e.fields.insert("confidence".into(), json!(0.5));
            if i % 3 == 0 {
                e.fields.insert("via".into(), json!(format!("merge {i}")));
                e.created = odd[i % 4].into();
            }
        }
        let g = Graph::from_records(nodes, edges);
        let counted = (alloc::thread_net() - before) as f64;
        let parts = g.memory_bytes();
        let ratio = parts.total() as f64 / counted;
        assert!(
            (ratio - 1.0).abs() <= 0.05,
            "memory_bytes {parts:?} total {} against {counted} counted: {ratio}",
            parts.total()
        );
    }

    #[test]
    fn check_values_at_1k_are_pinned() {
        assert_eq!(
            [checks(false), checks(true)],
            [(207_022, 811, 999, 113), (201_744, 837, 999, 243)]
        );
    }
}

//! s20 and s25 (plan 158, B3): graph.json's format and the own-graph check come from the end columns and the kept
//! lines, never a scan of edges.jsonl. Checked here against the old way, kept as the oracle: the scan of every line
//! on a write ([`scan_full_edges`]), and on load the old order (normalize every line, then load).

use super::graph::{OwnEnds, load_files, normalize_own_ends};
use super::{encode_full_edges, scan_full_edges};
use crate::problems;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use umx_core::codec::{self, GraphFile};
use umx_core::ontology::TextFields;
use umx_core::{Edge, GraphMeta, Id, Node, Problem, Value};
use umx_graph::{EdgesLoad, Graph, MemoryBytes, NodesLoad};

const SYMMETRIC: &[&str] = &["related_to"];
const KINDS: [&str; 3] = ["supports", "related_to", "blocks"];

/// SplitMix64.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn one_in(&mut self, n: usize) -> bool {
        self.below(n) == 0
    }
}

fn id(n: u128) -> Id {
    Id((0x0194_0000_0000_u128 << 80) | n)
}

fn meta() -> GraphMeta {
    codec::decode_graph(include_bytes!(
        "../../tests/fixtures/own-graph-end/graph.json"
    ))
    .unwrap()
}

/// Local nodes 1 to 6; another graph is 90 or 91.
fn nodes() -> Vec<u8> {
    let node = |n: u128| Node {
        id: id(n),
        kind: "problem".into(),
        title: "title".into(),
        status: "open".into(),
        note: String::new(),
        evidence: Vec::new(),
        status_history: Vec::new(),
        created: "2026-11-10T01:00:00Z".into(),
        updated: "2026-11-10T01:00:00Z".into(),
        created_by: "tester".into(),
        locator: None,
        authored_by_session: None,
        edited_by_session: None,
        fields: BTreeMap::new(),
    };
    codec::encode_nodes((1..=6).map(node).collect::<Vec<_>>().iter())
}

/// An end's graph: mostly local, else another graph, now and then this one.
fn end_graph(rng: &mut Rng, owner: Id, own: bool) -> Option<Id> {
    match rng.below(10) {
        0..=5 => None,
        6 | 7 => Some(id(90)),
        8 => Some(id(91)),
        _ if own => Some(owner),
        _ => None,
    }
}

fn random_edge(rng: &mut Rng, n: u128, owner: Id, own: bool, missing: bool) -> Edge {
    let local = |rng: &mut Rng| id(1 + rng.below(if missing { 8 } else { 6 }) as u128);
    let mut fields = BTreeMap::new();
    match rng.below(6) {
        0 => {
            fields.insert("x_graph".into(), Value::String("plain".into()));
        }
        1 => {
            fields.insert("note".into(), Value::String("a \\ backslash".into()));
        }
        _ => {}
    }
    Edge {
        id: id(100 + n),
        kind: KINDS[rng.below(KINDS.len())].into(),
        from_graph: end_graph(rng, owner, own),
        from: local(rng),
        to_graph: end_graph(rng, owner, own),
        to: local(rng),
        created: "2026-11-10T03:00:00Z".into(),
        created_by: "tester".into(),
        authored_by_session: None,
        edited_by_session: None,
        evidence: Vec::new(),
        fields,
    }
}

fn line(edge: &Edge) -> Vec<u8> {
    codec::encode_edges([edge])
}

fn replace(bytes: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    match bytes.windows(from.len()).position(|w| w == from) {
        Some(at) => [&bytes[..at], to, &bytes[at + from.len()..]].concat(),
        None => bytes.to_vec(),
    }
}

/// An edges.jsonl with repeats, steps back, dangling ends, symmetric reversed pairs, own-graph ends, escaped keys
/// and now and then a line the codec refuses.
fn edges_file(rng: &mut Rng, owner: Id) -> Vec<u8> {
    let mut out = Vec::new();
    let count = 1 + rng.below(9);
    let mut made: Vec<Edge> = Vec::new();
    for i in 0..count {
        let n = if rng.one_in(4) && i > 0 {
            rng.below(i) as u128
        } else {
            i as u128
        };
        let mut edge = if rng.one_in(5) && !made.is_empty() {
            // The reverse of an earlier edge: a symmetric pair when both ends are local.
            let earlier = &made[rng.below(made.len())];
            let mut e = earlier.clone();
            e.id = id(100 + 20 + i as u128);
            e.kind = "related_to".into();
            (e.from, e.to) = (earlier.to, earlier.from);
            (e.from_graph, e.to_graph) = (earlier.to_graph, earlier.from_graph);
            e
        } else {
            random_edge(rng, n, owner, true, true)
        };
        if rng.one_in(3) && !made.is_empty() {
            edge.kind = "related_to".into();
        }
        let mut bytes = line(&edge);
        match rng.below(14) {
            0 => bytes = replace(&bytes, b"\"from_graph\"", b"\"from_\\u0067raph\""),
            1 => bytes = replace(&bytes, b"\"to_graph\"", b"\"to_\\u0067raph\""),
            2 if edge.from_graph.is_some() => {
                // A null graph: the codec refuses it.
                let at = bytes
                    .windows(13)
                    .position(|w| w == b"\"from_graph\":")
                    .unwrap();
                bytes = [
                    &bytes[..at],
                    b"\"from_graph\":null,\"x\":",
                    &bytes[at + 13..],
                ]
                .concat();
            }
            3 => bytes = b"{\"id\":\"bad \\ line\"}\n".to_vec(),
            _ => {}
        }
        out.extend(bytes);
        made.push(edge);
    }
    out
}

/// What a load leaves, for comparing: the graph's bytes and memory, both reports and the own ends.
type Loaded = (Vec<u8>, Vec<u8>, MemoryBytes, NodesLoad, EdgesLoad, OwnEnds);

fn summary(r: Result<(Graph, NodesLoad, EdgesLoad, OwnEnds), Problem>) -> Result<Loaded, Problem> {
    r.map(|(g, n, e, o)| {
        (
            g.encode_nodes(),
            g.encode_edges(),
            g.memory_bytes(),
            n,
            e,
            o,
        )
    })
}

fn read(nodes: Vec<u8>, edges: Vec<u8>) -> impl Fn(GraphFile) -> Result<Option<Vec<u8>>, Problem> {
    move |file| {
        Ok(Some(match file {
            GraphFile::Nodes => nodes.clone(),
            _ => edges.clone(),
        }))
    }
}

/// The load as it was before s20: every line scanned and own ends normalized, then the edges loaded.
fn old_load(
    read: impl Fn(GraphFile) -> Result<Option<Vec<u8>>, Problem>,
    owner: Id,
) -> Result<(Graph, NodesLoad, EdgesLoad, OwnEnds), Problem> {
    let node_bytes = read(GraphFile::Nodes)?.unwrap_or_default();
    let (mut graph, nodes) =
        Graph::load_nodes_indexing_for_graph(node_bytes, TextFields::default(), owner).unwrap();
    let edge_bytes = read(GraphFile::Edges)?.unwrap_or_default();
    let (edge_bytes, own_ends) = normalize_own_ends(edge_bytes, owner)?;
    let mut edges = graph
        .load_edges(&edge_bytes)
        .map_err(|e| problems::decode_failed(&e, edge_bytes.len() as u64))?;
    let reversed = super::external::reversed_pairs(&graph, owner, SYMMETRIC);
    if !reversed.is_empty() {
        let lines = umx_graph::edge_lines(&edge_bytes, &reversed)
            .map_err(|e| problems::decode_failed(&e, edge_bytes.len() as u64))?;
        edges.reversed.extend(graph.keep_reversed(&reversed, lines));
    }
    Ok((graph, nodes, edges, own_ends))
}

fn load(
    nodes: &[u8],
    edges: &[u8],
    owner: Id,
) -> Result<(Graph, NodesLoad, EdgesLoad, OwnEnds), Problem> {
    load_files(
        read(nodes.to_vec(), edges.to_vec()),
        TextFields::default(),
        SYMMETRIC,
        owner,
    )
}

#[derive(Default)]
struct Seen {
    own: usize,
    failed: usize,
    reversed: usize,
    dangling: usize,
    repeats: usize,
    unsorted: usize,
    own_with_kept: usize,
}

/// graph-storage condition 1: on every file, the load without the scan reports what the old order reports, and a
/// fallback leaves nothing of its first pass (the graph's memory included).
#[test]
fn loading_without_the_scan_matches_the_old_order_on_every_file() {
    let owner = meta().id;
    let nodes = nodes();
    let mut seen = Seen::default();
    for seed in 0..3000 {
        let mut rng = Rng(seed);
        let edges = edges_file(&mut rng, owner);
        let old = summary(old_load(read(nodes.clone(), edges.clone()), owner));
        let new = summary(load(&nodes, &edges, owner));
        assert_eq!(new, old, "seed {seed}: {}", String::from_utf8_lossy(&edges));
        match &old {
            Err(_) => seen.failed += 1,
            Ok((.., e, own)) => {
                seen.own += usize::from(!own.is_empty());
                seen.reversed += usize::from(!e.reversed.is_empty());
                seen.dangling += usize::from(!e.dangling.is_empty());
                seen.repeats += usize::from(!e.repeats.is_empty());
                seen.unsorted += usize::from(e.unsorted.is_some());
                seen.own_with_kept += usize::from(
                    !own.is_empty()
                        && (!e.dangling.is_empty()
                            || !e.repeats.is_empty()
                            || !e.reversed.is_empty()),
                );
            }
        }
    }
    for (what, n) in [
        ("own ends", seen.own),
        ("failed loads", seen.failed),
        ("reversed pairs", seen.reversed),
        ("dangling ends", seen.dangling),
        ("repeats", seen.repeats),
        ("out of order", seen.unsorted),
        ("own ends beside kept lines", seen.own_with_kept),
    ] {
        assert!(n >= 20, "the files must cover {what}: {n}");
    }
}

type Encoded = Result<(Vec<u8>, Vec<u8>, u32), Problem>;

fn both(meta: &GraphMeta, g: &Graph) -> (Encoded, Encoded) {
    let new = encode_full_edges(meta, g).map(|e| (e.edges, e.metadata, e.umx));
    let old = scan_full_edges(meta, g.encode_edges()).map(|e| (e.edges, e.metadata, e.umx));
    (new, old)
}

/// The s20 write differential: after random batches of adds, edits and deletes with local, external and own-graph
/// ends, each prepared then committed or rolled back (a dry run), the column-built edges.jsonl, graph.json and
/// format equal the scan's, or both fail alike.
#[test]
fn edge_writes_without_the_scan_match_the_scan_after_random_batches() {
    let base = meta();
    let owner = base.id;
    let nodes = nodes();
    let (mut raised, mut kept_external, mut failed, mut cases) = (0, 0, 0, 0);
    for seed in 0..400 {
        let mut rng = Rng(seed ^ 0x5eed);
        let meta = GraphMeta {
            umx: 1 + rng.below(3) as u32,
            ..base.clone()
        };
        let edges = edges_file(&mut rng, owner);
        let Ok((mut g, ..)) = load(&nodes, &edges, owner) else {
            continue;
        };
        kept_external += usize::from(
            g.kept_edge_lines()
                .any(|l| l.windows(9).any(|w| w == b"_graph\":\"")),
        );
        let mut next = 200;
        for _ in 0..1 + rng.below(4) {
            let mut batch = g.batch();
            let mut removed: Vec<Id> = Vec::new();
            for _ in 0..1 + rng.below(4) {
                let live: Vec<Id> = batch
                    .graph()
                    .edges()
                    .map(|e| e.id())
                    .filter(|id| !removed.contains(id))
                    .collect();
                if !live.is_empty() && rng.one_in(3) {
                    let id = live[rng.below(live.len())];
                    batch.remove_edge(id);
                    removed.push(id);
                    continue;
                }
                let own = rng.one_in(8);
                let mut edge = random_edge(&mut rng, next, owner, own, false);
                next += 1;
                if !live.is_empty() && rng.one_in(2) {
                    // An edit in place of a live edge.
                    edge.id = live[rng.below(live.len())];
                }
                batch.insert_edge(edge);
            }
            if batch.prepare().is_err() {
                batch.rollback();
                continue;
            }
            let (new, old) = both(&meta, batch.graph());
            assert_eq!(new, old, "seed {seed}, prepared");
            cases += 1;
            match &old {
                Ok((.., umx)) => raised += usize::from(*umx > meta.umx),
                Err(_) => failed += 1,
            }
            if rng.one_in(2) {
                batch.rollback();
            } else {
                batch.commit();
            }
            let (new, old) = both(&meta, &g);
            assert_eq!(new, old, "seed {seed}, after the batch");
            cases += 1;
        }
    }
    assert!(cases >= 1000, "at least 1,000 cases: {cases}");
    assert!(raised >= 20, "writes that raise the format: {raised}");
    assert!(
        failed >= 20,
        "writes refused for an own-graph end: {failed}"
    );
    assert!(
        kept_external >= 20,
        "graphs with an external kept line: {kept_external}"
    );
}

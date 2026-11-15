//! ws004 s09 similar_to through the services (workshop 008 section 9; req-65) over the fakes: applied vectors write
//! `similar/` and `_meta.tsv`; a load reads them, or rebuilds when they no longer match; `umx similarity rebuild`
//! reproduces the files byte for byte; a secured graph keeps them in the local cache; nothing crosses graphs.

use std::collections::BTreeMap;
use std::sync::Arc;
use umx_core::ports::{Change, FileRef, GraphDir, GraphFiles, Locality, ModelKey};
use umx_core::setup::GraphFilesSetup;
use umx_core::similar::{self, SimilarShardLine};
use umx_core::vectors::{ShardKind, TextHash};
use umx_core::{GraphEmbedding, Id, Policy};
use umx_fakes::{FakeClock, FakeEmbedder, FakeGraphFiles, FakeIds, FakeLocalState};
use umx_services::{
    Ctx, Direction, NeighbourRow, NeighboursRequest, NodeAddRequest, NodeEditRequest, Pacer,
    PathRequest, Planned, Ports, Services, Step, WalkRequest, Way, batches, step,
};

fn key() -> ModelKey {
    ModelKey::new("fake", 8).unwrap()
}

fn ctx() -> Ctx {
    Ctx::new("tester")
}

fn services(files: &Arc<FakeGraphFiles>, embedder: &Arc<FakeEmbedder>) -> Services {
    let ports = Ports {
        sources: std::sync::Arc::new(umx_fakes::FakeSourceReader::open(
            umx_core::ports::GraphDir::parse("/").unwrap(),
        )),
        files: files.clone(),
        vector_cache: files.clone(),
        vector_cache_root: GraphDir::parse("/work/cache").unwrap(),
        clock: Arc::new(FakeClock::default()),
        ids: Arc::new(FakeIds::new()),
        state: Arc::new(FakeLocalState::new()),
        scopes: Arc::new(FakeLocalState::new()),
        registry: Arc::new(FakeLocalState::new()),
        allows: Arc::new(FakeLocalState::new()),
        lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
    };
    Services::new(ports).with_embedder(embedder.clone())
}

struct World {
    files: Arc<FakeGraphFiles>,
    embedder: Arc<FakeEmbedder>,
    svc: Services,
}

fn world_at(
    path: &str,
    alias: &str,
    policy: Policy,
    similarity: Option<serde_json::Value>,
) -> World {
    let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let embedder = Arc::new(FakeEmbedder::new(key()).with_locality(Locality::Local));
    let mut w = World {
        svc: services(&files, &embedder),
        files,
        embedder,
    };
    let mut embedding = GraphEmbedding::new(Some(key()), policy);
    embedding.similarity = similarity;
    w.svc
        .graph_new(&ctx(), path, alias, None, false, Some(embedding))
        .unwrap();
    w
}

fn world() -> World {
    world_at("/work/g", "demo", Policy::OPEN, None)
}

fn add(w: &mut World, title: &str) -> Id {
    w.svc
        .node_add(
            &ctx(),
            NodeAddRequest {
                kind: "problem".into(),
                title: title.into(),
                note: Some(format!("About {title}.")),
                ..Default::default()
            },
        )
        .unwrap()
        .value
        .node
        .id
}

fn run(w: &mut World) {
    drive(w, false);
}

/// [`run`], with the lists' files deferred as the daemon's embed backlog does (ws004 s25a): memory moves, files are
/// owed until [`flush`].
fn run_deferred(w: &mut World) {
    drive(w, true);
}

fn drive(w: &mut World, deferred: bool) {
    let mut pacer = Pacer::default();
    loop {
        let jobs = w.svc.take_embed_jobs();
        if jobs.is_empty() {
            return;
        }
        let planned: Vec<Planned> = jobs
            .iter()
            .filter_map(|j| w.svc.embed_plan(j).unwrap())
            .collect();
        for batch in batches(planned) {
            let Step::Done { vectors, .. } = step(&*w.embedder, &batch, &mut pacer, 0) else {
                panic!("the fake embeds");
            };
            if deferred {
                w.svc.embed_apply_deferred(&vectors).unwrap();
            } else {
                w.svc.embed_apply(&vectors).unwrap();
            }
        }
    }
}

/// Writes every owed similar_to file, a step (one file) at a time; returns how many steps it took.
fn flush(w: &mut World) -> usize {
    let mut steps = 0;
    while let Some(problems) = w.svc.flush_similar_step() {
        assert!(problems.is_empty(), "{problems:?}");
        steps += 1;
    }
    steps
}

/// Every similar_to file under `dir`: each shard's bytes by name, and `_meta.tsv`.
fn similar_files(files: &FakeGraphFiles, dir: &GraphDir) -> BTreeMap<String, Vec<u8>> {
    let mut out: BTreeMap<String, Vec<u8>> = files
        .list_shards(dir, &key(), ShardKind::Similar)
        .unwrap()
        .into_iter()
        .map(|s| {
            let bytes = files
                .read_ref(dir, &FileRef::Shard(s.clone()))
                .unwrap()
                .unwrap();
            (s.to_string(), bytes)
        })
        .collect();
    if let Some(meta) = files.read_ref(dir, &FileRef::SimilarMeta(key())).unwrap() {
        out.insert("_meta.tsv".into(), meta);
    }
    out
}

fn dir() -> GraphDir {
    GraphDir::parse("/work/g").unwrap()
}

/// Every (node, other, score) line across the files.
fn edges(files: &BTreeMap<String, Vec<u8>>) -> Vec<(Id, Id, i32)> {
    files
        .iter()
        .filter(|(name, _)| name.as_str() != "_meta.tsv")
        .flat_map(|(_, bytes)| similar::decode_shard(bytes))
        .filter_map(|l| match l {
            SimilarShardLine::Edge(e) => Some((e.node, e.other, e.score)),
            SimilarShardLine::Kept(_) => None,
        })
        .collect()
}

fn reload(w: &mut World) -> Vec<&'static str> {
    let mut fresh = services(&w.files, &w.embedder);
    let done = fresh.graph_load(&ctx(), "/work/g", None).unwrap();
    w.svc = fresh;
    done.notes.iter().map(|p| p.code).collect()
}

#[test]
fn committed_membership_flips_refresh_lists_without_changing_vector_files() {
    for batch in [false, true] {
        let mut w = world();
        let hidden = add(&mut w, "shared cat food");
        add(&mut w, "shared cat food");
        run(&mut w);
        let mut records: Vec<_> = w
            .svc
            .loaded_graph("demo")
            .unwrap()
            .nodes()
            .map(|n| n.to_node())
            .collect();
        records
            .iter_mut()
            .find(|n| n.id == hidden)
            .unwrap()
            .fields
            .insert("prov_key".into(), serde_json::json!("agent/pij/pij-seat"));
        w.files
            .write_set(
                &dir(),
                &[Change {
                    file: FileRef::Graph(umx_core::codec::GraphFile::Nodes),
                    bytes: Some(&umx_core::codec::encode_nodes(&records)),
                }],
            )
            .unwrap();
        reload(&mut w);
        assert!(
            edges(&similar_files(&w.files, &dir()))
                .iter()
                .any(|(a, b, _)| { *a == hidden || *b == hidden })
        );
        let vectors: Vec<_> = w
            .files
            .list_shards(&dir(), &key(), ShardKind::Vectors)
            .unwrap()
            .into_iter()
            .map(|s| {
                let file = FileRef::Shard(s);
                let bytes = w.files.read_ref(&dir(), &file).unwrap().unwrap();
                (file, bytes)
            })
            .collect();
        let req = umx_services::NodeEditRequest {
            kind: Some("agent".into()),
            ..Default::default()
        };
        if batch {
            w.svc
                .batch(
                    &ctx(),
                    umx_services::BatchRequest {
                        raised_in: None,
                        reason: None,
                        echo_sentences: false,
                        lines: vec![umx_services::BatchLine {
                            line: 1,
                            op: umx_services::BatchOp::NodeEdit {
                                node: Some(hidden.to_text()),
                                node_ref: None,
                                req,
                            },
                        }],
                        dry_run: false,
                        refs: Default::default(),
                        refs_graph: None,
                    },
                )
                .unwrap();
        } else {
            w.svc.node_edit(&ctx(), &hidden.to_text(), req).unwrap();
        }
        let g = w.svc.loaded_graph("demo").unwrap();
        assert!(g.is_provenance(g.node_ix(hidden).unwrap()));
        for n in g.nodes() {
            assert!(
                g.similar_out(n.ix())
                    .iter()
                    .all(|e| g.node_at(e.other).id() != hidden)
            );
        }
        while let Some(warnings) = w.svc.flush_similar_step() {
            assert!(warnings.is_empty(), "{warnings:?}");
        }
        assert!(
            edges(&similar_files(&w.files, &dir()))
                .iter()
                .all(|(a, b, _)| { *a != hidden && *b != hidden })
        );
        let g = w.svc.loaded_graph("demo").unwrap();
        assert!(g.is_provenance(g.node_ix(hidden).unwrap()));
        for n in g.nodes() {
            assert!(
                g.similar_out(n.ix())
                    .iter()
                    .all(|e| g.node_at(e.other).id() != hidden)
            );
        }
        for (file, bytes) in vectors {
            assert_eq!(w.files.read_ref(&dir(), &file).unwrap(), Some(bytes));
        }
        assert!(w.svc.take_embed_jobs().iter().all(|j| j.node != hidden));
    }
}

/// The lists in memory, as (node, other, score), from the graph the services hold.
fn in_memory(w: &World) -> Vec<(Id, Id, i32)> {
    let g = &w.svc.loaded("demo").unwrap().graph;
    let mut out: Vec<(Id, Id, i32)> = g
        .nodes()
        .flat_map(|n| {
            g.similar_out(n.ix())
                .iter()
                .map(move |e| (n.id(), g.node_at(e.other).id(), e.score))
                .collect::<Vec<_>>()
        })
        .collect();
    out.sort();
    out
}

#[test]
fn hidden_provenance_preserves_nonempty_similarity_results_and_query_work() {
    use umx_core::codec::{self, GraphFile};
    use umx_services::CypherRequest;
    let mut w = world();
    let ids: Vec<_> = (0..6)
        .map(|i| add(&mut w, &format!("Problem {i}")))
        .collect();
    run(&mut w);
    let req = NeighboursRequest {
        node: ids[0].to_text(),
        relation: Some("similar_to".into()),
        direction: Direction::Out,
        limit: 100,
        offset: 0,
    };
    let hidden = w.svc.neighbours(&ctx(), req.clone()).unwrap();
    assert!(!hidden.value.rows.is_empty());
    assert_eq!(
        w.svc
            .neighbours(&ctx().with_provenance(true), req.clone())
            .unwrap(),
        hidden
    );
    let query = CypherRequest {
        text: "MATCH (a)-[r:similar_to]->(b) RETURN a.id, b.id, r.score".into(),
        params: vec![],
        limit: None,
        offset: None,
        along: None,
    };
    let queried = w.svc.query(&ctx(), &query).unwrap();
    assert!(!queried.value.rows.is_empty());
    let graph = w.svc.loaded_graph("demo").unwrap();
    let nodes: Vec<_> = graph
        .nodes()
        .filter(|n| !graph.is_provenance(n.ix()))
        .map(|n| n.to_node())
        .collect();
    let edges: Vec<_> = graph
        .edges()
        .filter(|e| {
            [e.from_ix(), e.to_ix()]
                .into_iter()
                .flatten()
                .all(|n| !graph.is_provenance(n))
        })
        .map(|e| e.to_edge())
        .collect();
    w.files
        .tamper_for_tests(&dir(), GraphFile::Nodes, &codec::encode_nodes(&nodes));
    w.files
        .tamper_for_tests(&dir(), GraphFile::Edges, &codec::encode_edges(&edges));
    reload(&mut w);
    assert_eq!(w.svc.neighbours(&ctx(), req).unwrap(), hidden);
    assert_eq!(w.svc.query(&ctx(), &query).unwrap(), queried);
}

#[test]
fn applied_vectors_write_similar_files_and_a_reload_reads_them_back() {
    let mut w = world();
    let ids: Vec<Id> = (0..6)
        .map(|i| add(&mut w, &format!("Problem {i}")))
        .collect();
    assert!(
        similar_files(&w.files, &dir()).is_empty(),
        "nothing before the vectors arrive"
    );
    run(&mut w);
    let files = similar_files(&w.files, &dir());
    let mut on_disk = edges(&files);
    on_disk.sort();
    assert_eq!(on_disk, in_memory(&w), "the files are what memory holds");
    // The fake model's floor is 0 and k is 10: every node lists the others it scores at or above 0.
    assert!(!on_disk.is_empty());
    assert!(
        on_disk
            .iter()
            .all(|(a, b, _)| a != b && ids.contains(a) && ids.contains(b))
    );
    let meta = String::from_utf8(files["_meta.tsv"].clone()).unwrap();
    assert!(
        meta.starts_with("1\tfake@8\t10\t0.0000\t@node.v2\t"),
        "{meta}"
    );
    // A teammate's clone: the stored lists load as they are, nothing is rebuilt.
    let before = in_memory(&w);
    assert_eq!(reload(&mut w), Vec::<&str>::new());
    assert!(w.svc.take_similarity_rebuilds().is_empty());
    assert!(!w.svc.similarity_stale("demo"));
    assert_eq!(in_memory(&w), before);
}

#[test]
fn similarity_rebuild_reproduces_the_files_byte_for_byte() {
    let mut w = world();
    for i in 0..8 {
        add(&mut w, &format!("Problem {i}"));
        // Vectors arrive node by node: the lists are kept current incrementally.
        run(&mut w);
    }
    let kept_current = similar_files(&w.files, &dir());
    assert!(kept_current.len() > 1);
    // Nothing to do: the files already match.
    assert_eq!(
        w.svc
            .similarity_rebuild(&ctx())
            .unwrap()
            .value
            .files_written,
        0
    );
    // Throw every similar_to file away and rebuild from the committed vectors.
    let removals: Vec<FileRef> = kept_current
        .keys()
        .map(|name| match name.as_str() {
            "_meta.tsv" => FileRef::SimilarMeta(key()),
            path => FileRef::parse(path).unwrap(),
        })
        .collect();
    let set: Vec<Change<'_>> = removals
        .iter()
        .map(|f| Change {
            file: f.clone(),
            bytes: None,
        })
        .collect();
    w.files.write_set(&dir(), &set).unwrap();
    let rebuilt = w.svc.similarity_rebuild(&ctx()).unwrap().value;
    assert_eq!(rebuilt.files_written, kept_current.len());
    assert_eq!((rebuilt.k, rebuilt.floor), (10, 0));
    assert_eq!(
        similar_files(&w.files, &dir()),
        kept_current,
        "byte for byte"
    );
}

#[test]
fn lists_that_no_longer_match_their_vectors_are_rebuilt_and_warned_meanwhile() {
    let mut w = world();
    for i in 0..5 {
        add(&mut w, &format!("Problem {i}"));
    }
    run(&mut w);
    let good = similar_files(&w.files, &dir());
    // A merge left _meta.tsv from another branch's vectors: the digest's last hex digit differs.
    let mut meta = good["_meta.tsv"].clone();
    let n = meta.iter().rposition(|&b| b == b'\t').unwrap() - 1;
    meta[n] = if meta[n] == b'0' { b'1' } else { b'0' };
    w.files
        .write_set(
            &dir(),
            &[Change {
                file: FileRef::SimilarMeta(key()),
                bytes: Some(&meta),
            }],
        )
        .unwrap();
    assert_eq!(reload(&mut w), ["similar_to_incomplete"]);
    assert!(w.svc.similarity_stale("demo"));
    assert_eq!(w.svc.take_similarity_rebuilds(), ["demo"]);
    assert!(in_memory(&w).is_empty(), "stale lists are never served");
    // The background rebuild, in three steps: snapshot (shared), compute (no lock), apply (writer).
    let job = w.svc.similarity_snapshot("demo").unwrap();
    let lists = job.compute();
    assert!(w.svc.similarity_apply("demo", lists).unwrap().is_some());
    assert!(!w.svc.similarity_stale("demo"));
    assert_eq!(similar_files(&w.files, &dir()), good);
    // A snapshot whose vectors moved before it was applied is refused, and a new one is taken.
    let job = w.svc.similarity_snapshot("demo").unwrap();
    add(&mut w, "Problem 9");
    run(&mut w);
    assert_eq!(w.svc.similarity_apply("demo", job.compute()).unwrap(), None);
}

#[test]
fn a_setting_out_of_range_warns_uses_the_default_and_stays_in_graph_json() {
    let mut w = world_at(
        "/work/g",
        "demo",
        Policy::OPEN,
        Some(serde_json::json!({"k": 999, "floor": 0.25})),
    );
    let warnings: Vec<_> = reload(&mut w);
    assert_eq!(warnings, ["similarity_setting_invalid"]);
    let g = &w.svc.loaded("demo").unwrap().graph;
    assert_eq!(
        g.similar_settings(),
        (10, 2500),
        "k falls back; the valid floor is used"
    );
    let graph_json = w
        .files
        .read_ref(&dir(), &FileRef::Graph(umx_core::codec::GraphFile::Graph))
        .unwrap()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&graph_json).unwrap();
    assert_eq!(
        v["embedding"]["similarity"],
        serde_json::json!({"k": 999, "floor": 0.25})
    );
    let text = String::from_utf8(graph_json).unwrap();
    let order =
        ["\"key\"", "\"text_form\"", "\"policy\"", "\"similarity\""].map(|k| text.find(k).unwrap());
    assert!(
        order.windows(2).all(|w| w[0] < w[1]),
        "key order inside embedding: {text}"
    );
}

#[test]
fn a_secured_graph_keeps_its_similar_files_in_the_local_cache() {
    let mut w = world_at("/work/g", "demo", Policy::SECURED, None);
    for i in 0..4 {
        add(&mut w, &format!("Problem {i}"));
    }
    run(&mut w);
    assert!(
        similar_files(&w.files, &dir()).is_empty(),
        "nothing in the graph folder"
    );
    let graph_id = w.svc.loaded("demo").unwrap().meta.id.to_text();
    let folder_key = TextHash::of("/work/g").to_string();
    let cache = GraphDir::parse(&format!("/work/cache/{graph_id}/{}", &folder_key[..16])).unwrap();
    let cached = similar_files(&w.files, &cache);
    assert!(cached.contains_key("_meta.tsv"));
    assert!(!edges(&cached).is_empty());
    assert_eq!(
        reload(&mut w),
        Vec::<&str>::new(),
        "the cache loads like the folder"
    );
}

#[test]
fn stored_lists_never_name_another_graphs_nodes() {
    let mut a = world_at("/work/a", "alpha", Policy::OPEN, None);
    // A second graph loaded in the same services, sharing the fake disk.
    a.svc
        .graph_new(
            &ctx(),
            "/work/b",
            "beta",
            None,
            false,
            Some(GraphEmbedding::new(Some(key()), Policy::OPEN)),
        )
        .unwrap();
    let mut alpha_ids = Vec::new();
    for i in 0..4 {
        alpha_ids.push(
            a.svc
                .node_add(
                    &Ctx::new("tester").graph("alpha"),
                    NodeAddRequest {
                        kind: "problem".into(),
                        title: format!("Shared title {i}"),
                        ..Default::default()
                    },
                )
                .unwrap()
                .value
                .node
                .id,
        );
        a.svc
            .node_add(
                &Ctx::new("tester").graph("beta"),
                NodeAddRequest {
                    kind: "problem".into(),
                    title: format!("Shared title {i}"),
                    ..Default::default()
                },
            )
            .unwrap();
    }
    run(&mut a);
    let on_disk = edges(&similar_files(
        &a.files,
        &GraphDir::parse("/work/a").unwrap(),
    ));
    assert!(!on_disk.is_empty());
    assert!(
        on_disk
            .iter()
            .all(|(n, o, _)| alpha_ids.contains(n) && alpha_ids.contains(o)),
        "alpha's files hold only alpha's nodes, though beta has nodes with the same text"
    );
}

fn neighbours(
    w: &World,
    node: Id,
    relation: Option<&str>,
) -> (Vec<NeighbourRow>, Vec<&'static str>) {
    let done = w
        .svc
        .neighbours(
            &ctx(),
            NeighboursRequest {
                node: node.to_text(),
                relation: relation.map(str::to_owned),
                direction: Direction::Both,
                limit: 1000,
                offset: 0,
            },
        )
        .unwrap();
    (done.value.rows, done.notes.iter().map(|p| p.code).collect())
}

#[test]
fn similar_to_is_followed_only_when_named_with_scores_and_no_ids() {
    let mut w = world();
    let ids: Vec<Id> = (0..5)
        .map(|i| add(&mut w, &format!("Problem {i}")))
        .collect();
    run(&mut w);
    w.svc
        .edge_add(
            &ctx(),
            &ids[0].to_text(),
            "supports",
            &ids[1].to_text(),
            &[],
        )
        .unwrap();
    // Untyped: the stored edge only.
    let (rows, warnings) = neighbours(&w, ids[0], None);
    assert!(warnings.is_empty());
    assert_eq!(
        rows.iter()
            .map(|r| r.edge.relation.as_str())
            .collect::<Vec<_>>(),
        ["supports"]
    );
    // Named: the node's list (out, best first), then who lists it (in), every one derived.
    let (rows, warnings) = neighbours(&w, ids[0], Some("similar_to"));
    assert!(warnings.is_empty(), "{warnings:?}");
    let g = &w.svc.loaded("demo").unwrap().graph;
    let ix = g.node_ix(ids[0]).unwrap();
    let expected: Vec<(Way, Id, i32)> = g
        .similar_out(ix)
        .iter()
        .map(|e| (Way::Out, g.node_at(e.other).id(), e.score))
        .chain(
            g.similar_in(ix)
                .iter()
                .map(|e| (Way::In, g.node_at(e.other).id(), e.score)),
        )
        .collect();
    assert!(!expected.is_empty());
    let got: Vec<(Way, Id, i32)> = rows
        .iter()
        .map(|r| (r.edge.way, r.node.id, r.edge.score.unwrap()))
        .collect();
    assert_eq!(got, expected);
    assert!(
        rows.iter()
            .all(|r| r.edge.derived && r.edge.id.is_none() && r.edge.relation == "similar_to")
    );
    // Walks and paths along similar_to stop at 2 steps.
    let walk = |depth| {
        w.svc.walk(
            &ctx(),
            WalkRequest {
                node: ids[0].to_text(),
                depth,
                relation: Some("similar_to".into()),
                direction: Direction::Out,
                within: None,
                limit: 100,
                offset: 0,
            },
        )
    };
    assert!(walk(2).is_ok());
    assert_eq!(walk(3).unwrap_err().code, "usage_invalid");
    let path = w.svc.path(
        &ctx(),
        PathRequest {
            from: ids[0].to_text(),
            to: ids[4].to_text(),
            max_depth: 3,
            relation: Some("similar_to".into()),
            direction: Direction::Both,
        },
    );
    assert_eq!(path.unwrap_err().code, "usage_invalid");
    // similar_to is umx's own: never added by hand.
    let refused = w
        .svc
        .edge_add(
            &ctx(),
            &ids[0].to_text(),
            "similar_to",
            &ids[1].to_text(),
            &[],
        )
        .unwrap_err();
    assert_eq!(refused.code, "name_invalid");
    assert_eq!(
        w.svc
            .ontology_add_relation(&ctx(), "similar_to", false, None)
            .unwrap_err()
            .code,
        "name_invalid"
    );
    assert_eq!(
        w.svc
            .ontology_add_relation(&ctx(), "similar_to", true, None)
            .unwrap_err()
            .code,
        "symmetric_refused",
        "--symmetric gets #101's answer, which names what was asked"
    );
}

#[test]
fn an_older_graphs_stored_similar_to_edge_is_listed_apart_from_the_lists() {
    let mut w = world();
    let ids: Vec<Id> = (0..3)
        .map(|i| add(&mut w, &format!("Problem {i}")))
        .collect();
    run(&mut w);
    // Written before the name was reserved: a real edge record named similar_to.
    let edge = umx_core::Edge {
        id: Id(ids[2].0 + 1000),
        kind: "similar_to".into(),
        from_graph: None,
        from: ids[0],
        to_graph: None,
        to: ids[2],
        created: "2026-11-10T01:00:00Z".into(),
        created_by: "older-umx".into(),
        authored_by_session: None,
        edited_by_session: None,
        evidence: Vec::new(),
        fields: Default::default(),
    };
    let bytes = umx_core::codec::encode_edges([&edge]);
    w.files
        .write_set(
            &dir(),
            &[Change {
                file: FileRef::Graph(umx_core::codec::GraphFile::Edges),
                bytes: Some(&bytes),
            }],
        )
        .unwrap();
    reload(&mut w);
    let (rows, _) = neighbours(&w, ids[0], Some("similar_to"));
    let stored: Vec<&NeighbourRow> = rows.iter().filter(|r| !r.edge.derived).collect();
    assert_eq!(stored.len(), 1);
    assert_eq!(
        (stored[0].edge.id, stored[0].node.id, stored[0].edge.score),
        (Some(edge.id), ids[2], None)
    );
    let last = rows.last().unwrap();
    assert!(
        !last.edge.derived,
        "stored records come after the lists, never merged into them"
    );
    let (untyped, _) = neighbours(&w, ids[0], None);
    assert!(
        untyped.is_empty(),
        "an untyped step never follows similar_to, stored or not"
    );
}

#[test]
fn a_node_with_no_vector_yet_makes_a_similar_to_read_incomplete() {
    let mut w = world();
    let first = add(&mut w, "Problem 0");
    run(&mut w);
    let pending = add(&mut w, "Problem 1");
    let (rows, warnings) = neighbours(&w, pending, Some("similar_to"));
    assert!(rows.is_empty());
    assert_eq!(warnings, ["similar_to_incomplete"]);
    let (_, warnings) = neighbours(&w, first, Some("similar_to"));
    assert!(warnings.is_empty());
}

/// Rewrites one graph file in place, as a checkout or a hand edit would.
fn put_file(w: &World, dir: &GraphDir, file: FileRef, bytes: Option<&[u8]>) {
    w.files.write_set(dir, &[Change { file, bytes }]).unwrap();
}

/// R102-3: `_meta.tsv` matching the vectors does not vouch for shards that did not survive. A missing shard, or a
/// line that is damaged, leaves fewer edges than it counts: stale, warned and rebuilt, never served as complete.
#[test]
fn a_missing_shard_or_a_damaged_line_makes_the_lists_stale() {
    let mut w = world_at(
        "/work/g",
        "demo",
        Policy::OPEN,
        Some(serde_json::json!({"floor": -1})),
    );
    for i in 0..6 {
        add(&mut w, &format!("Problem {i}"));
    }
    run(&mut w);
    let good = similar_files(&w.files, &dir());
    let shards = w
        .files
        .list_shards(&dir(), &key(), ShardKind::Similar)
        .unwrap();
    assert!(shards.len() > 1, "a fixture with more than one shard");
    assert_eq!(
        reload(&mut w),
        Vec::<&str>::new(),
        "positive control: intact files load fresh"
    );

    put_file(&w, &dir(), FileRef::Shard(shards[0].clone()), None);
    assert_eq!(
        reload(&mut w),
        ["similar_to_incomplete"],
        "a shard is missing"
    );
    assert!(w.svc.similarity_stale("demo"));
    assert_eq!(w.svc.take_similarity_rebuilds(), ["demo"]);
    assert!(in_memory(&w).is_empty(), "a partial result is never served");
    w.svc.similarity_rebuild(&ctx()).unwrap();
    assert_eq!(
        similar_files(&w.files, &dir()),
        good,
        "the rebuild writes it back"
    );

    let name = shards[1].to_string();
    let damaged = String::from_utf8(good[&name].clone())
        .unwrap()
        .replacen('.', ",", 1);
    put_file(
        &w,
        &dir(),
        FileRef::Shard(shards[1].clone()),
        Some(damaged.as_bytes()),
    );
    assert_eq!(
        reload(&mut w),
        ["similar_to_incomplete"],
        "a line is damaged"
    );
    w.svc.similarity_rebuild(&ctx()).unwrap();
    assert_eq!(
        reload(&mut w),
        Vec::<&str>::new(),
        "rebuilt once, fresh after: no rebuild on every load"
    );
    assert_eq!(
        edges(&similar_files(&w.files, &dir())).len(),
        edges(&good).len()
    );
}

/// R102-1: a rebuild computed from one load of a graph is never applied to a later load of the same folder, whose
/// vector generation may well be the same number: not when its settings changed, and not (without a panic) when it
/// has fewer nodes.
#[test]
fn a_rebuild_from_before_a_reload_is_refused() {
    let mut w = world();
    let mut ids = Vec::new();
    for i in 0..3 {
        ids.push(add(&mut w, &format!("Problem {i}")));
    }
    run(&mut w);
    w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
    let job = w.svc.similarity_snapshot("demo").unwrap();
    let raw = w
        .files
        .read(&dir(), umx_core::codec::GraphFile::Graph)
        .unwrap()
        .unwrap();
    let mut meta: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    meta["embedding"]["similarity"] = serde_json::json!({"k": 1});
    w.files
        .write(
            &dir(),
            umx_core::codec::GraphFile::Graph,
            &serde_json::to_vec(&meta).unwrap(),
        )
        .unwrap();
    w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
    assert_eq!(
        w.svc.similarity_info("demo").unwrap().1,
        1,
        "the reload took k = 1"
    );
    assert_eq!(w.svc.similarity_apply("demo", job.compute()).unwrap(), None);
    assert_eq!(w.svc.similarity_info("demo").unwrap().1, 1, "k stays 1");

    let job = w.svc.similarity_snapshot("demo").unwrap();
    let nodes = w
        .files
        .read(&dir(), umx_core::codec::GraphFile::Nodes)
        .unwrap()
        .unwrap();
    let fewer: Vec<u8> = nodes
        .split_inclusive(|&b| b == b'\n')
        .filter(|line| {
            serde_json::from_slice::<serde_json::Value>(line).unwrap()["id"]
                != serde_json::json!(ids[2])
        })
        .flatten()
        .copied()
        .collect();
    w.files
        .write(&dir(), umx_core::codec::GraphFile::Nodes, &fewer)
        .unwrap();
    w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
    assert_eq!(w.svc.similarity_apply("demo", job.compute()).unwrap(), None);
    let job = w.svc.similarity_snapshot("demo").unwrap();
    assert!(
        w.svc
            .similarity_apply("demo", job.compute())
            .unwrap()
            .is_some(),
        "positive control: a rebuild of this load applies"
    );

    // Same nodes and settings, other vectors: only the load tells the two apart.
    let job = w.svc.similarity_snapshot("demo").unwrap();
    for shard in w
        .files
        .list_shards(&dir(), &key(), ShardKind::Vectors)
        .unwrap()
    {
        put_file(&w, &dir(), FileRef::Shard(shard), None);
    }
    w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
    assert_eq!(w.svc.similarity_apply("demo", job.compute()).unwrap(), None);
    assert!(
        in_memory(&w).is_empty(),
        "no lists from vectors this load does not have"
    );
}

/// Fails the next similar_to write set, once.
struct FailSimilar {
    inner: Arc<FakeGraphFiles>,
    armed: std::sync::atomic::AtomicBool,
}

impl GraphFiles for FailSimilar {
    fn relocate(
        &self,
        from: &umx_core::ports::GraphDir,
        to: &umx_core::ports::GraphDir,
    ) -> Result<(), umx_core::ports::FilesError> {
        self.inner.relocate(from, to)
    }
    fn read_ref(
        &self,
        d: &GraphDir,
        f: &FileRef,
    ) -> Result<Option<Vec<u8>>, umx_core::ports::FilesError> {
        self.inner.read_ref(d, f)
    }
    fn write_set_checked(
        &self,
        d: &GraphDir,
        c: &[Change<'_>],
        expect: Option<&umx_core::ports::GraphStamps>,
    ) -> Result<umx_core::ports::Committed, umx_core::ports::FilesError> {
        let similar = c.iter().any(|c| matches!(c.file, FileRef::SimilarMeta(_)));
        if similar && self.armed.swap(false, std::sync::atomic::Ordering::SeqCst) {
            return Err(umx_core::ports::FilesError::io(
                "test: similar write failed",
            ));
        }
        self.inner.write_set_checked(d, c, expect)
    }
    fn read_stamped(
        &self,
        d: &GraphDir,
        f: umx_core::codec::GraphFile,
    ) -> Result<(Option<Vec<u8>>, umx_core::ports::FileStamp), umx_core::ports::FilesError> {
        self.inner.read_stamped(d, f)
    }
    fn stamps(
        &self,
        d: &GraphDir,
    ) -> Result<umx_core::ports::GraphStamps, umx_core::ports::FilesError> {
        self.inner.stamps(d)
    }
    fn list(
        &self,
        d: &GraphDir,
    ) -> Result<Vec<umx_core::codec::GraphFile>, umx_core::ports::FilesError> {
        self.inner.list(d)
    }
    fn resolve(&self, d: &GraphDir) -> Result<GraphDir, umx_core::ports::FilesError> {
        self.inner.resolve(d)
    }
    fn list_shards(
        &self,
        d: &GraphDir,
        k: &ModelKey,
        kind: ShardKind,
    ) -> Result<Vec<umx_core::vectors::ShardFile>, umx_core::ports::FilesError> {
        self.inner.list_shards(d, k, kind)
    }
    fn list_vector_spaces(
        &self,
        d: &GraphDir,
    ) -> Result<Vec<ModelKey>, umx_core::ports::FilesError> {
        self.inner.list_vector_spaces(d)
    }
    fn find_graphs(
        &self,
        root: &GraphDir,
        limits: umx_core::ports::FindLimits,
    ) -> Result<umx_core::ports::Found, umx_core::ports::FilesError> {
        self.inner.find_graphs(root, limits)
    }
    fn peek_graph(
        &self,
        dir: &GraphDir,
    ) -> Result<Option<umx_core::ports::FoundGraph>, umx_core::ports::FilesError> {
        self.inner.peek_graph(dir)
    }
    fn adapter_id(&self) -> &'static str {
        "test.fail-similar"
    }
}

/// R102-2: a similar_to write that fails leaves the lists owed in full. Retrying the same vectors changes no list,
/// yet writes every missing one with `_meta.tsv`, never `_meta.tsv` alone over files that lack them.
#[test]
fn a_failed_similar_write_is_owed_in_full() {
    let inner = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let files = Arc::new(FailSimilar {
        inner: inner.clone(),
        armed: false.into(),
    });
    let embedder = Arc::new(FakeEmbedder::new(key()).with_locality(Locality::Local));
    let ports = Ports {
        sources: std::sync::Arc::new(umx_fakes::FakeSourceReader::open(
            umx_core::ports::GraphDir::parse("/").unwrap(),
        )),
        files: files.clone(),
        vector_cache: files.clone(),
        vector_cache_root: GraphDir::parse("/work/cache").unwrap(),
        clock: Arc::new(FakeClock::default()),
        ids: Arc::new(FakeIds::new()),
        state: Arc::new(FakeLocalState::new()),
        scopes: Arc::new(FakeLocalState::new()),
        registry: Arc::new(FakeLocalState::new()),
        allows: Arc::new(FakeLocalState::new()),
        lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
    };
    let mut w = World {
        svc: Services::new(ports).with_embedder(embedder.clone()),
        files: inner,
        embedder,
    };
    let mut embedding = GraphEmbedding::new(Some(key()), Policy::OPEN);
    embedding.similarity = Some(serde_json::json!({"floor": -1}));
    w.svc
        .graph_new(&ctx(), "/work/g", "demo", None, false, Some(embedding))
        .unwrap();
    for i in 0..3 {
        add(&mut w, &format!("Problem {i}"));
    }
    run(&mut w);
    assert_eq!(edges(&similar_files(&w.files, &dir())).len(), 6);
    add(&mut w, "Problem 3");
    let planned: Vec<Planned> = w
        .svc
        .take_embed_jobs()
        .iter()
        .filter_map(|j| w.svc.embed_plan(j).unwrap())
        .collect();
    let mut vectors = Vec::new();
    for batch in batches(planned) {
        let Step::Done { vectors: v, .. } = step(&*w.embedder, &batch, &mut Pacer::default(), 0)
        else {
            panic!("the fake embeds");
        };
        vectors.extend(v);
    }
    files.armed.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        w.svc.embed_apply(&vectors).unwrap_err().code,
        "write_failed"
    );
    assert_eq!(in_memory(&w).len(), 12, "memory moved on");
    assert_eq!(
        edges(&similar_files(&w.files, &dir())).len(),
        6,
        "the write really failed"
    );
    let retry = w.svc.embed_apply(&vectors).unwrap();
    assert!(
        retry.0.similar_files_written > 1,
        "{retry:?}: more than _meta.tsv"
    );
    assert_eq!(edges(&similar_files(&w.files, &dir())).len(), 12);
    assert_eq!(reload(&mut w), Vec::<&str>::new());
    assert_eq!(in_memory(&w).len(), 12, "a reload reads every list back");
}

/// R102-4: a walk along similar_to that reaches a node with no vector yet (here through an older graph's stored
/// similar_to edge) is incomplete, though its start has a vector.
#[test]
fn a_walk_that_reaches_a_node_with_no_vector_is_incomplete() {
    let mut w = world();
    let first = add(&mut w, "Problem 0");
    run(&mut w);
    let pending = add(&mut w, "Problem 1");
    let edge = umx_core::Edge {
        id: Id(pending.0 + 1000),
        kind: "similar_to".into(),
        from_graph: None,
        from: first,
        to_graph: None,
        to: pending,
        created: "2026-11-10T01:00:00Z".into(),
        created_by: "older-umx".into(),
        authored_by_session: None,
        edited_by_session: None,
        evidence: Vec::new(),
        fields: Default::default(),
    };
    let bytes = umx_core::codec::encode_edges([&edge]);
    put_file(
        &w,
        &dir(),
        FileRef::Graph(umx_core::codec::GraphFile::Edges),
        Some(&bytes),
    );
    w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
    let walk = |w: &World, relation: Option<&str>| {
        w.svc
            .walk(
                &ctx(),
                WalkRequest {
                    node: first.to_text(),
                    relation: relation.map(Into::into),
                    direction: Direction::Out,
                    depth: 2,
                    within: None,
                    limit: 100,
                    offset: 0,
                },
            )
            .unwrap()
    };
    let done = walk(&w, Some("similar_to"));
    assert!(done.value.rows.iter().any(|r| r.node.id == pending));
    let codes: Vec<&str> = done.notes.iter().map(|p| p.code).collect();
    assert_eq!(codes, ["similar_to_incomplete"]);
}

/// Security follow-up: a graph secured after it was open lists every similar_to file left in its folder, `_meta.tsv`
/// too (it names what the lists were made from).
#[test]
fn a_graph_secured_later_lists_its_similar_files_left_behind() {
    let mut w = world();
    for i in 0..3 {
        add(&mut w, &format!("Problem {i}"));
    }
    run(&mut w);
    let raw = w
        .files
        .read(&dir(), umx_core::codec::GraphFile::Graph)
        .unwrap()
        .unwrap();
    let mut meta: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    meta["embedding"]["policy"] = serde_json::json!("secured");
    w.files
        .write(
            &dir(),
            umx_core::codec::GraphFile::Graph,
            &serde_json::to_vec(&meta).unwrap(),
        )
        .unwrap();
    let done = w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
    let remain = done
        .notes
        .iter()
        .find(|p| p.code == "secured_shards_remain")
        .expect("files from the open period remain");
    let listed: Vec<&str> = remain.candidates.iter().map(|c| c.id.as_str()).collect();
    assert!(listed.contains(&"similar/fake@8/_meta.tsv"), "{listed:?}");
    assert!(listed.iter().any(|f| f.starts_with("similar/fake@8/")
        && f.ends_with(".tsv")
        && !f.ends_with("_meta.tsv")));
}

fn similar_query(w: &World, text: &str) -> umx_services::Done<umx_services::QueryRows> {
    w.svc
        .query(
            &ctx(),
            &umx_services::CypherRequest {
                text: text.into(),
                limit: None,
                offset: None,
                params: Vec::new(),
                along: None,
            },
        )
        .unwrap()
}

/// umx query reads the same lists umx neighbours does (ws005 s05): the same nodes and scores, plus the lists' model,
/// k and floor when the query names similar_to, and similar_to_incomplete when a node it read has no vector.
#[test]
fn a_query_naming_similar_to_reads_the_lists_neighbours_reads() {
    use umx_services::cypher::Cell;
    let mut w = world();
    let ids: Vec<Id> = (0..5)
        .map(|i| add(&mut w, &format!("Problem {i}")))
        .collect();
    run(&mut w);
    let (rows, _) = neighbours(&w, ids[0], Some("similar_to"));
    let from_neighbours: Vec<(String, f64)> = rows
        .iter()
        .filter(|r| r.edge.way == Way::Out)
        .map(|r| {
            let score = similar::format_units(r.edge.score.unwrap());
            (r.node.id.to_text(), score.parse().unwrap())
        })
        .collect();
    assert!(!from_neighbours.is_empty());
    let done = similar_query(
        &w,
        &format!(
            "MATCH (a {{id: '{}'}})-[s:similar_to]->(b) RETURN b.id AS id, s.score AS score, s",
            ids[0]
        ),
    );
    let from_query: Vec<(String, f64)> = done
        .value
        .rows
        .iter()
        .map(|r| match &r[..] {
            [Cell::Str(id), Cell::Float(score), Cell::Similar(s)] => {
                assert_eq!((s.from, s.to.to_text()), (ids[0], id.clone()));
                (id.clone(), *score)
            }
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(from_query, from_neighbours);
    assert!(done.notes.is_empty(), "{:?}", done.notes);
    let lists = done.value.similarity.expect("named similar_to");
    let g = w.svc.loaded_graph("demo").unwrap();
    assert_eq!(lists.len(), 1);
    assert_eq!(
        (
            lists[0].graph.as_str(),
            lists[0].key.to_string(),
            lists[0].k,
            lists[0].floor
        ),
        (
            "demo",
            key().to_string(),
            g.similar_settings().0,
            g.similar_settings().1
        )
    );
    // No default floor is calibrated (embeddings-expert B1): an agent must not read it as a relevance cut.
    assert!(!lists[0].floor_calibrated);
    // Not named: no similarity section.
    let plain = similar_query(&w, "MATCH (a) RETURN count(*) AS n");
    assert!(plain.value.similarity.is_none());
    // A node with no vector yet (not embedded): an unbound scan reads its empty list and warns.
    add(&mut w, "Problem late");
    let late = similar_query(&w, "MATCH (a)-[:similar_to]->(b) RETURN count(*) AS n");
    let codes: Vec<&str> = late.notes.iter().map(|p| p.code).collect();
    assert_eq!(codes, ["similar_to_incomplete"]);
    // Embedded: complete again.
    run(&mut w);
    let codes: Vec<&str> = similar_query(&w, "MATCH (a)-[:similar_to]->(b) RETURN count(*) AS n")
        .notes
        .iter()
        .map(|p| p.code)
        .collect();
    assert!(codes.is_empty(), "{codes:?}");
}

/// req-65's done_when: adding and embedding nodes makes similar_to queryable with a score filter, but not untyped.
#[test]
fn req_65_add_embed_then_match_with_score_filter() {
    use umx_services::cypher::Cell;
    let mut w = world();
    let ids: Vec<Id> = (0..6)
        .map(|i| add(&mut w, &format!("Problem {i}")))
        .collect();
    run(&mut w);
    let (rows, _) = neighbours(&w, ids[0], Some("similar_to"));
    let mut from_neighbours: Vec<(String, f64)> = rows
        .iter()
        .filter(|r| r.edge.way == Way::Out)
        .map(|r| {
            let score = similar::format_units(r.edge.score.unwrap());
            (r.node.id.to_text(), score.parse().unwrap())
        })
        .collect();
    from_neighbours.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let high = from_neighbours.first().expect("embedded neighbours").1;
    let low = from_neighbours.last().unwrap().1;
    let threshold = (low + high) / 2.0;
    assert!(low < threshold && threshold < high);
    let expected: Vec<(String, f64)> = from_neighbours
        .iter()
        .filter(|(_, score)| *score > threshold)
        .cloned()
        .collect();
    assert!(!expected.is_empty(), "some neighbours pass the filter");
    assert!(
        expected.len() < from_neighbours.len(),
        "some neighbours fail the filter"
    );
    let done = similar_query(
        &w,
        &format!(
            "MATCH (a {{id: '{}'}})-[s:similar_to]->(b) WHERE s.score > {threshold} \
             RETURN b.id AS id, s.score AS score ORDER BY score DESC, id",
            ids[0]
        ),
    );
    let from_query: Vec<(String, f64)> = done
        .value
        .rows
        .iter()
        .map(|r| match &r[..] {
            [Cell::Str(id), Cell::Float(score)] => (id.clone(), *score),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(from_query, expected);
    let untyped = similar_query(
        &w,
        &format!("MATCH (a {{id: '{}'}})-->(b) RETURN b.id", ids[0]),
    );
    assert!(
        untyped.value.rows.is_empty(),
        "derived similar_to lists are not stored edges"
    );
}

/// One history of a graph, vectors arriving batch by batch: adds, an edit of an embedded text (its whole-node vector
/// moves, so lists that held it lose or gain ground), more adds. umx has no node delete yet.
fn drain_history(w: &mut World, deferred: bool) {
    let ids: Vec<Id> = (0..6).map(|i| add(w, &format!("Problem {i}"))).collect();
    drive(w, deferred);
    w.svc
        .node_edit(
            &ctx(),
            &ids[2].to_text(),
            NodeEditRequest {
                title: Some("Problem two, renamed for the drain".into()),
                ..Default::default()
            },
        )
        .unwrap();
    for i in 6..10 {
        add(w, &format!("Problem {i}"));
        drive(w, deferred);
    }
}

/// Every similar_to file removed, as by hand.
fn remove_similar_files(w: &World) {
    let removals: Vec<FileRef> = similar_files(&w.files, &dir())
        .keys()
        .map(|name| match name.as_str() {
            "_meta.tsv" => FileRef::SimilarMeta(key()),
            path => FileRef::parse(path).unwrap(),
        })
        .collect();
    let set: Vec<Change<'_>> = removals
        .iter()
        .map(|f| Change {
            file: f.clone(),
            bytes: None,
        })
        .collect();
    w.files.write_set(&dir(), &set).unwrap();
}

#[test]
fn deferred_lists_flush_byte_for_byte_to_what_each_batch_and_a_one_shot_rebuild_write() {
    let mut each = world();
    drain_history(&mut each, false);
    let mut later = world();
    drain_history(&mut later, true);
    // Memory is current all along; only the files wait.
    assert_eq!(in_memory(&later), in_memory(&each));
    assert!(later.svc.similar_owed());
    assert_ne!(
        similar_files(&later.files, &dir()),
        similar_files(&each.files, &dir())
    );
    let steps = flush(&mut later);
    assert!(steps > 2, "a file a step, _meta.tsv last: {steps}");
    assert!(!later.svc.similar_owed());
    let flushed = similar_files(&later.files, &dir());
    assert_eq!(flushed, similar_files(&each.files, &dir()), "byte for byte");
    assert_eq!(
        later
            .svc
            .similarity_rebuild(&ctx())
            .unwrap()
            .value
            .files_written,
        0,
        "a rebuild finds nothing to change"
    );
    remove_similar_files(&later);
    later.svc.similarity_rebuild(&ctx()).unwrap();
    assert_eq!(
        similar_files(&later.files, &dir()),
        flushed,
        "a one-shot rebuild from nothing writes the same bytes"
    );
}

#[test]
fn lists_owed_at_a_crash_load_stale_and_empty_then_rebuild_to_the_one_shot_files() {
    let mut w = world();
    drain_history(&mut w, true);
    // Part of the files lag: some list shards are written, `_meta.tsv` is not (it is always last).
    assert!(w.svc.flush_similar_step().is_some());
    assert!(w.svc.similar_owed());
    // The process dies here: vector shards written, similar_to files behind, the old `_meta.tsv` on disk.
    assert_eq!(reload(&mut w), ["similar_to_incomplete"]);
    assert!(w.svc.similarity_stale("demo"));
    assert!(
        in_memory(&w).is_empty(),
        "lagging lists are never served, so none can name a node that is gone"
    );
    assert_eq!(w.svc.take_similarity_rebuilds(), ["demo"]);
    let job = w.svc.similarity_snapshot("demo").unwrap();
    assert!(
        w.svc
            .similarity_apply("demo", job.compute())
            .unwrap()
            .is_some()
    );
    assert!(!w.svc.similarity_stale("demo"));
    let mut each = world();
    drain_history(&mut each, false);
    assert_eq!(
        similar_files(&w.files, &dir()),
        similar_files(&each.files, &dir())
    );
}

#[test]
fn an_unload_writes_the_owed_lists_first() {
    let mut w = world();
    drain_history(&mut w, true);
    assert!(w.svc.similar_owed());
    w.svc.graph_unload(&ctx(), "demo").unwrap();
    let mut each = world();
    drain_history(&mut each, false);
    assert_eq!(
        similar_files(&w.files, &dir()),
        similar_files(&each.files, &dir())
    );
    assert_eq!(reload(&mut w), Vec::<&str>::new(), "the lists load fresh");
}

#[test]
fn a_node_edited_mid_drain_never_gets_its_old_vector_on_disk() {
    let mut w = world();
    let id = add(&mut w, "Problem zero");
    let jobs = w.svc.take_embed_jobs();
    let planned: Vec<Planned> = jobs
        .iter()
        .filter_map(|j| w.svc.embed_plan(j).unwrap())
        .collect();
    let Step::Done { vectors, .. } = step(&*w.embedder, &planned, &mut Pacer::default(), 0) else {
        panic!("the fake embeds");
    };
    let made: Vec<(String, TextHash)> = vectors
        .iter()
        .flat_map(|v| v.vectors.iter().map(|(f, h, _)| (f.clone(), *h)))
        .collect();
    // The title changes while its vectors are being made.
    w.svc
        .node_edit(
            &ctx(),
            &id.to_text(),
            NodeEditRequest {
                title: Some("Problem zero, renamed".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let (applied, _) = w.svc.embed_apply_deferred(&vectors).unwrap();
    let mid = on_disk(&w, id);
    // The edit queued the new texts; they land like any others.
    run_deferred(&mut w);
    flush(&mut w);
    assert!(!w.svc.similar_owed());
    let after = on_disk(&w, id);
    let moved: Vec<&(String, TextHash)> = made.iter().filter(|m| !after.contains(m)).collect();
    assert!(!moved.is_empty(), "the edit moved an embedded text");
    assert!(
        moved.iter().all(|m| !mid.contains(m)),
        "a moved text's old vector never reached disk: {moved:?} in {mid:?} ({applied:?})"
    );
    assert!(
        mid.iter().all(|m| made.contains(m)) && !mid.is_empty(),
        "the texts the edit left alone were written"
    );
}

/// The (field, text hash) of each vector line of `id` in its shard.
fn on_disk(w: &World, id: Id) -> Vec<(String, TextHash)> {
    let shard = umx_core::vectors::ShardFile::of(key(), id);
    let bytes = w
        .files
        .read_ref(&dir(), &FileRef::Shard(shard))
        .unwrap()
        .unwrap_or_default();
    umx_core::vectors::decode_shard(&bytes)
        .into_iter()
        .filter_map(|l| match l {
            umx_core::vectors::ShardLine::Vector(v) if v.node == id => Some((v.field, v.hash)),
            _ => None,
        })
        .collect()
}

/// A world over `files` (a wrapper of `inner`) with its own clock and list settings.
fn world_over(
    files: Arc<dyn GraphFiles>,
    inner: Arc<FakeGraphFiles>,
    clock: Arc<FakeClock>,
    similarity: Option<serde_json::Value>,
) -> World {
    let embedder = Arc::new(FakeEmbedder::new(key()).with_locality(Locality::Local));
    let ports = Ports {
        files: files.clone(),
        vector_cache: files,
        vector_cache_root: GraphDir::parse("/work/cache").unwrap(),
        clock,
        ids: Arc::new(FakeIds::new()),
        state: Arc::new(FakeLocalState::new()),
        scopes: Arc::new(FakeLocalState::new()),
        allows: Arc::new(FakeLocalState::new()),
        lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
        registry: Arc::new(FakeLocalState::new()),
        sources: Arc::new(umx_fakes::FakeSourceReader::open(
            GraphDir::parse("/").unwrap(),
        )),
    };
    let mut w = World {
        svc: Services::new(ports).with_embedder(embedder.clone()),
        files: inner,
        embedder,
    };
    let mut embedding = GraphEmbedding::new(Some(key()), Policy::OPEN);
    embedding.similarity = similarity;
    w.svc
        .graph_new(&ctx(), "/work/g", "demo", None, false, Some(embedding))
        .unwrap();
    w
}

fn retitle(w: &mut World, id: Id, title: &str) {
    w.svc
        .node_edit(
            &ctx(),
            &id.to_text(),
            NodeEditRequest {
                title: Some(title.into()),
                ..Default::default()
            },
        )
        .unwrap();
}

/// The list shards only, without `_meta.tsv`.
fn list_shards(files: &BTreeMap<String, Vec<u8>>) -> BTreeMap<String, Vec<u8>> {
    files
        .iter()
        .filter(|(name, _)| name.as_str() != "_meta.tsv")
        .map(|(n, b)| (n.clone(), b.clone()))
        .collect()
}

/// R180-1: lists A are committed; an edit owes lists B and one B shard lands; the edit is undone, so the vectors
/// (and so `_meta.tsv`'s digest) are A's again, while the files mix A and B; then the process dies. With k fixed and
/// a floor of -1 the edge count matches too, so only the removed `_meta.tsv` stops the mix loading as fresh.
#[test]
fn a_partial_flush_then_an_undone_edit_never_loads_as_fresh() {
    let mut w = world_at(
        "/work/g",
        "demo",
        Policy::OPEN,
        Some(serde_json::json!({"k": 2, "floor": -1})),
    );
    let ids: Vec<Id> = (0..6)
        .map(|i| add(&mut w, &format!("Problem {i}")))
        .collect();
    run(&mut w);
    let a_files = similar_files(&w.files, &dir());
    let a_lists = in_memory(&w);
    assert!(a_files.contains_key("_meta.tsv"), "lists A are committed");
    retitle(
        &mut w,
        ids[2],
        "Problem two, now about something else entirely",
    );
    run_deferred(&mut w);
    let mut steps = 0;
    while list_shards(&similar_files(&w.files, &dir())) == list_shards(&a_files) {
        assert!(w.svc.flush_similar_step().unwrap().is_empty());
        steps += 1;
        assert!(steps < 10, "a B shard lands");
    }
    assert!(w.svc.similar_owed(), "more of B is still owed");
    retitle(&mut w, ids[2], "Problem 2");
    run_deferred(&mut w);
    assert_eq!(in_memory(&w), a_lists, "memory is back to A");
    // The process dies here.
    assert_eq!(reload(&mut w), ["similar_to_incomplete"]);
    assert!(in_memory(&w).is_empty(), "the mixed files are never served");
    let job = w.svc.similarity_snapshot("demo").unwrap();
    assert!(
        w.svc
            .similarity_apply("demo", job.compute())
            .unwrap()
            .is_some()
    );
    assert_eq!(in_memory(&w), a_lists);
    assert_eq!(
        similar_files(&w.files, &dir()),
        a_files,
        "a one-shot A rebuild"
    );
}

/// Records each write set's files and can fail the next set that touches a similar_to file.
struct Recorder {
    inner: Arc<FakeGraphFiles>,
    sets: std::sync::Mutex<Vec<Vec<FileRef>>>,
    fail_next_similar: std::sync::atomic::AtomicBool,
}

fn is_similar(f: &FileRef) -> bool {
    match f {
        FileRef::SimilarMeta(_) => true,
        FileRef::Shard(s) => s.kind() == ShardKind::Similar,
        _ => false,
    }
}

impl GraphFiles for Recorder {
    fn relocate(
        &self,
        from: &umx_core::ports::GraphDir,
        to: &umx_core::ports::GraphDir,
    ) -> Result<(), umx_core::ports::FilesError> {
        self.inner.relocate(from, to)
    }
    fn read_ref(
        &self,
        d: &GraphDir,
        f: &FileRef,
    ) -> Result<Option<Vec<u8>>, umx_core::ports::FilesError> {
        self.inner.read_ref(d, f)
    }
    fn write_set_checked(
        &self,
        d: &GraphDir,
        c: &[Change<'_>],
        expect: Option<&umx_core::ports::GraphStamps>,
    ) -> Result<umx_core::ports::Committed, umx_core::ports::FilesError> {
        if c.iter().any(|c| is_similar(&c.file)) {
            self.sets
                .lock()
                .unwrap()
                .push(c.iter().map(|c| c.file.clone()).collect());
            if self
                .fail_next_similar
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                return Err(umx_core::ports::FilesError::io(
                    "test: similar write failed",
                ));
            }
        }
        self.inner.write_set_checked(d, c, expect)
    }
    fn read_stamped(
        &self,
        d: &GraphDir,
        f: umx_core::codec::GraphFile,
    ) -> Result<(Option<Vec<u8>>, umx_core::ports::FileStamp), umx_core::ports::FilesError> {
        self.inner.read_stamped(d, f)
    }
    fn stamps(
        &self,
        d: &GraphDir,
    ) -> Result<umx_core::ports::GraphStamps, umx_core::ports::FilesError> {
        self.inner.stamps(d)
    }
    fn list(
        &self,
        d: &GraphDir,
    ) -> Result<Vec<umx_core::codec::GraphFile>, umx_core::ports::FilesError> {
        self.inner.list(d)
    }
    fn resolve(&self, d: &GraphDir) -> Result<GraphDir, umx_core::ports::FilesError> {
        self.inner.resolve(d)
    }
    fn list_shards(
        &self,
        d: &GraphDir,
        k: &ModelKey,
        kind: ShardKind,
    ) -> Result<Vec<umx_core::vectors::ShardFile>, umx_core::ports::FilesError> {
        self.inner.list_shards(d, k, kind)
    }
    fn list_vector_spaces(
        &self,
        d: &GraphDir,
    ) -> Result<Vec<ModelKey>, umx_core::ports::FilesError> {
        self.inner.list_vector_spaces(d)
    }
    fn find_graphs(
        &self,
        root: &GraphDir,
        limits: umx_core::ports::FindLimits,
    ) -> Result<umx_core::ports::Found, umx_core::ports::FilesError> {
        self.inner.find_graphs(root, limits)
    }
    fn peek_graph(
        &self,
        dir: &GraphDir,
    ) -> Result<Option<umx_core::ports::FoundGraph>, umx_core::ports::FilesError> {
        self.inner.peek_graph(dir)
    }
    fn adapter_id(&self) -> &'static str {
        "test.recorder"
    }
}

/// Vectors for every queued node, made but not applied.
fn made(w: &mut World) -> Vec<umx_services::Vectors> {
    let planned: Vec<Planned> = w
        .svc
        .take_embed_jobs()
        .iter()
        .filter_map(|j| w.svc.embed_plan(j).unwrap())
        .collect();
    let mut vectors = Vec::new();
    for batch in batches(planned) {
        let Step::Done { vectors: v, .. } = step(&*w.embedder, &batch, &mut Pacer::default(), 0)
        else {
            panic!("the fake embeds");
        };
        vectors.extend(v);
    }
    vectors
}

/// R180-2: lists owed in full after a failed whole-set write, and a failed owed step, are written back a file at a
/// time: no step, the first one that succeeds included, writes more than one file, and none loops on a failure.
#[test]
fn every_owed_write_step_holds_at_most_one_file_through_failures_and_recovery() {
    let inner = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let files = Arc::new(Recorder {
        inner: inner.clone(),
        sets: Default::default(),
        fail_next_similar: false.into(),
    });
    let mut w = world_over(
        files.clone(),
        inner,
        Arc::new(FakeClock::default()),
        Some(serde_json::json!({"floor": -1})),
    );
    for i in 0..4 {
        add(&mut w, &format!("Problem {i}"));
    }
    run(&mut w);
    // A whole-set write fails: the lists are owed in full (R102-2).
    add(&mut w, "Problem 4");
    let vectors = made(&mut w);
    files
        .fail_next_similar
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(w.svc.embed_apply(&vectors).is_err());
    files.sets.lock().unwrap().clear();
    // From here on the daemon's path: deferred applies and owed steps.
    add(&mut w, "Problem 5");
    run_deferred(&mut w);
    assert!(
        files.sets.lock().unwrap().is_empty(),
        "applies write no list file"
    );
    assert!(
        w.svc.flush_similar_step().unwrap().is_empty(),
        "owed in full -> every shard owed"
    );
    assert!(
        w.svc.flush_similar_step().unwrap().is_empty(),
        "_meta.tsv removed"
    );
    files
        .fail_next_similar
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let failed = w.svc.flush_similar_step().unwrap();
    assert_eq!(
        failed.iter().map(|p| p.code).collect::<Vec<_>>(),
        ["write_failed"]
    );
    assert!(
        !w.svc.similar_owed(),
        "a failing folder waits for new lists, not a loop"
    );
    // New lists are owed: the steps go on from where they stopped.
    add(&mut w, "Problem 6");
    run_deferred(&mut w);
    assert!(flush(&mut w) > 2);
    let sets = files.sets.lock().unwrap().clone();
    assert!(sets.len() > 3, "{sets:?}");
    assert!(
        sets.iter().all(|set| set.len() <= 1),
        "a file per step: {sets:?}"
    );
    let flushed = similar_files(&w.files, &dir());
    assert_eq!(reload(&mut w), Vec::<&str>::new(), "the lists load fresh");
    remove_similar_files(&w);
    w.svc.similarity_rebuild(&ctx()).unwrap();
    assert_eq!(
        similar_files(&w.files, &dir()),
        flushed,
        "what a one-shot rebuild writes"
    );
}

/// R180-4: an unload writes what is owed and says when that failed or was not synced; an idle unload keeps it for
/// the daemon's log. A failed write leaves the lists to be rebuilt at the next load.
#[test]
fn an_unload_reports_owed_writes_that_failed_or_did_not_sync() {
    let start = |clock: Arc<FakeClock>| {
        let inner = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
        let mut w = world_over(inner.clone(), inner.clone(), clock, None);
        let ids: Vec<Id> = (0..4)
            .map(|i| add(&mut w, &format!("Problem {i}")))
            .collect();
        run(&mut w);
        retitle(&mut w, ids[1], "Problem one, renamed");
        run_deferred(&mut w);
        assert!(w.svc.similar_owed());
        (w, inner)
    };
    let codes = |p: &[umx_core::Problem]| p.iter().map(|p| p.code).collect::<Vec<_>>();

    let (mut w, inner) = start(Arc::new(FakeClock::default()));
    inner.fail_writes(true);
    let done = w.svc.graph_unload(&ctx(), "demo").unwrap();
    assert_eq!(codes(&done.notes), ["write_failed"]);
    inner.fail_writes(false);
    assert_eq!(reload(&mut w), ["similar_to_incomplete"]);

    let (mut w, inner) = start(Arc::new(FakeClock::default()));
    inner.fail_sync(true);
    let done = w.svc.graph_unload(&ctx(), "demo").unwrap();
    assert!(!done.notes.is_empty());
    assert!(done.notes.iter().all(|p| p.code == "write_not_durable"));
    inner.fail_sync(false);
    assert_eq!(reload(&mut w), Vec::<&str>::new(), "the files landed");

    let clock = Arc::new(FakeClock::default());
    let (mut w, inner) = start(clock.clone());
    w.svc.set_idle_unload_ms(60_000);
    inner.fail_writes(true);
    clock.advance(60_000);
    assert_eq!(w.svc.unload_idle(), ["demo"]);
    assert_eq!(codes(&w.svc.take_unload_problems()), ["write_failed"]);
    assert!(w.svc.take_unload_problems().is_empty());
}

// ---------------------------------------------------------------- F144 (review R1-3): a rebuild on changed files

/// A rebuild never publishes lists for a graph whose files changed on disk since the load, even lists that would
/// write no file: refused with graph_files_changed before they go in, memory and files as they were.
#[test]
fn a_rebuild_of_a_graph_whose_files_changed_publishes_nothing() {
    let mut w = world();
    for i in 0..5 {
        add(&mut w, &format!("Problem {i}"));
    }
    run(&mut w);
    let (files, shown) = (similar_files(&w.files, &dir()), in_memory(&w));
    let job = w.svc.similarity_snapshot("demo").unwrap();
    // A checkout lands while the rebuild computes.
    w.files
        .touch_for_tests(&dir(), umx_core::codec::GraphFile::Nodes);
    let refused = w.svc.similarity_apply("demo", job.compute()).unwrap_err();
    assert_eq!(refused.code, "graph_files_changed");
    assert_eq!(in_memory(&w), shown);
    assert!(!w.svc.similarity_stale("demo"));
    assert_eq!(similar_files(&w.files, &dir()), files);
}

/// Five nodes with their lists written, then another branch's `_meta.tsv` and a load: the lists are stale and a
/// rebuild is due. Returns the world and the good files.
fn stale_lists() -> (World, BTreeMap<String, Vec<u8>>) {
    let mut w = world();
    for i in 0..5 {
        add(&mut w, &format!("Problem {i}"));
    }
    run(&mut w);
    let good = similar_files(&w.files, &dir());
    let mut meta = good["_meta.tsv"].clone();
    let n = meta.iter().rposition(|&b| b == b'\t').unwrap() - 1;
    meta[n] = if meta[n] == b'0' { b'1' } else { b'0' };
    w.files
        .write_set(
            &dir(),
            &[Change {
                file: FileRef::SimilarMeta(key()),
                bytes: Some(&meta),
            }],
        )
        .unwrap();
    assert_eq!(reload(&mut w), ["similar_to_incomplete"]);
    (w, good)
}

/// A change that lands while a rebuild's files are written (after the up-front look) refuses the write, and the
/// rebuild's lists are taken out again: the graph stays stale, nothing is served or owed, the similar_to files and
/// the changed file stay as they are. Control: with nothing landing, the same rebuild goes through.
#[test]
fn a_change_landing_while_a_rebuild_writes_takes_its_lists_out_again() {
    let (mut w, _) = stale_lists();
    let stale = similar_files(&w.files, &dir());
    let job = w.svc.similarity_snapshot("demo").unwrap();
    let nodes = umx_core::codec::GraphFile::Nodes;
    w.files
        .change_after_next_marker_for_tests(&dir(), &dir(), nodes, b"checked out\n");
    let refused = w.svc.similarity_apply("demo", job.compute()).unwrap_err();
    assert_eq!(refused.code, "graph_files_changed");
    assert!(w.svc.similarity_stale("demo"), "stays stale");
    assert!(
        in_memory(&w).is_empty(),
        "the rebuild's lists are not served"
    );
    assert!(!w.svc.similar_owed(), "nothing owed");
    assert_eq!(similar_files(&w.files, &dir()), stale);
    assert_eq!(
        w.files.raw_for_tests(&dir(), nodes).as_deref(),
        Some(&b"checked out\n"[..])
    );
    // Control: the same rebuild with nothing landing.
    let (mut w, good) = stale_lists();
    let job = w.svc.similarity_snapshot("demo").unwrap();
    assert!(
        w.svc
            .similarity_apply("demo", job.compute())
            .unwrap()
            .is_some()
    );
    assert!(!w.svc.similarity_stale("demo"));
    assert!(!in_memory(&w).is_empty());
    assert_eq!(similar_files(&w.files, &dir()), good);
}

/// The digest and its cache key, as `umx embeddings status` reads them.
fn digest_now(w: &World) -> ([u8; 32], umx_services::VectorsStamp) {
    let stamp = w
        .svc
        .embeddings_status(&|_, _| true)
        .remove(0)
        .vectors_stamp
        .unwrap();
    (w.svc.vectors_digest("demo", &stamp).unwrap(), stamp)
}

/// ws004 s34b (F194): `vectors_digest` is `_meta.tsv`'s own digest under the key line (one source). It moves when an
/// edit's new vector is applied, with the node and vector counts unchanged, not at the edit while that vector is
/// pending; it stays through a write that changes no vector and through a restart (it comes from the vector lines
/// on disk).
#[test]
fn the_vectors_digest_moves_with_applied_vectors_only() {
    let mut w = world();
    let ids: Vec<Id> = (0..4)
        .map(|i| add(&mut w, &format!("Problem {i}")))
        .collect();
    run(&mut w);
    let (first, stamp) = digest_now(&w);
    let meta = similar::Meta::decode(&similar_files(&w.files, &dir())["_meta.tsv"]).unwrap();
    assert_eq!(first, similar::vectors_digest(&key(), &meta.digest));
    // A write that changes no vector: same digest.
    w.svc
        .edge_add(
            &ctx(),
            &ids[0].to_text(),
            "supports",
            &ids[1].to_text(),
            &[],
        )
        .unwrap();
    assert_eq!(digest_now(&w).0, first);
    // A text edit: pending, the stored vectors are the old ones, so the digest has not moved.
    w.svc
        .node_edit(
            &ctx(),
            &ids[2].to_text(),
            NodeEditRequest {
                title: Some("Problem two, renamed".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(digest_now(&w).0, first, "moved at the edit");
    let counts = |w: &World| {
        let r = w.svc.embeddings_status(&|_, _| true).remove(0);
        (r.nodes, r.done)
    };
    let before = counts(&w);
    run(&mut w);
    let (applied, moved) = digest_now(&w);
    assert_ne!(applied, first, "the applied vector did not move it");
    assert_ne!(moved, stamp);
    assert_eq!(counts(&w).0, before.0, "same node count");
    assert_eq!(counts(&w).1, Some(4), "every vector current again");
    // A restart: a new load of the same files gives the same digest.
    reload(&mut w);
    assert_eq!(digest_now(&w).0, applied);
}

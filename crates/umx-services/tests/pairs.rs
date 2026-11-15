//! ws004 s31 `umx similarity pairs` through the services over the fakes: cross-graph by default, one model key,
//! search's visibility (d-20, d-21), the stored lists' scores and ties, the similar floor as the default `--min`,
//! never written, and the same lists for any number of parts.

use std::collections::BTreeMap;
use std::sync::Arc;
use umx_core::ports::{GraphDir, GraphFiles, Locality, ModelKey};
use umx_core::setup::GraphFilesSetup;
use umx_core::{GraphEmbedding, Id, Policy};
use umx_fakes::{FakeClock, FakeEmbedder, FakeGraphFiles, FakeIds, FakeLocalState};
use umx_services::{
    Ctx, DigestMemo, NodeAddRequest, NodeEditRequest, Pacer, PairsAsk, PairsJob, PairsResult,
    Planned, Ports, Scope, Services, Step, batches, step,
};

fn key() -> ModelKey {
    ModelKey::new("fake", 8).unwrap()
}

struct World {
    files: Arc<FakeGraphFiles>,
    ids: Arc<FakeIds>,
    embedder: Arc<FakeEmbedder>,
    svc: Services,
}

fn world() -> World {
    let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    world_on(files, Arc::new(FakeIds::new()), key())
}

/// Services over `files` and `ids` (shared with another world, so graphs and ids never collide) embedding in `model`.
fn world_on(files: Arc<FakeGraphFiles>, ids: Arc<FakeIds>, model: ModelKey) -> World {
    let embedder = Arc::new(FakeEmbedder::new(model).with_locality(Locality::Local));
    let ports = Ports {
        sources: Arc::new(umx_fakes::FakeSourceReader::open(
            GraphDir::parse("/").unwrap(),
        )),
        files: files.clone(),
        vector_cache: files.clone(),
        vector_cache_root: GraphDir::parse("/work/cache").unwrap(),
        clock: Arc::new(FakeClock::default()),
        ids: ids.clone(),
        state: Arc::new(FakeLocalState::new()),
        scopes: Arc::new(FakeLocalState::new()),
        registry: Arc::new(FakeLocalState::new()),
        allows: Arc::new(FakeLocalState::new()),
        lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
    };
    World {
        svc: Services::new(ports).with_embedder(embedder.clone()),
        files,
        ids,
        embedder,
    }
}

/// A graph whose vectors are really in `model`: built and embedded by other services on the same disk, then loaded.
fn foreign(w: &mut World, alias: &str, model: ModelKey, titles: &[&str]) {
    let mut other = world_on(w.files.clone(), w.ids.clone(), model.clone());
    graph(&mut other, alias, model, Policy::OPEN);
    for t in titles {
        add(&mut other, alias, t);
    }
    run(&mut other);
    w.svc
        .graph_load(&Ctx::new("tester"), &format!("/work/{alias}"), None)
        .unwrap();
}

fn skipped(r: &PairsResult) -> Vec<(&str, Option<String>, &str)> {
    r.skipped_model
        .iter()
        .map(|s| {
            (
                s.graph.as_str(),
                s.model.as_ref().map(ToString::to_string),
                s.reason,
            )
        })
        .collect()
}

fn graph(w: &mut World, alias: &str, model: ModelKey, policy: Policy) {
    w.svc
        .graph_new(
            &Ctx::new("tester"),
            &format!("/work/{alias}"),
            alias,
            None,
            false,
            Some(GraphEmbedding::new(Some(model), policy)),
        )
        .unwrap();
}

fn add(w: &mut World, alias: &str, title: &str) -> Id {
    w.svc
        .node_add(
            &Ctx::new("tester").graph(alias),
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
            w.svc.embed_apply(&vectors).unwrap();
        }
    }
}

const TITLES: [&str; 6] = [
    "Approval delay in finance",
    "Finance approval is slow",
    "Supplier onboarding gap",
    "Onboarding suppliers takes weeks",
    "Invoice matching errors",
    "Mismatched invoices",
];

/// alpha and beta (open, fake@8) hold the same six texts under their own ids.
fn two() -> World {
    let mut w = world();
    for alias in ["alpha", "beta"] {
        graph(&mut w, alias, key(), Policy::OPEN);
        for t in TITLES {
            add(&mut w, alias, t);
        }
    }
    run(&mut w);
    w
}

fn named(graphs: &str) -> Ctx {
    let mut c = Ctx::new("tester");
    c.graph = Some(graphs.into());
    c
}

fn pairs(w: &World, ctx: &Ctx, ask: PairsAsk) -> (PairsResult, Vec<&'static str>) {
    let done = w.svc.similarity_pairs(ctx, &ask).unwrap();
    (done.value, done.notes.iter().map(|p| p.code).collect())
}

/// Each node's stored list in `alias`: node -> [(other, score)] best first.
fn stored(w: &World, alias: &str) -> BTreeMap<Id, Vec<(Id, i32)>> {
    let g = &w.svc.loaded(alias).unwrap().graph;
    g.nodes()
        .map(|n| {
            (
                n.id(),
                g.similar_out(n.ix())
                    .iter()
                    .map(|e| (g.node_at(e.other).id(), e.score))
                    .collect(),
            )
        })
        .filter(|(_, l): &(Id, Vec<(Id, i32)>)| !l.is_empty())
        .collect()
}

/// The rows as node -> [(other, score)] in rank order.
fn lists(r: &PairsResult) -> BTreeMap<Id, Vec<(Id, i32)>> {
    let mut out: BTreeMap<Id, Vec<(usize, Id, i32)>> = BTreeMap::new();
    for row in r.rows() {
        out.entry(row.id)
            .or_default()
            .push((row.rank, row.other_id, row.score));
    }
    out.into_iter()
        .map(|(id, mut l)| {
            l.sort();
            (id, l.into_iter().map(|(_, o, s)| (o, s)).collect())
        })
        .collect()
}

#[test]
fn same_graph_rows_equal_the_stored_similar_to_lists() {
    let w = two();
    let (r, _) = pairs(
        &w,
        &named("alpha"),
        PairsAsk {
            k: Some(10),
            same_graph: true,
            ..PairsAsk::default()
        },
    );
    assert!(!r.pairs.is_empty());
    assert_eq!(
        lists(&r),
        stored(&w, "alpha"),
        "same scores, same order, same ties"
    );
}

/// F228: a pairs row says which of its node titles umx's secret check flags, as node get flags a title: the
/// secret-shaped one on every row that shows it, as the node or as the other, and no other title. Synthetic titles,
/// a fake token built at run time.
#[test]
fn a_flagged_title_is_named_on_every_pairs_row_that_shows_it() {
    let mut w = world();
    for alias in ["alpha", "beta"] {
        graph(&mut w, alias, key(), Policy::OPEN);
        for t in TITLES {
            add(&mut w, alias, t);
        }
    }
    let secret = add(
        &mut w,
        "beta",
        &format!("Approval delay in finance AKIA{}", "Q7Z3X9K2Q7Z3PAIR"),
    );
    run(&mut w);
    let (r, _) = pairs(
        &w,
        &named("alpha,beta"),
        PairsAsk {
            k: Some(10),
            ..PairsAsk::default()
        },
    );
    let rows = r.rows();
    assert!(
        rows.iter().any(|row| row.id == secret) && rows.iter().any(|row| row.other_id == secret),
        "positive control: the node is paired both ways"
    );
    for row in rows {
        assert_eq!(
            (row.title_flagged, row.other_title_flagged),
            (row.id == secret, row.other_id == secret),
            "{} -> {}",
            row.title,
            row.other_title
        );
    }
}

#[test]
fn cross_graph_only_by_default_and_scored_as_the_stored_lists() {
    let w = two();
    let (r, _) = pairs(
        &w,
        &named("alpha,beta"),
        PairsAsk {
            k: Some(10),
            ..PairsAsk::default()
        },
    );
    assert!(!r.pairs.is_empty());
    assert!(r.rows().into_iter().all(|row| row.graph != row.other_graph));
    assert!(!r.same_graph);
    // beta holds alpha's texts under other ids: a cross pair scores exactly as alpha's own stored pair of the same texts.
    let title = |alias: &str, id: Id| {
        w.svc
            .loaded(alias)
            .unwrap()
            .graph
            .node(id)
            .unwrap()
            .title()
            .to_string()
    };
    let alpha = stored(&w, "alpha");
    let by_title: BTreeMap<(String, String), i32> = alpha
        .iter()
        .flat_map(|(n, l)| {
            l.iter()
                .map(|(o, s)| ((title("alpha", *n), title("alpha", *o)), *s))
                .collect::<Vec<_>>()
        })
        .collect();
    let mut checked = 0;
    for row in r.rows().into_iter().filter(|row| row.graph == "alpha") {
        if let Some(s) = by_title.get(&(row.title.to_string(), row.other_title.to_string())) {
            assert_eq!(row.score, *s, "{} -> {}", row.title, row.other_title);
            checked += 1;
        }
    }
    assert!(checked > 0);
}

#[test]
fn a_node_ties_to_the_lower_other_id() {
    let mut w = world();
    graph(&mut w, "alpha", key(), Policy::OPEN);
    graph(&mut w, "beta", key(), Policy::OPEN);
    let a = add(&mut w, "alpha", "Approval delay in finance");
    let b1 = add(&mut w, "beta", "Approval delay");
    let b2 = add(&mut w, "beta", "Approval delay");
    run(&mut w);
    let (r, _) = pairs(
        &w,
        &named("alpha,beta"),
        PairsAsk {
            k: Some(1),
            ..PairsAsk::default()
        },
    );
    let row = r
        .rows()
        .into_iter()
        .find(|row| row.id == a)
        .expect("a has a pair");
    assert_eq!(row.other_id, b1.min(b2));
}

#[test]
fn only_graphs_with_the_chosen_key_are_compared_and_the_rest_are_named() {
    let mut w = two();
    let other = ModelKey::new("other", 8).unwrap();
    foreign(
        &mut w,
        "gamma",
        other.clone(),
        &["Approval delay in finance"],
    );
    // Named: the first named graph with a key.
    let (r, warnings) = pairs(&w, &named("alpha,gamma,beta"), PairsAsk::default());
    assert_eq!(r.model.as_ref(), Some(&key()));
    assert_eq!(r.model_source.word(), "named-first");
    assert_eq!(r.graphs, ["alpha", "beta"]);
    assert_eq!(
        skipped(&r),
        [("gamma", Some("other@8".to_string()), "another model")]
    );
    assert!(warnings.contains(&"similarity_model_mismatch"));
    assert!(
        r.rows()
            .into_iter()
            .all(|row| row.graph != "gamma" && row.other_graph != "gamma")
    );
    let (r, _) = pairs(&w, &named("gamma,alpha"), PairsAsk::default());
    assert_eq!(r.model.as_ref(), Some(&other));
    assert_eq!(r.graphs, ["gamma"]);
    // Unnamed: the key most graphs record.
    let (r, _) = pairs(&w, &Ctx::new("tester"), PairsAsk::default());
    assert_eq!(r.model.as_ref(), Some(&key()));
    assert_eq!(r.model_source.word(), "most-graphs");
    // --model chooses.
    let (r, _) = pairs(
        &w,
        &Ctx::new("tester"),
        PairsAsk {
            model: Some("other@8".into()),
            ..PairsAsk::default()
        },
    );
    assert_eq!(
        (r.model.as_ref(), r.model_source.word()),
        (Some(&other), "flag")
    );
    assert_eq!(r.graphs, ["gamma"]);
}

#[test]
fn a_secured_graph_is_compared_only_when_named() {
    let mut w = two();
    graph(&mut w, "vault", key(), Policy::SECURED);
    for t in TITLES {
        add(&mut w, "vault", t);
    }
    run(&mut w);
    let unnamed = [
        Ctx::new("tester"),
        Ctx {
            scope: Some(Scope::All {
                include_named: false,
            }),
            ..Ctx::new("tester")
        },
    ];
    for ctx in unnamed {
        let (r, warnings) = pairs(&w, &ctx, PairsAsk::default());
        assert!(!r.graphs.contains(&"vault".to_string()));
        assert!(
            r.skipped_model.iter().all(|s| s.graph != "vault"),
            "never named"
        );
        assert!(
            r.rows()
                .into_iter()
                .all(|row| row.graph != "vault" && row.other_graph != "vault")
        );
        assert!(warnings.contains(&"secured_graphs_skipped"));
    }
    let included = [
        named("alpha,vault"),
        Ctx {
            scope: Some(Scope::All {
                include_named: true,
            }),
            ..Ctx::new("tester")
        },
    ];
    for ctx in included {
        let (r, _) = pairs(&w, &ctx, PairsAsk::default());
        assert!(r.rows().into_iter().any(|row| row.graph == "vault"));
    }
}

#[test]
fn copies_never_pair_a_node_with_itself() {
    let mut w = two();
    let copy = GraphDir::parse("/work/copy").unwrap();
    let from = GraphDir::parse("/work/alpha").unwrap();
    for file in [
        umx_core::codec::GraphFile::Graph,
        umx_core::codec::GraphFile::Nodes,
        umx_core::codec::GraphFile::Edges,
    ] {
        let bytes = w.files.read(&from, file).unwrap().unwrap();
        w.files.write(&copy, file, &bytes).unwrap();
    }
    w.svc
        .graph_load(&Ctx::new("tester"), "/work/copy", Some("copy"))
        .unwrap();
    run(&mut w);
    let (r, warnings) = pairs(
        &w,
        &named("alpha,copy"),
        PairsAsk {
            k: Some(10),
            ..PairsAsk::default()
        },
    );
    assert!(!r.pairs.is_empty(), "the copy's other nodes still pair");
    assert!(r.rows().into_iter().all(|row| row.id != row.other_id));
    assert!(warnings.contains(&"query_graph_twice"));
    // Unnamed: one copy per graph id.
    let (r, _) = pairs(&w, &Ctx::new("tester"), PairsAsk::default());
    assert_eq!(
        r.graphs
            .iter()
            .filter(|g| *g == "alpha" || *g == "copy")
            .count(),
        1
    );
}

#[test]
fn min_defaults_to_the_keys_similar_floor_and_a_flag_overrides_it() {
    let mut w = two();
    let ask = |min: Option<&str>| PairsAsk {
        k: Some(10),
        min: min.map(str::to_owned),
        ..PairsAsk::default()
    };
    // fake@8 is not calibrated: no floor, and a note.
    let (all, warnings) = pairs(&w, &named("alpha,beta"), ask(None));
    assert_eq!((all.min.units, all.min.source), (0, "none"));
    assert!(warnings.contains(&"floor_uncalibrated"));
    // A cut between the lowest and highest scores found, so it must drop some rows and keep others.
    let mut scores: Vec<i32> = all.rows().into_iter().map(|r| r.score).collect();
    scores.sort();
    let cut = scores[scores.len() / 2];
    assert!(
        scores[0] < cut && cut <= scores[scores.len() - 1],
        "{scores:?}"
    );
    let kept = all.rows().into_iter().filter(|r| r.score >= cut).count();
    // The machine config's similar floor is the default once set; graph.json floors are not read.
    let mut floors = umx_core::similarity::Floors::default();
    floors.set(&key(), None, Some(cut as f32 / 10_000.0));
    w.svc.set_floors(floors);
    let (r, warnings) = pairs(&w, &named("alpha,beta"), ask(None));
    assert_eq!((r.min.units, r.min.source), (cut, "config"));
    assert!(!warnings.contains(&"floor_uncalibrated"));
    assert_eq!(r.total, kept, "the config floor cuts");
    // A flag overrides it, below as well as above.
    let text = format!("{}.{:04}", cut / 10_000, cut % 10_000);
    let (r, _) = pairs(&w, &named("alpha,beta"), ask(Some("0.0000")));
    assert_eq!((r.min.units, r.min.source), (0, "flag"));
    assert_eq!(
        r.total,
        all.rows().into_iter().filter(|r| r.score >= 0).count()
    );
    let (r, _) = pairs(&w, &named("alpha,beta"), ask(Some(&text)));
    assert_eq!(r.total, kept, "the flag cuts");
    assert!(
        w.svc
            .similarity_pairs(&named("alpha,beta"), &ask(Some("0.25")))
            .is_err(),
        "4 decimals, as similar_to writes them"
    );
}

#[test]
fn nothing_is_written_and_the_stored_lists_do_not_move() {
    let w = two();
    let writes = |w: &World| {
        (
            w.files.shard_writes(),
            w.files.similar_writes(),
            w.files.graph_writes(),
        )
    };
    let files_before = writes(&w);
    let lists_before = (stored(&w, "alpha"), stored(&w, "beta"));
    let (r, _) = pairs(
        &w,
        &named("alpha,beta"),
        PairsAsk {
            k: Some(10),
            same_graph: true,
            ..PairsAsk::default()
        },
    );
    assert!(!r.pairs.is_empty());
    assert_eq!(writes(&w), files_before);
    assert_eq!((stored(&w, "alpha"), stored(&w, "beta")), lists_before);
}

#[test]
fn rows_are_best_first_and_page_with_offset_and_limit() {
    let w = two();
    let all = pairs(&w, &named("alpha,beta"), PairsAsk::default()).0;
    assert!(all.pairs.windows(2).all(|p| p[0].score >= p[1].score));
    assert_eq!(all.total, all.pairs.len());
    assert!(!all.truncated);
    let page = pairs(
        &w,
        &named("alpha,beta"),
        PairsAsk {
            limit: Some(2),
            offset: Some(1),
            ..PairsAsk::default()
        },
    )
    .0;
    assert_eq!(page.rows(), all.rows()[1..3]);
    assert_eq!(page.total, all.total);
    assert!(page.truncated);
}

/// F194 (ws004 s34a): one call returns every row, past the old 10,000-row page, so a view never pays for a compare
/// per page. 2 graphs of 600 nodes at k 10 give 12,000 rows, all in one call; the limit stops at 250,000.
#[test]
fn one_call_returns_more_than_ten_thousand_rows_whole() {
    let mut w = world();
    for alias in ["alpha", "beta"] {
        graph(&mut w, alias, key(), Policy::OPEN);
        for i in 0..600 {
            add(&mut w, alias, &format!("{alias} topic {i}"));
        }
    }
    run(&mut w);
    let ask = |limit| PairsAsk {
        k: Some(10),
        limit: Some(limit),
        ..PairsAsk::default()
    };
    let (r, _) = pairs(&w, &named("alpha,beta"), ask(20_000));
    assert_eq!(r.total, 2 * 600 * 10, "uncalibrated: nothing is cut");
    assert_eq!(r.pairs.len(), r.total);
    assert!(!r.truncated);
    assert!(
        w.svc
            .similarity_pairs(&named("alpha,beta"), &ask(250_000))
            .is_ok()
    );
    assert!(
        w.svc
            .similarity_pairs(&named("alpha,beta"), &ask(250_001))
            .is_err()
    );
}

#[test]
fn the_same_pairs_for_any_number_of_parts() {
    let mut w = two();
    graph(&mut w, "gamma", key(), Policy::OPEN);
    for t in TITLES {
        add(&mut w, "gamma", &format!("{t} again"));
    }
    run(&mut w);
    let ctx = named("alpha,beta,gamma");
    let ask = PairsAsk {
        k: Some(4),
        same_graph: true,
        ..PairsAsk::default()
    };
    let with_parts = |n: usize| {
        let mut job = w.svc.pairs_plan(&ctx, &ask).unwrap();
        for g in 0..job.graphs() {
            assert!(copy(&w, &mut job, g));
        }
        let parts = (0..n).map(|p| job.part(p, n, &|| false).unwrap()).collect();
        w.svc
            .pairs_finish(&ctx, job.merge(parts))
            .unwrap()
            .unwrap()
            .value
    };
    let one = with_parts(1);
    assert!(!one.pairs.is_empty());
    for n in [2, 3, 7] {
        assert_eq!(with_parts(n), one, "{n} parts");
    }
}

#[test]
fn a_cancelled_part_returns_nothing() {
    let w = two();
    let mut job = w
        .svc
        .pairs_plan(&named("alpha,beta"), &PairsAsk::default())
        .unwrap();
    for g in 0..job.graphs() {
        assert!(copy(&w, &mut job, g));
    }
    assert!(job.part(0, 1, &|| true).is_none());
}

#[test]
fn a_graph_secured_or_unloaded_meanwhile_is_never_read() {
    let mut w = two();
    let unnamed = Ctx::new("tester");
    let mut job = w.svc.pairs_plan(&unnamed, &PairsAsk::default()).unwrap();
    for g in 0..job.graphs() {
        assert!(copy(&w, &mut job, g));
    }
    let part = job_part(&job);
    let found = job.merge(vec![part]);
    w.svc
        .graph_set(
            &Ctx::new("tester"),
            "beta",
            &[("policy".into(), "secured".into())],
        )
        .unwrap();
    assert!(
        w.svc.pairs_finish(&unnamed, found).unwrap().is_none(),
        "secured meanwhile"
    );
    let mut job = w
        .svc
        .pairs_plan(&named("alpha,beta"), &PairsAsk::default())
        .unwrap();
    for g in 0..job.graphs() {
        assert!(copy(&w, &mut job, g));
    }
    let part = job_part(&job);
    let found = job.merge(vec![part]);
    w.svc.graph_unload(&Ctx::new("tester"), "beta").unwrap();
    // A named graph unloaded meanwhile is refused as at the start.
    let refused = w
        .svc
        .pairs_finish(&named("alpha,beta"), found)
        .err()
        .unwrap();
    assert_eq!(refused.code, "graph_not_loaded");
    // A copy step after an unload says so too.
    let mut job = w
        .svc
        .pairs_plan(&named("alpha"), &PairsAsk::default())
        .unwrap();
    w.svc.graph_unload(&Ctx::new("tester"), "alpha").unwrap();
    assert!(!copy(&w, &mut job, 0));
}

fn job_part(job: &umx_services::PairsJob) -> umx_services::PairsPart {
    job.part(0, 1, &|| false).unwrap()
}

/// One copy step with a memo of its own.
fn copy(w: &World, job: &mut PairsJob, g: usize) -> bool {
    w.svc.pairs_copy(job, g, &mut DigestMemo::new()).unwrap()
}

#[test]
fn a_graph_with_no_vectors_never_chooses_the_model_and_is_listed_as_such() {
    let mut w = world();
    graph(&mut w, "alpha", key(), Policy::OPEN);
    for t in TITLES {
        add(&mut w, "alpha", t);
    }
    run(&mut w);
    let other = ModelKey::new("other", 8).unwrap();
    foreign(&mut w, "gamma", other.clone(), &["Approval delay"]);
    // Two graphs that record other@8 but hold no vectors: they would outvote alpha if they counted.
    graph(&mut w, "empty_a", other.clone(), Policy::OPEN);
    graph(&mut w, "empty_b", other.clone(), Policy::OPEN);
    let (r, _) = pairs(&w, &Ctx::new("tester"), PairsAsk::default());
    assert_eq!(
        r.model.as_ref(),
        Some(&key()),
        "one graph each: the lowest alias"
    );
    assert_eq!(r.graphs, ["alpha"]);
    assert_eq!(
        skipped(&r),
        [
            ("empty_a", Some("other@8".to_string()), "no vectors yet"),
            ("empty_b", Some("other@8".to_string()), "no vectors yet"),
            ("gamma", Some("other@8".to_string()), "another model"),
        ]
    );
    // Named first: an empty graph named first does not choose either.
    let (r, _) = pairs(&w, &named("empty_a,gamma,alpha"), PairsAsk::default());
    assert_eq!(r.model.as_ref(), Some(&other));
    assert_eq!(r.graphs, ["gamma"]);
    // An empty graph in the chosen space is not compared.
    let (r, _) = pairs(
        &w,
        &named("empty_a,gamma"),
        PairsAsk {
            model: Some("other@8".into()),
            ..PairsAsk::default()
        },
    );
    assert_eq!(r.graphs, ["gamma"]);
    assert_eq!(
        skipped(&r)[0],
        ("empty_a", Some("other@8".to_string()), "no vectors yet")
    );
}

/// Security R298 B1: a left-out graph secured between the plan and the finish of an unnamed read is counted with
/// the secured graphs, never named in skipped_model or similarity_model_mismatch.
#[test]
fn a_left_out_graph_secured_meanwhile_is_counted_not_named() {
    let mut w = two();
    foreign(
        &mut w,
        "gamma",
        ModelKey::new("other", 8).unwrap(),
        &["Approval delay"],
    );
    let unnamed = Ctx::new("tester");
    let finish = |w: &mut World, secure: bool| {
        let mut job = w.svc.pairs_plan(&unnamed, &PairsAsk::default()).unwrap();
        for g in 0..job.graphs() {
            assert!(copy(w, &mut job, g));
        }
        let part = job_part(&job);
        let found = job.merge(vec![part]);
        if secure {
            w.svc
                .graph_set(
                    &Ctx::new("tester"),
                    "gamma",
                    &[("policy".into(), "secured".into())],
                )
                .unwrap();
        }
        w.svc.pairs_finish(&unnamed, found).unwrap().unwrap()
    };
    let open = finish(&mut w, false);
    assert_eq!(skipped(&open.value)[0].0, "gamma");
    assert!(
        open.notes
            .iter()
            .any(|p| p.code == "similarity_model_mismatch")
    );
    let done = finish(&mut w, true);
    assert!(
        done.value.skipped_model.is_empty(),
        "{:?}",
        done.value.skipped_model
    );
    let codes: Vec<&str> = done.notes.iter().map(|p| p.code).collect();
    assert!(!codes.contains(&"similarity_model_mismatch"), "{codes:?}");
    let secured = done
        .notes
        .iter()
        .find(|p| p.code == "secured_graphs_skipped")
        .expect("counted");
    assert!(
        secured
            .slots
            .iter()
            .any(|(n, v)| *n == "n" && *v == umx_core::Slot::Count(1)),
        "{secured:?}"
    );
    // Named, it stays named: the caller chose it.
    let (r, _) = pairs(&w, &named("alpha,gamma"), PairsAsk::default());
    assert_eq!(skipped(&r)[0].0, "gamma");
}

/// Each open graph's `vectors_digest` as `embeddings status` gives it, through `memo`.
fn status_digests(w: &World, memo: &mut DigestMemo) -> BTreeMap<String, [u8; 32]> {
    w.svc
        .embeddings_status(&|_, _| true)
        .into_iter()
        .filter_map(|r| {
            let stamp = r.vectors_stamp?;
            let digest = w.svc.vectors_digest_kept(&r.alias, &stamp, memo)?;
            Some((r.alias, digest))
        })
        .collect()
}

/// ws004 s36 (F206): each compared graph's digest is the one `embeddings status` gives while nothing changes; an
/// edit whose vector lands between the plan and the copy shows in the digest the copy takes, through a memo that
/// still holds the pre-edit entry, and the other graph's stays.
#[test]
fn the_compared_digests_are_status_digests_taken_at_the_copy() {
    let mut w = two();
    let edited = add(&mut w, "alpha", "Late arrival");
    run(&mut w);
    let ctx = named("alpha,beta");
    let mut memo = DigestMemo::new();
    let before = status_digests(&w, &mut memo);
    let (r, _) = pairs(&w, &ctx, PairsAsk::default());
    assert_eq!(r.graphs, ["alpha", "beta"]);
    assert_eq!(r.digests, [before["alpha"], before["beta"]]);
    // The plan, then an edit and its new vector, then the copy.
    let mut job = w.svc.pairs_plan(&ctx, &PairsAsk::default()).unwrap();
    w.svc
        .node_edit(
            &Ctx::new("tester").graph("alpha"),
            &edited.to_text(),
            NodeEditRequest {
                title: Some("Late arrival, renamed".into()),
                ..Default::default()
            },
        )
        .unwrap();
    run(&mut w);
    for g in 0..job.graphs() {
        assert!(w.svc.pairs_copy(&mut job, g, &mut memo).unwrap());
    }
    let part = job_part(&job);
    let done = w
        .svc
        .pairs_finish(&ctx, job.merge(vec![part]))
        .unwrap()
        .unwrap();
    let after = status_digests(&w, &mut DigestMemo::new());
    assert_ne!(
        after["alpha"], before["alpha"],
        "the applied vector moved it"
    );
    assert_eq!(done.value.digests, [after["alpha"], before["beta"]]);
}

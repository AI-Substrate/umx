//! `umx query` in services (req-27, workstream 005 s04): d-20 scope, `--param`, the permit gate, the clock backstop
//! and the policy check at finish.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use umx_core::ports::{Clock, GraphDir};
use umx_core::value::SetValue;
use umx_core::{GraphEmbedding, Id, Policy, Value};
use umx_fakes::{FakeGraphFiles, FakeIds, FakeLocalState};
use umx_services::cypher::Cell;
use umx_services::{
    AlongRequest, Ctx, CypherRequest, NodeAddRequest, Ports, QUERY_PERMITS, Services,
};

/// A clock that moves `step` ms on every read; once `flip` is set, its next read makes this machine's policy
/// record unreadable, so every graph's effective policy becomes secured (as a record change mid-query would).
struct Ticking {
    now: AtomicU64,
    step: u64,
    flip: AtomicBool,
    state: Arc<FakeLocalState>,
}

impl Clock for Ticking {
    fn now_ms(&self) -> u64 {
        if self.flip.swap(false, Ordering::SeqCst) {
            self.state.set_bytes(b"not a state file");
        }
        self.now.fetch_add(self.step, Ordering::SeqCst)
    }

    fn adapter_id(&self) -> &'static str {
        "test.ticking"
    }
}

struct World {
    svc: Services,
    clock: Arc<Ticking>,
}

fn world(step: u64) -> World {
    let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let state = Arc::new(FakeLocalState::new());
    let clock = Arc::new(Ticking {
        now: AtomicU64::new(1_800_000_000_000),
        step,
        flip: AtomicBool::new(false),
        state: state.clone(),
    });
    let ports = Ports {
        sources: std::sync::Arc::new(umx_fakes::FakeSourceReader::open(
            umx_core::ports::GraphDir::parse("/").unwrap(),
        )),
        files: files.clone(),
        vector_cache: files,
        vector_cache_root: GraphDir::parse("/work/cache").unwrap(),
        clock: clock.clone(),
        ids: Arc::new(FakeIds::new()),
        state,
        scopes: Arc::new(FakeLocalState::new()),
        registry: Arc::new(FakeLocalState::new()),
        allows: Arc::new(FakeLocalState::new()),
        lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
    };
    World {
        svc: Services::new(ports),
        clock,
    }
}

fn ctx(graph: Option<&str>) -> Ctx {
    let mut c = Ctx::new("tester");
    c.graph = graph.map(str::to_string);
    c
}

fn graph(w: &mut World, alias: &str, policy: Policy) {
    let embedding = GraphEmbedding {
        key: None,
        text_form: 1,
        policy,
        similarity: None,
        extra: Default::default(),
    };
    w.svc
        .graph_new(
            &ctx(None),
            &format!("/work/{alias}"),
            alias,
            None,
            false,
            Some(embedding),
        )
        .unwrap();
}

fn add(w: &mut World, alias: &str, title: &str) -> Id {
    w.svc
        .node_add(
            &ctx(Some(alias)),
            NodeAddRequest {
                kind: "problem".into(),
                title: title.into(),
                ..Default::default()
            },
        )
        .unwrap()
        .value
        .node
        .id
}

fn request(text: &str, params: &[&str]) -> CypherRequest {
    CypherRequest {
        text: text.into(),
        limit: None,
        offset: None,
        params: params
            .iter()
            .map(|p| {
                let (name, value) = p.split_once('=').expect("name=value");
                (name.to_string(), SetValue::Raw(value.to_string()))
            })
            .collect(),
        along: None,
    }
}

fn texts(rows: &[Vec<Cell>]) -> Vec<Vec<String>> {
    rows.iter()
        .map(|r| {
            r.iter()
                .map(|c| match c {
                    Cell::Str(s) => s.clone(),
                    Cell::Int(i) => i.to_string(),
                    other => format!("{other:?}"),
                })
                .collect()
        })
        .collect()
}

/// An open graph "notes" and a secured graph "vault", one node each.
fn two_graphs() -> World {
    let mut w = world(0);
    graph(&mut w, "notes", Policy::OPEN);
    graph(&mut w, "vault", Policy::SECURED);
    add(&mut w, "notes", "open thing");
    add(&mut w, "vault", "secret thing");
    w
}

const TITLES: &str = "MATCH (n) RETURN n.graph AS g, n.title AS t ORDER BY g";

#[test]
fn declared_unused_vocabulary_uses_the_union_of_only_the_selected_ontologies() {
    let mut w = world(0);
    graph(&mut w, "notes", Policy::OPEN);
    graph(&mut w, "terms", Policy::OPEN);
    add(&mut w, "notes", "one node");
    let terms = ctx(Some("terms"));
    w.svc
        .ontology_add_type(&terms, "unfilled_kind", &[], None)
        .unwrap();
    w.svc
        .ontology_add_relation(&terms, "planned_link", false, None)
        .unwrap();
    w.svc
        .ontology_add_field(&terms, "problem", "planned_field:string", None)
        .unwrap();
    w.svc
        .ontology_add_relation_field(&terms, "planned_link", "edge_note:string")
        .unwrap();

    for (text, unknown, unused, name) in [
        (
            "MATCH (n:unfilled_kind) RETURN n.id",
            "query_unknown_label",
            "query_label_unused",
            "unfilled_kind",
        ),
        (
            "MATCH ()-[:planned_link]->(n) RETURN n.id",
            "query_unknown_relation",
            "query_relation_unused",
            "planned_link",
        ),
        (
            "MATCH (n) WHERE n.planned_field IS NOT NULL RETURN n.id",
            "query_unknown_property",
            "query_property_unused",
            "planned_field",
        ),
        (
            "MATCH (n) WHERE n.edge_note IS NOT NULL RETURN n.id",
            "query_unknown_property",
            "query_property_unused",
            "edge_note",
        ),
    ] {
        for (scope, code) in [
            (Some("notes"), unknown),
            (Some("notes,terms"), unused),
            (None, unused),
        ] {
            let done = w.svc.query(&ctx(scope), &request(text, &[])).unwrap();
            assert!(done.value.rows.is_empty(), "{text}: {:?}", done.value.rows);
            assert_eq!(done.notes.len(), 1, "{text}: {:?}", done.notes);
            assert_eq!(done.notes[0].code, code, "{text}: {scope:?}");
            assert_eq!(done.notes[0].slot("name").as_deref(), Some(name));
        }
    }

    let done = w
        .svc
        .query(
            &ctx(Some("notes")),
            &request("MATCH ()-[:answers]->(n) RETURN n.id", &[]),
        )
        .unwrap();
    assert!(done.value.rows.is_empty());
    assert_eq!(done.notes[0].code, "query_relation_unused");
    let done = w
        .svc
        .query(
            &ctx(Some("notes,terms")),
            &request("MATCH (n) RETURN n.planned_field AS field", &[]),
        )
        .unwrap();
    assert_eq!(done.value.rows, [[Cell::Null]]);
    assert_eq!(done.notes[0].code, "query_property_unused");
}

#[test]
fn used_stored_fields_do_not_become_unused_on_virtual_graph_nodes() {
    let mut w = world(0);
    graph(&mut w, "notes", Policy::OPEN);
    let c = ctx(Some("notes"));
    w.svc
        .ontology_add_field(&c, "problem", "planned_field:string", None)
        .unwrap();
    w.svc
        .node_add(
            &c,
            NodeAddRequest {
                kind: "problem".into(),
                title: "uses the field".into(),
                sets: vec![("planned_field".into(), SetValue::Str("present".into()))],
                ..Default::default()
            },
        )
        .unwrap();
    let stored = w
        .svc
        .query(
            &c,
            &request("MATCH (n) RETURN n.planned_field AS field", &[]),
        )
        .unwrap();
    assert_eq!(stored.value.rows, [[Cell::Str("present".into())]]);
    assert!(stored.notes.is_empty());
    let virtual_node = w
        .svc
        .query(
            &c,
            &request("MATCH (g:graph) RETURN g.planned_field AS field", &[]),
        )
        .unwrap();
    assert_eq!(virtual_node.value.rows, [[Cell::Null]]);
    assert_eq!(virtual_node.notes[0].code, "query_unknown_property");
}

#[test]
fn secured_declarations_do_not_change_an_unnamed_querys_vocabulary_warning() {
    let mut w = two_graphs();
    let vault = ctx(Some("vault"));
    w.svc
        .ontology_add_type(&vault, "private_kind", &[], None)
        .unwrap();
    w.svc
        .ontology_add_relation(&vault, "private_link", false, None)
        .unwrap();
    w.svc
        .ontology_add_field(&vault, "problem", "private_field:string", None)
        .unwrap();
    for (text, unknown, unused) in [
        (
            "MATCH (n:private_kind) RETURN n.id",
            "query_unknown_label",
            "query_label_unused",
        ),
        (
            "MATCH ()-[:private_link]->(n) RETURN n.id",
            "query_unknown_relation",
            "query_relation_unused",
        ),
        (
            "MATCH (n) WHERE n.private_field IS NOT NULL RETURN n.id",
            "query_unknown_property",
            "query_property_unused",
        ),
    ] {
        let hidden = w.svc.query(&ctx(None), &request(text, &[])).unwrap();
        assert!(hidden.value.rows.is_empty());
        assert_eq!(
            hidden.notes.iter().map(|p| p.code).collect::<Vec<_>>(),
            ["secured_graphs_skipped", unknown]
        );
        let named = w
            .svc
            .query(&ctx(Some("notes,vault")), &request(text, &[]))
            .unwrap();
        assert!(named.value.rows.is_empty());
        assert_eq!(
            named.notes.iter().map(|p| p.code).collect::<Vec<_>>(),
            [unused]
        );
    }
}

/// d-20: an unnamed query reads open graphs only and counts the rest without naming them; a named secured graph
/// is read; a comma list reads both.
#[test]
fn an_unnamed_query_reads_open_graphs_and_only_counts_secured_ones() {
    let w = two_graphs();
    let done = w.svc.query(&ctx(None), &request(TITLES, &[])).unwrap();
    assert_eq!(texts(&done.value.rows), [["notes", "open thing"]]);
    let codes: Vec<&str> = done.notes.iter().map(|p| p.code).collect();
    assert_eq!(codes, ["secured_graphs_skipped"]);
    assert!(!format!("{:?}", done.notes).contains("vault"));

    let done = w
        .svc
        .query(&ctx(Some("vault")), &request(TITLES, &[]))
        .unwrap();
    assert_eq!(texts(&done.value.rows), [["vault", "secret thing"]]);
    assert!(done.notes.is_empty(), "{:?}", done.notes);

    let done = w
        .svc
        .query(&ctx(Some("notes,vault")), &request(TITLES, &[]))
        .unwrap();
    assert_eq!(
        texts(&done.value.rows),
        [["notes", "open thing"], ["vault", "secret thing"]]
    );
    assert!(done.notes.is_empty(), "{:?}", done.notes);
}

/// No error or warning from an unnamed query names a secured graph, even when the query reads a label, relation or
/// field only the secured graph has: soft vocabulary looks at the graphs in scope only.
#[test]
fn an_unnamed_query_says_nothing_of_a_secured_graphs_vocabulary() {
    let mut w = two_graphs();
    w.svc
        .ontology_add_type(&ctx(Some("vault")), "secret_kind", &[], None)
        .unwrap();
    w.svc
        .node_add(
            &ctx(Some("vault")),
            NodeAddRequest {
                kind: "secret_kind".into(),
                title: "kept".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let done = w
        .svc
        .query(
            &ctx(None),
            &request("MATCH (n:secret_kind) RETURN n.id AS id", &[]),
        )
        .unwrap();
    assert!(done.value.rows.is_empty());
    let codes: Vec<&str> = done.notes.iter().map(|p| p.code).collect();
    assert_eq!(codes, ["secured_graphs_skipped", "query_unknown_label"]);
    let text = format!("{:?}", done.notes);
    assert!(!text.contains("vault"), "{text}");
}

/// `--param name=value`: typed as `--set` types a value, bound as a literal (so `{id: $id}` uses the id index);
/// a repeated, malformed or missing parameter is refused.
#[test]
fn params_are_typed_and_checked() {
    let mut w = world(0);
    graph(&mut w, "notes", Policy::OPEN);
    let id = add(&mut w, "notes", "the one");
    add(&mut w, "notes", "another");
    let svc = &w.svc;
    let run = |text: &str, params: &[&str]| svc.query(&ctx(None), &request(text, params));
    let done = run(
        "MATCH (n {id: $id}) RETURN n.title AS t",
        &[&format!("id={id}")],
    )
    .unwrap();
    assert_eq!(texts(&done.value.rows), [["the one"]]);
    let done = run(
        "UNWIND [1] AS x RETURN $n = 3 AS three, $f = 0.5 AS half, $b AS b",
        &["n=3", "f=0.5", "b=true"],
    )
    .unwrap();
    assert_eq!(
        done.value.rows,
        [[Cell::Bool(true), Cell::Bool(true), Cell::Bool(true)]]
    );
    for (params, what) in [
        (&["id=a", "id=b"][..], "twice"),
        (&["Id=a"][..], "snake_case"),
    ] {
        let p = run("MATCH (n {id: $id}) RETURN n.id AS id", params).unwrap_err();
        assert_eq!(p.code, "usage_invalid", "{params:?}");
        assert!(format!("{p:?}").contains(what), "{params:?}: {p:?}");
    }
    let p = run("MATCH (n {id: $id}) RETURN n.id AS id", &[]).unwrap_err();
    assert_eq!(p.code, "query_param_missing");
}

/// At most QUERY_PERMITS queries run at once: another is refused with query_busy and nothing runs. A permit is given
/// back when its guard drops, also when the query panics, and the count never goes below zero.
#[test]
fn a_third_query_is_busy_and_permits_come_back() {
    let w = two_graphs();
    let q = request("MATCH (n) RETURN count(*) AS nodes", &[]);
    let held: Vec<_> = (0..QUERY_PERMITS)
        .map(|_| w.svc.hold_query_permit().expect("a permit"))
        .collect();
    assert!(w.svc.hold_query_permit().is_none());
    assert_eq!(w.svc.query(&ctx(None), &q).unwrap_err().code, "query_busy");
    drop(held);
    assert_eq!(w.svc.query_permits_held(), 0);
    assert!(w.svc.query(&ctx(None), &q).is_ok());
    assert_eq!(w.svc.query_permits_held(), 0);

    let svc = std::panic::AssertUnwindSafe(&w.svc);
    let caught = std::panic::catch_unwind(|| {
        let _permit = svc.hold_query_permit().expect("a permit");
        panic!("a query panicked");
    });
    assert!(caught.is_err());
    assert_eq!(w.svc.query_permits_held(), 0);
    let all: Vec<_> = (0..QUERY_PERMITS)
        .map(|_| w.svc.hold_query_permit().expect("every permit is back"))
        .collect();
    assert_eq!(w.svc.query_permits_held(), QUERY_PERMITS);
    drop(all);
    assert_eq!(w.svc.query_permits_held(), 0);
}

/// A query big enough for the cancel hook to be asked several times.
fn long_query() -> String {
    let list = (0..500)
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "MATCH (n) WITH count(n) AS nodes UNWIND [{list}] AS x UNWIND [{list}] AS y RETURN nodes, count(*) AS pairs"
    )
}

/// The 10 s backstop over the clock port: query_timed_out, no rows.
#[test]
fn the_clock_backstop_times_a_query_out() {
    let mut w = world(6_000);
    graph(&mut w, "notes", Policy::OPEN);
    add(&mut w, "notes", "a");
    let p = w
        .svc
        .query(&ctx(None), &request(&long_query(), &[]))
        .unwrap_err();
    assert_eq!(p.code, "query_timed_out", "{p:?}");
    // A clock that stands still lets the same query finish.
    let mut w = world(0);
    graph(&mut w, "notes", Policy::OPEN);
    add(&mut w, "notes", "a");
    let done = w
        .svc
        .query(&ctx(None), &request(&long_query(), &[]))
        .unwrap();
    assert_eq!(texts(&done.value.rows), [["1", "250000"]]);
}

#[test]
fn all_graph_query_rechecks_visibility_and_ignores_the_default_graph() {
    for include_named in [false, true] {
        let mut w = world(0);
        graph(&mut w, "notes", Policy::OPEN);
        add(&mut w, "notes", "a");
        let c = ctx(None)
            .scope(umx_services::Scope::All { include_named })
            .default_graph("not_the_scope");
        w.clock.flip.store(true, Ordering::SeqCst);
        let done = w.svc.query(&c, &request(&long_query(), &[])).unwrap();
        if include_named {
            assert_eq!(texts(&done.value.rows), [["1", "250000"]]);
            let warning = done
                .notes
                .iter()
                .find(|p| p.code == "secured_graphs_read")
                .unwrap();
            assert_eq!(warning.candidates[0].id, "notes");
        } else {
            assert_eq!(texts(&done.value.rows), [["0", "250000"]]);
            assert_eq!(
                done.notes.iter().map(|p| p.code).collect::<Vec<_>>(),
                ["secured_graphs_skipped"]
            );
        }
    }
}

#[test]
fn all_graph_query_without_opt_in_matches_the_unnamed_scope() {
    let mut w = world(0);
    graph(&mut w, "open", Policy::OPEN);
    graph(&mut w, "vault", Policy::SECURED);
    let open = add(&mut w, "open", "Open note");
    add(&mut w, "vault", "Named note");
    let req = request("MATCH (n) RETURN n.id AS id", &[]);
    let plain = w.svc.query(&ctx(None), &req).unwrap();
    let all = w
        .svc
        .query(
            &ctx(None)
                .scope(umx_services::Scope::All {
                    include_named: false,
                })
                .default_graph("vault"),
            &req,
        )
        .unwrap();
    assert_eq!(plain, all);
    assert_eq!(texts(&all.value.rows), [[open.to_text()]]);
    let skipped = all
        .notes
        .iter()
        .find(|p| p.code == "secured_graphs_skipped")
        .unwrap();
    assert_eq!(skipped.slot("n").as_deref(), Some("1"));
    assert!(skipped.candidates.is_empty());
}

/// d-20 at finish: a graph that stops being open while an unnamed query runs leaves the answer (the query runs
/// again without it) and is counted, not named. A named query keeps it.
#[test]
fn a_graph_secured_while_the_query_runs_leaves_an_unnamed_query() {
    let mut w = world(0);
    graph(&mut w, "notes", Policy::OPEN);
    add(&mut w, "notes", "a");
    w.clock.flip.store(true, Ordering::SeqCst);
    // The flip happens at the query's first clock read, after its scope is taken, so the policy is unreadable by
    // the time the query finishes.
    let done = w
        .svc
        .query(&ctx(None), &request(&long_query(), &[]))
        .unwrap();
    assert_eq!(texts(&done.value.rows), [["0", "250000"]]);
    let codes: Vec<&str> = done.notes.iter().map(|p| p.code).collect();
    assert_eq!(codes, ["secured_graphs_skipped"]);
    assert_eq!(w.svc.effective_policy("notes"), Some(Policy::SECURED));
    let done = w
        .svc
        .query(&ctx(Some("notes")), &request(&long_query(), &[]))
        .unwrap();
    assert_eq!(texts(&done.value.rows), [["1", "250000"]]);
}

/// Reviewer B2: `--along` on an unnamed query whose graph is secured as it runs. The range's reply object, its hints
/// and its refusals (spine names, anchor titles, resolved ends) are that graph's, so none comes back: the graph leaves
/// the answer and is counted, as without a range.
#[test]
fn a_range_on_a_graph_secured_while_the_query_runs_says_nothing_of_it() {
    let setup = || {
        let mut w = world(0);
        graph(&mut w, "notes", Policy::OPEN);
        let mut segment = |title: &str, at: &str| {
            w.svc
                .node_add(
                    &ctx(Some("notes")),
                    NodeAddRequest {
                        kind: "segment".into(),
                        title: title.into(),
                        sets: vec![("starts_on".into(), SetValue::Raw(at.into()))],
                        ..Default::default()
                    },
                )
                .unwrap()
                .value
                .node
                .id
        };
        let one = segment("Hidden segment one", "2026-11-11");
        let two = segment("Hidden segment two", "2026-11-12");
        let point = add(&mut w, "notes", "Hidden point");
        let decl = serde_json::json!({
            "name": "hidden_day", "title": "Hidden day", "main": true,
            "anchors": { "type": "segment" }, "order": ["starts_on"], "place": ["raised_in"],
        });
        w.svc
            .graph_spine_set(&ctx(Some("notes")), "notes", &decl)
            .unwrap();
        (w, [one, two, point])
    };
    let ranged = |spine: &str, after: Id| {
        let mut r = request("MATCH (n) RETURN n.id AS id", &[]);
        r.along = Some(AlongRequest {
            spine: spine.into(),
            after: Some(after.to_text()),
            before: None,
        });
        r
    };
    let (w, [one, _, point]) = setup();
    // While the graph is open: a range, then refusals that name its spines and its anchors.
    let open = w.svc.query(&ctx(None), &ranged("hidden_day", one)).unwrap();
    assert_eq!(open.value.along.unwrap().after, Some(one.to_text()));
    let missing = w.svc.query(&ctx(None), &ranged("nope", one)).unwrap_err();
    assert_eq!(missing.code, "spine_not_found");
    assert!(format!("{missing:?}").contains("hidden_day"));
    let end = w
        .svc
        .query(&ctx(None), &ranged("hidden_day", point))
        .unwrap_err();
    assert_eq!(end.code, "along_end_invalid");
    assert!(format!("{end:?}").contains("Hidden segment"));
    for (what, spine, after) in [
        ("a range", "hidden_day", 0),
        ("a missing spine", "nope", 0),
        ("an end that is no anchor", "hidden_day", 2),
    ] {
        let (w, ids) = setup();
        w.clock.flip.store(true, Ordering::SeqCst);
        let done = w
            .svc
            .query(&ctx(None), &ranged(spine, ids[after]))
            .unwrap_or_else(|p| panic!("{what}: {p:?}"));
        assert!(
            done.value.rows.is_empty() && done.value.along.is_none(),
            "{what}"
        );
        let codes: Vec<&str> = done.notes.iter().map(|p| p.code).collect();
        assert_eq!(codes, ["secured_graphs_skipped"], "{what}");
        let reply = format!("{done:?}");
        for hidden in ["hidden_day", "Hidden", "\"notes\""]
            .into_iter()
            .map(String::from)
            .chain(ids.iter().map(|id| id.to_text()))
        {
            assert!(!reply.contains(&hidden), "{what}: {hidden} in {reply}");
        }
    }
}

/// `--param-str` keeps graph text such as "true" or "2026" a string; `--param-json` takes a flat list (cypher-expert
/// F1); a nested list is refused.
#[test]
fn param_str_and_json_pass_text_and_lists() {
    let mut w = world(0);
    graph(&mut w, "notes", Policy::OPEN);
    let a = add(&mut w, "notes", "2026");
    let b = add(&mut w, "notes", "true");
    let svc = &w.svc;
    let run = |text: &str, params: Vec<(&str, SetValue)>| {
        let mut req = request(text, &[]);
        req.params = params
            .into_iter()
            .map(|(n, v)| (n.to_string(), v))
            .collect();
        svc.query(&ctx(None), &req)
    };
    let titled = "MATCH (n) WHERE n.title = $t RETURN n.id AS id";
    for (t, want) in [("2026", a), ("true", b)] {
        let raw = run(titled, vec![("t", SetValue::Raw(t.into()))]).unwrap();
        assert!(raw.value.rows.is_empty(), "--param {t} is typed, not text");
        let text = run(titled, vec![("t", SetValue::Str(t.into()))]).unwrap();
        assert_eq!(
            texts(&text.value.rows),
            [[want.to_text()]],
            "--param-str {t}"
        );
    }
    let ids = Value::Array(vec![Value::String(a.to_text()), Value::String(b.to_text())]);
    let done = run(
        "MATCH (n) WHERE n.id IN $ids RETURN count(*) AS found",
        vec![("ids", SetValue::Json(ids))],
    )
    .unwrap();
    assert_eq!(texts(&done.value.rows), [["2"]]);
    let nested = Value::Array(vec![Value::Array(vec![Value::from(1)])]);
    let p = run(
        "UNWIND $x AS y RETURN y AS z",
        vec![("x", SetValue::Json(nested))],
    )
    .unwrap_err();
    assert_eq!(p.code, "usage_invalid");
}

/// A graph "dates" whose `problem.due_on` is kind `date`, holding every precision (ws008 TM2).
fn dated() -> World {
    let mut w = world(0);
    graph(&mut w, "dates", Policy::OPEN);
    let c = ctx(Some("dates"));
    w.svc
        .ontology_add_field(&c, "problem", "due_on:date", None)
        .unwrap();
    for (title, due) in [
        ("year", Some("2025")),
        ("january", Some("2025-01")),
        ("february", Some("2025-02")),
        ("mid_january", Some("2025-01-15")),
        ("late_january", Some("2025-01-31T23:00:00Z")),
        ("thursday", Some("2026-10-29")),
        ("friday", Some("2026-10-30")),
        // Friday 09:40 in Sydney, stored as Thursday 22:40 UTC (R4).
        ("meeting", Some("2026-10-30T09:40:00+11:00")),
        ("undated", None),
    ] {
        w.svc
            .node_add(
                &c,
                NodeAddRequest {
                    kind: "problem".into(),
                    title: title.into(),
                    sets: due
                        .map(|d| ("due_on".to_string(), SetValue::Raw(d.into())))
                        .into_iter()
                        .collect(),
                    ..Default::default()
                },
            )
            .unwrap();
    }
    w
}

fn titles(rows: &[Vec<Cell>]) -> std::collections::BTreeSet<String> {
    texts(rows).into_iter().map(|r| r[0].clone()).collect()
}

/// `--where` and Cypher answer every date comparison alike, over every mix of precisions, because both ask
/// umx_core's one date compare.
#[test]
fn where_and_cypher_compare_dates_alike() {
    let w = dated();
    let c = ctx(Some("dates"));
    let listed = |expr: &str| -> std::collections::BTreeSet<String> {
        let req = umx_services::NodeListRequest {
            kind: Some("problem".into()),
            wheres: vec![expr.to_string()],
            ..Default::default()
        };
        let page = w.svc.node_list(&c, req).unwrap().value;
        page.rows.into_iter().map(|n| n.to_node().title).collect()
    };
    let queried = |text: &str| titles(&w.svc.query(&c, &request(text, &[])).unwrap().value.rows);
    let literals = [
        "2025",
        "2025-01",
        "2025-02",
        "2025-01-15",
        "2025-01-31T23:00:00Z",
        "2026-10-29",
        "2026-10-30",
        "2026-10-29T22:40:00Z",
    ];
    for lit in literals {
        for (op, cypher_op) in [
            ("=", "="),
            ("!=", "<>"),
            ("<", "<"),
            ("<=", "<="),
            (">", ">"),
            (">=", ">="),
        ] {
            let cli = listed(&format!("due_on{op}{lit}"));
            let cypher = queried(&format!(
                "MATCH (n:problem) WHERE n.due_on {cypher_op} \"{lit}\" RETURN n.title"
            ));
            assert_eq!(cli, cypher, "due_on {op} {lit}");
        }
    }
    let set = |names: &[&str]| {
        names
            .iter()
            .map(|s| s.to_string())
            .collect::<std::collections::BTreeSet<_>>()
    };
    // R4: the Sydney Friday meeting is before the UTC Friday, and on the UTC Thursday.
    assert!(listed("due_on<2026-10-30").contains("meeting"));
    assert_eq!(listed("due_on=2026-10-29"), set(&["thursday", "meeting"]));
    assert!(listed("due_on<=2026-10-29").contains("meeting"));
    assert_eq!(
        listed("due_on=2025-01"),
        set(&["year", "january", "mid_january", "late_january"])
    );
    assert_eq!(
        listed("due_on<2025-02"),
        set(&["january", "mid_january", "late_january"])
    );
    // A literal with an offset reads as its instant.
    assert_eq!(
        queried("MATCH (n:problem) WHERE n.due_on = \"2026-10-30T09:40:00+11:00\" RETURN n.title"),
        set(&["thursday", "meeting"])
    );
}

/// Every Cypher path that compares by `=` or by order uses the date compare: property maps, a parameter, `IN`
/// (as written and hoisted), the simple `CASE`, `OPTIONAL MATCH`, `EXISTS`, `WITH ... WHERE`, `ORDER BY`, `min`
/// and `max`. DISTINCT keeps identity.
#[test]
fn every_cypher_path_compares_dates_by_instant() {
    let w = dated();
    let c = ctx(Some("dates"));
    let run =
        |text: &str, params: &[&str]| w.svc.query(&c, &request(text, params)).unwrap().value.rows;
    let set = |names: &[&str]| {
        names
            .iter()
            .map(|s| s.to_string())
            .collect::<std::collections::BTreeSet<_>>()
    };
    let thursday = set(&["thursday", "meeting"]);
    for (text, params) in [
        (
            "MATCH (n:problem {due_on: \"2026-10-29\"}) RETURN n.title",
            &[][..],
        ),
        (
            "MATCH (n:problem) WHERE n.due_on = $d RETURN n.title",
            &["d=2026-10-29"][..],
        ),
        (
            "MATCH (n:problem) WHERE n.due_on IN [\"2026-10-29\", \"soon\"] RETURN n.title",
            &[][..],
        ),
        (
            "WITH [\"2026-10-29\", \"soon\"] AS days MATCH (n:problem) WHERE n.due_on IN days RETURN n.title",
            &[][..],
        ),
        (
            "MATCH (n:problem) WHERE CASE n.due_on WHEN \"2026-10-29\" THEN true ELSE false END RETURN n.title",
            &[][..],
        ),
        (
            "MATCH (a:problem {title: \"year\"}) OPTIONAL MATCH (n:problem) WHERE n.due_on = \"2026-10-29\" \
             RETURN n.title",
            &[][..],
        ),
        (
            "MATCH (n:problem) WHERE EXISTS { MATCH (m:problem) WHERE m.due_on = \"2026-10-29\" AND m.title = n.title } \
             RETURN n.title",
            &[][..],
        ),
        (
            "MATCH (n:problem) WITH n WHERE n.due_on = \"2026-10-29\" RETURN n.title",
            &[][..],
        ),
    ] {
        assert_eq!(titles(&run(text, params)), thursday, "{text}");
    }
    // Wholly before, through `<` on a WITH: the Sydney meeting is before the UTC Friday (R4).
    assert!(
        titles(&run(
            "MATCH (n:problem) WITH n WHERE n.due_on < \"2026-10-30\" RETURN n.title",
            &[]
        ))
        .contains("meeting")
    );
    // ORDER BY: by start, the longer span first.
    let ordered: Vec<String> = texts(&run(
        "MATCH (n:problem) WHERE n.due_on IS NOT NULL RETURN n.title AS title, n.due_on AS due ORDER BY due",
        &[],
    ))
    .into_iter()
    .map(|r| r[0].clone())
    .collect();
    assert_eq!(
        ordered,
        [
            "year",
            "january",
            "mid_january",
            "late_january",
            "february",
            "thursday",
            "meeting",
            "friday"
        ]
    );
    assert_eq!(
        texts(&run(
            "MATCH (n:problem) RETURN min(n.due_on) AS first, max(n.due_on) AS last",
            &[]
        )),
        [["2025", "2026-10-30"]]
    );
    // Text sorts as before, except that a date written another way sits at its instant (cypher-expert's row).
    assert_eq!(
        texts(&run(
            "UNWIND [\"2025\", \" x\", \"abc\", \"2025-01-01T09:40:00+11:00\", \"2024-12-31T23:00:00Z\"] AS v \
             RETURN v ORDER BY v",
            &[]
        )),
        [
            [" x"],
            ["2025-01-01T09:40:00+11:00"],
            ["2024-12-31T23:00:00Z"],
            ["2025"],
            ["abc"]
        ]
    );
    assert_eq!(
        texts(&run(
            "UNWIND [\"2024-12-31T23:00:00Z\", \"2025-01-01T09:40:00+11:00\"] AS v RETURN min(v) AS first",
            &[]
        )),
        [["2025-01-01T09:40:00+11:00"]]
    );
    // DISTINCT and grouping keep identity: `2025` and `2025-01` overlap, yet stay two values.
    assert_eq!(
        run(
            "MATCH (n:problem) WHERE n.title IN [\"year\", \"january\"] RETURN DISTINCT n.due_on AS d",
            &[]
        )
        .len(),
        2
    );
}

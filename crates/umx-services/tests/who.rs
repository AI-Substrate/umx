//! Services over the fakes: `umx who` and `umx node who` (ws007 s04, req-96). One test per rule: exact keys, bare
//! seats and sessions, ambiguity, unknown keys, d-20 (security R5), the join across graphs on prov_key, the typed
//! stamp accessors against kept raw values (graph-addressing-expert N1) and the evidence paths
//! (graph-addressing-expert).

use std::sync::Arc;
use umx_core::codec::GraphFile;
use umx_core::ports::{GraphDir, GraphFiles};
use umx_core::provenance::{Actor, ActorInput, Tier};
use umx_core::{Id, Problem};
use umx_fakes::{FakeClock, FakeGraphFiles, FakeIds, FakeLocalState};
use umx_services::{
    Ctx, EdgeEditRequest, NodeAddRequest, Ports, Services, StampState, WhoListed, WhoRecord,
};

struct World {
    files: Arc<FakeGraphFiles>,
    svc: Services,
}

fn ctx() -> Ctx {
    Ctx::new("tester")
}

/// Graphs `demo`, `beta` and `vault`, all open; tests secure `vault` when they need to.
fn world() -> World {
    let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let ports = Ports {
        sources: Arc::new(umx_fakes::FakeSourceReader::open(
            GraphDir::parse("/").unwrap(),
        )),
        files: files.clone(),
        vector_cache: Arc::new(FakeGraphFiles::new(GraphDir::parse("/cache").unwrap())),
        vector_cache_root: GraphDir::parse("/cache").unwrap(),
        clock: Arc::new(FakeClock::default()),
        ids: Arc::new(FakeIds::new()),
        state: Arc::new(FakeLocalState::new()),
        scopes: Arc::new(FakeLocalState::new()),
        registry: Arc::new(FakeLocalState::new()),
        allows: Arc::new(FakeLocalState::new()),
        lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
    };
    let mut svc = Services::new(ports);
    for (path, alias) in [
        ("/work/g", "demo"),
        ("/work/b", "beta"),
        ("/work/v", "vault"),
    ] {
        svc.graph_new(&ctx(), path, alias, None, false, None)
            .unwrap();
    }
    World { files, svc }
}

fn secure(w: &mut World, alias: &str) {
    let given = [("policy".to_string(), "secured".to_string())];
    w.svc.graph_set(&ctx(), alias, &given).unwrap();
}

/// A pij seat writing in harness session `session` (stored hashed, as the CLI sends it).
fn seat(seat: &str, harness: &str, session: &str) -> Actor {
    Actor::try_from(ActorInput {
        tier: Tier::Pij,
        seat: Some(seat.into()),
        role: Some("coder".into()),
        model: Some("Example/Model-5".into()),
        effort: None,
        harness: Some(harness.into()),
        harness_session: Some(session.into()),
        user: Some("tester".into()),
    })
    .unwrap()
}

fn cli(user: &str) -> Actor {
    Actor::try_from(ActorInput {
        tier: Tier::Cli,
        seat: None,
        role: None,
        model: None,
        effort: None,
        harness: None,
        harness_session: None,
        user: Some(user.into()),
    })
    .unwrap()
}

const S1: &str = "h-1111111111111111";
const S2: &str = "h-2222222222222222";

fn add(w: &mut World, graph: &str, actor: &Actor, title: &str, evidence: &[&str]) -> Id {
    w.svc
        .node_add(
            &ctx().graph(graph).actor(actor.clone()),
            NodeAddRequest {
                kind: "problem".into(),
                title: title.into(),
                evidence: evidence.iter().map(|e| (*e).into()).collect(),
                ..Default::default()
            },
        )
        .unwrap()
        .value
        .node
        .id
}

fn who(w: &World, c: &Ctx, key: &str) -> Result<WhoListed, Problem> {
    w.svc.who(c, key, 100).map(|d| d.value)
}

/// (graph, record, title or relation, how, sessions).
type Row = (String, &'static str, String, Vec<&'static str>, Vec<String>);

/// One [`Row`] per who row.
fn rows(listed: &WhoListed) -> Vec<Row> {
    listed
        .rows
        .iter()
        .map(|r| {
            (
                r.alias.clone(),
                r.record.as_str(),
                r.title.clone().unwrap_or_else(|| r.kind.clone()),
                r.how.clone(),
                r.sessions.clone(),
            )
        })
        .collect()
}

fn slot(p: &Problem, name: &str) -> String {
    p.slots
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, s)| s.to_string())
        .unwrap_or_else(|| panic!("{p:?} has no slot {name}"))
}

fn candidates(p: &Problem) -> Vec<String> {
    p.candidates.iter().map(|c| c.id.clone()).collect()
}

/// A node of `demo` by address: three graphs are loaded, so a bare id names no graph.
fn at(id: Id) -> String {
    format!("umx:demo/{id}")
}

fn session_key(harness: &str, session: &str) -> String {
    format!("session/{harness}/{session}")
}

#[test]
fn a_full_key_a_bare_seat_and_a_bare_session_name_what_they_wrote() {
    let mut w = world();
    let (a1, a2, b) = (
        seat("pij-alpha", "copilot", S1),
        seat("pij-alpha", "copilot", S2),
        seat("pij-bravo", "claude", "h-3333333333333333"),
    );
    add(&mut w, "demo", &a1, "First by alpha", &[]);
    add(&mut w, "demo", &a2, "Second by alpha", &[]);
    add(&mut w, "demo", &b, "By bravo", &[]);
    let one = |title: &str, session: &str| {
        (
            "demo".to_string(),
            "node",
            title.to_string(),
            // node add stamps both: the author is also the last editor until another session edits.
            vec!["authored", "edited"],
            vec![session_key("copilot", session)],
        )
    };
    let alpha = vec![one("First by alpha", S1), one("Second by alpha", S2)];
    for key in ["agent/pij/pij-alpha", "pij-alpha", "PIJ-ALPHA"] {
        let listed = who(&w, &ctx(), key).unwrap();
        assert_eq!(listed.key, "agent/pij/pij-alpha", "{key}");
        assert_eq!(rows(&listed), alpha, "{key}");
        assert_eq!((listed.total, listed.truncated), (2, false));
    }
    for key in [session_key("copilot", S1).as_str(), S1] {
        let listed = who(&w, &ctx(), key).unwrap();
        assert_eq!(listed.key, session_key("copilot", S1));
        assert_eq!(rows(&listed), [one("First by alpha", S1)], "{key}");
    }
    // The cap cuts rows, never the count.
    let capped = w.svc.who(&ctx(), "pij-alpha", 1).unwrap().value;
    assert_eq!(
        (capped.rows.len(), capped.total, capped.truncated),
        (1, 2, true)
    );
    // Not a prefix match, not a model, harness or platform key.
    for (given, code) in [
        ("pij-alph", "who_unknown"),
        ("model/example-model-5", "usage_invalid"),
        ("harness/copilot", "usage_invalid"),
        ("agent/pij/Pij-Alpha", "usage_invalid"),
        ("pij alpha", "usage_invalid"),
    ] {
        assert_eq!(who(&w, &ctx(), given).unwrap_err().code, code, "{given}");
    }
}

/// Review F1: past the page, hits are only counted. One identity owns every content node and edge; at limit 0
/// no row is built and the count is whole.
#[test]
fn past_the_page_hits_are_counted_not_built() {
    let mut w = world();
    let alpha = seat("pij-alpha", "copilot", S1);
    let ids: Vec<Id> = (0..5)
        .map(|n| add(&mut w, "demo", &alpha, &format!("Node {n}"), &[]))
        .collect();
    w.svc
        .edge_add(
            &ctx().graph("demo").actor(alpha.clone()),
            &ids[0].to_text(),
            "supports",
            &ids[1].to_text(),
            &[],
        )
        .unwrap();
    for (limit, rows) in [(0, 0), (2, 2), (100, 6)] {
        let listed = w.svc.who(&ctx(), "pij-alpha", limit).unwrap().value;
        assert_eq!(
            (listed.rows.len(), listed.total, listed.truncated),
            (rows, 6, rows < 6),
            "limit {limit}"
        );
    }
}

#[test]
fn a_bare_id_that_ends_several_keys_is_ambiguous_and_lists_them() {
    let mut w = world();
    add(
        &mut w,
        "demo",
        &seat("pij-alpha", "copilot", S1),
        "Copilot",
        &[],
    );
    add(
        &mut w,
        "demo",
        &seat("pij-bravo", "claude", S1),
        "Claude",
        &[],
    );
    add(&mut w, "demo", &cli("pat"), "Shell", &[]);
    let err = who(&w, &ctx(), S1).unwrap_err();
    assert_eq!(err.code, "who_ambiguous");
    assert_eq!(
        candidates(&err),
        [session_key("claude", S1), session_key("copilot", S1)]
    );
    assert_eq!(slot(&err, "n"), "2");
    // A cli user is an agent and a session key of the same name: never a guess.
    let err = who(&w, &ctx(), "pat").unwrap_err();
    assert_eq!(candidates(&err), ["agent/cli/pat", "session/cli/pat"]);
    // Either full key answers.
    let listed = who(&w, &ctx(), "session/cli/pat").unwrap();
    assert_eq!(rows(&listed)[0].2, "Shell");
}

/// Security R5 and its plan-time check: a key only a secured graph holds is unknown unless that graph is named; the
/// candidates and the count never depend on what the secured graph holds.
#[test]
fn unnamed_secured_graphs_are_counted_never_searched_or_named() {
    let mut w = world();
    add(
        &mut w,
        "demo",
        &seat("pij-alpha", "copilot", S1),
        "Open one",
        &[],
    );
    add(
        &mut w,
        "demo",
        &seat("pij-bravo", "claude", S1),
        "Open two",
        &[],
    );
    add(
        &mut w,
        "vault",
        &seat("pij-charlie", "codex", S1),
        "Hidden",
        &[],
    );
    add(
        &mut w,
        "vault",
        &seat("pij-delta", "codex", S2),
        "Hidden too",
        &[],
    );
    secure(&mut w, "vault");
    let ambiguous = who(&w, &ctx(), S1).unwrap_err();
    assert_eq!(ambiguous.code, "who_ambiguous");
    assert_eq!(
        candidates(&ambiguous),
        [session_key("claude", S1), session_key("copilot", S1)],
        "the vault's codex session is not a candidate"
    );
    let unknown = who(&w, &ctx(), S2).unwrap_err();
    assert_eq!(unknown.code, "who_unknown");
    let nothing = who(&w, &ctx(), "h-9999999999999999").unwrap_err();
    for p in [&ambiguous, &unknown, &nothing] {
        assert_eq!(
            (slot(p, "graphs"), slot(p, "secured")),
            ("2".into(), "1".into()),
            "{p:?}"
        );
        assert!(!format!("{p:?}").contains("vault"));
    }
    assert_eq!(
        who(&w, &ctx(), "agent/pij/pij-delta").unwrap_err().code,
        "who_unknown"
    );
    // A secured graph read is one the caller named, alone or in a list.
    for named in ["vault", "demo,vault"] {
        let listed = who(&w, &ctx().graph(named), "agent/pij/pij-delta").unwrap();
        assert_eq!(rows(&listed)[0].2, "Hidden too", "{named}");
    }
    // An answer from the open graphs warns the count, never a name.
    let done = w.svc.who(&ctx(), "pij-alpha", 100).unwrap();
    let codes: Vec<_> = done.notes.iter().map(|p| p.code).collect();
    assert_eq!(codes, ["secured_graphs_skipped"]);
}

/// Ids are per graph; the same seat is the same key. `--graph` narrows the join.
#[test]
fn graphs_join_on_the_key_never_on_node_ids() {
    let mut w = world();
    let alpha = seat("pij-alpha", "copilot", S1);
    add(&mut w, "demo", &alpha, "In demo", &[]);
    add(&mut w, "beta", &alpha, "In beta", &[]);
    let agent = |alias: &str| {
        w.svc
            .loaded_graph(alias)
            .unwrap()
            .provenance_node("agent/pij/pij-alpha")
            .unwrap()
            .id()
    };
    assert_ne!(agent("demo"), agent("beta"));
    let listed = who(&w, &ctx(), "pij-alpha").unwrap();
    let mut seen: Vec<_> = rows(&listed).into_iter().map(|r| (r.0, r.2)).collect();
    seen.sort();
    assert_eq!(
        seen,
        [
            ("beta".to_string(), "In beta".to_string()),
            ("demo".to_string(), "In demo".to_string())
        ]
    );
    let narrowed = who(&w, &ctx().graph("beta"), "pij-alpha").unwrap();
    assert_eq!(rows(&narrowed).len(), 1);
    assert_eq!(rows(&narrowed)[0].0, "beta");
}

/// An agent's rows come from each of its sessions; a session's from that session only; edges count (authored and
/// last edited through the typed edge accessors).
#[test]
fn sessions_and_seats_list_the_edges_they_made_and_last_edited() {
    let mut w = world();
    let (s1, s2) = (
        seat("pij-alpha", "copilot", S1),
        seat("pij-alpha", "copilot", S2),
    );
    let a = add(&mut w, "demo", &s1, "From", &[]);
    let b = add(&mut w, "demo", &s1, "To", &[]);
    let edge = w
        .svc
        .edge_add(
            &ctx().graph("demo").actor(s1.clone()),
            &a.to_text(),
            "supports",
            &b.to_text(),
            &[],
        )
        .unwrap()
        .value
        .edge
        .id;
    w.svc
        .edge_edit(
            &ctx().graph("demo").actor(s2.clone()),
            &edge.to_text(),
            &EdgeEditRequest {
                relation: None,
                reverse: true,
                ..Default::default()
            },
        )
        .unwrap();
    let edge_row = |listed: &WhoListed| {
        listed
            .rows
            .iter()
            .find(|r| r.record == WhoRecord::Edge)
            .map(|r| (r.id, r.how.clone(), r.sessions.clone()))
    };
    let by_s2 = who(&w, &ctx(), S2).unwrap();
    assert_eq!(rows(&by_s2).len(), 1, "s2 only edited the edge");
    assert_eq!(
        edge_row(&by_s2),
        Some((edge, vec!["edited"], vec![session_key("copilot", S2)]))
    );
    let by_seat = who(&w, &ctx(), "pij-alpha").unwrap();
    assert_eq!(
        edge_row(&by_seat),
        Some((
            edge,
            vec!["authored", "edited"],
            vec![session_key("copilot", S1), session_key("copilot", S2)]
        ))
    );
    assert_eq!(by_seat.total, 3);
}

/// Graph-addressing-expert N1: a malformed stamp is kept raw and `field()` still shows it, but who and node who read
/// only the typed accessors, so it attributes nothing. A stamp naming a provenance node that is not a session reads
/// unknown.
#[test]
fn stamps_are_read_typed_never_from_kept_raw_values() {
    let mut w = world();
    let alpha = seat("pij-alpha", "copilot", S1);
    let id = add(&mut w, "demo", &alpha, "Tampered", &[]);
    let kept = add(&mut w, "demo", &alpha, "Pointing at the agent", &[]);
    let graph = w.svc.loaded_graph("demo").unwrap();
    let session = graph.node(id).unwrap().authored_by_session().unwrap();
    let agent = graph.provenance_node("agent/pij/pij-alpha").unwrap().id();
    let dir = GraphDir::parse("/work/g").unwrap();
    let nodes = String::from_utf8(w.files.read(&dir, GraphFile::Nodes).unwrap().unwrap()).unwrap();
    let stamp = format!("\"authored_by_session\":\"{session}\"");
    let lines: Vec<String> = nodes
        .lines()
        .map(|line| {
            if line.contains(&id.to_text()) {
                line.replace(&stamp, "\"authored_by_session\":\"not-an-id\"")
            } else if line.contains(&kept.to_text()) {
                line.replace(&stamp, &format!("\"authored_by_session\":\"{agent}\""))
            } else {
                line.to_string()
            }
        })
        .collect();
    assert_ne!(
        lines.join("\n"),
        nodes.trim_end(),
        "the stamps were rewritten"
    );
    w.files.tamper_for_tests(
        &dir,
        GraphFile::Nodes,
        format!("{}\n", lines.join("\n")).as_bytes(),
    );
    w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
    let graph = w.svc.loaded_graph("demo").unwrap();
    let raw = graph.node(id).unwrap();
    assert_eq!(raw.authored_by_session(), None);
    assert_eq!(
        raw.field("authored_by_session"),
        Some(serde_json::json!("not-an-id")),
        "positive control: the raw value is kept and field() shows it"
    );
    // Only the untouched edited stamps attribute: the malformed and the misdirected authored stamps say nothing.
    let listed = who(&w, &ctx(), "pij-alpha").unwrap();
    let how: Vec<_> = listed.rows.iter().map(|r| (r.id, r.how.clone())).collect();
    assert_eq!(how, [(id, vec!["edited"]), (kept, vec!["edited"])]);
    for node in [id, kept] {
        let stamps = w.svc.node_who(&ctx(), &at(node)).unwrap().value.stamps;
        assert_eq!(stamps[0].state, StampState::Unknown, "{node}");
        assert_eq!(stamps[0].session, None);
        assert_eq!(stamps[1].state, StampState::Recorded);
    }
}

/// Graph-addressing-expert: `evidence[i].by_session` and `evidence_removed[i].removed.by_session` are the adder;
/// `evidence_removed[i].by_session` is the remover. A pointer with no kind is attributed too (graph-storage-expert).
#[test]
fn evidence_is_attributed_by_path_and_never_swapped() {
    let mut w = world();
    let (adder, remover) = (
        seat("pij-alpha", "copilot", S1),
        seat("pij-bravo", "copilot", S2),
    );
    let id = add(&mut w, "demo", &cli("pat"), "Evidence holder", &[]);
    w.svc
        .node_evidence_add(&ctx().actor(adder.clone()), &at(id), "docs/a.md#L3", None)
        .unwrap();
    w.svc
        .node_evidence_add(&ctx().actor(adder.clone()), &at(id), "docs/b.md#L1", None)
        .unwrap();
    fn how(w: &World, key: &str) -> Vec<Vec<&'static str>> {
        let listed = who(w, &ctx(), key).unwrap();
        listed.rows.iter().map(|r| r.how.clone()).collect()
    }
    assert_eq!(how(&w, S1), [vec!["edited", "evidence_added"]]);
    w.svc
        .node_evidence_remove(
            &ctx().actor(remover.clone()),
            &at(id),
            "docs/a.md#L3",
            Some("stale"),
            None,
        )
        .unwrap();
    // The adder's edit stamp is replaced, but both its pointers still name it: one live, one inside the trail.
    assert_eq!(how(&w, S1), [vec!["evidence_added"]]);
    assert_eq!(how(&w, S2), [vec!["edited", "evidence_removed"]]);
    assert_eq!(how(&w, "session/cli/pat"), [vec!["authored"]]);
    w.svc
        .node_evidence_remove(
            &ctx().actor(remover),
            &at(id),
            "docs/b.md#L1",
            Some("stale"),
            None,
        )
        .unwrap();
    assert_eq!(
        how(&w, S1),
        [vec!["evidence_added"]],
        "only the trail names the adder now"
    );
}

/// Graph-storage-expert N1 (ws008 TM6): a status move's pointer names its adder in
/// `status_history[i].evidence.by_session` (evidence added); once a redact scrubs it, that slot names the redactor
/// (evidence removed, graph-storage-expert's follow-up).
#[test]
fn status_move_evidence_names_its_adder_and_then_its_redactor() {
    let mut w = world();
    let (author, mover, redactor) = (
        cli("pat"),
        seat("pij-alpha", "copilot", S1),
        seat("pij-bravo", "copilot", S2),
    );
    let id = add(&mut w, "demo", &author, "Status holder", &[]);
    w.svc
        .node_status_with(
            &ctx().actor(mover.clone()),
            &at(id),
            umx_services::NodeStatusRequest {
                status: "agreed".into(),
                reason: Some("reviewed".into()),
                label: None,
                anchor: None,
                evidence: Some("docs/minutes.md#L4".into()),
                evidence_kind: None,
            },
        )
        .unwrap();
    // Another session edits last, so only the move's pointer can name the mover.
    w.svc
        .node_note(&ctx().actor(author.clone()), &at(id), "later", &[], None)
        .unwrap();
    let how = |w: &World, key: &str| {
        who(w, &ctx(), key)
            .map(|l| l.rows.iter().map(|r| r.how.clone()).collect::<Vec<_>>())
            .unwrap_or_default()
    };
    assert_eq!(how(&w, S1), [vec!["evidence_added"]]);
    // Review F3: the same move with its key written escaped (a hand edit) is still read; a stored
    // `redacted: false` (graph-storage-expert) is still a pointer, so still the adder's.
    let dir = GraphDir::parse("/work/g").unwrap();
    let nodes = String::from_utf8(w.files.read(&dir, GraphFile::Nodes).unwrap().unwrap()).unwrap();
    assert_eq!(
        nodes.matches("\"evidence\":{").count(),
        1,
        "one move cites a pointer"
    );
    let escaped = nodes.replace("\"evidence\":{", "\"\\u0065vidence\":{\"redacted\":false,");
    w.files
        .tamper_for_tests(&dir, GraphFile::Nodes, escaped.as_bytes());
    w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
    assert_eq!(how(&w, S1), [vec!["evidence_added"]]);
    // Only `redacted: true` is a redacted entry (review B1): a `null` marker, and one that is not a boolean at all
    // (a shape the typed read refuses, read as values), are still the adder's pointer.
    for marker in ["null", "\"no\""] {
        let odd = escaped.replace("\"redacted\":false,", &format!("\"redacted\":{marker},"));
        w.files
            .tamper_for_tests(&dir, GraphFile::Nodes, odd.as_bytes());
        w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
        assert_eq!(how(&w, S1), [vec!["evidence_added"]], "redacted: {marker}");
    }
    w.svc
        .node_evidence_redact(
            &ctx().actor(redactor),
            &at(id),
            "docs/minutes.md#L4",
            Some("it held a token"),
        )
        .unwrap();
    assert_eq!(how(&w, S2), [vec!["edited", "evidence_removed"]]);
    assert!(
        how(&w, S1).is_empty(),
        "the scrubbed pointer no longer names its adder"
    );
}

/// Review round 2: a status move or its pointer written as an array (a hand edit) names no session. serde
/// would read a derived struct from an array positionally, inventing `by_session` and `redacted`; the read takes
/// objects only, as the value read does.
#[test]
fn array_shaped_status_moves_attribute_nothing() {
    let mut w = world();
    let (author, mover) = (cli("pat"), seat("pij-alpha", "copilot", S1));
    let id = add(&mut w, "demo", &author, "Status holder", &[]);
    w.svc
        .node_status_with(
            &ctx().actor(mover),
            &at(id),
            umx_services::NodeStatusRequest {
                status: "agreed".into(),
                reason: Some("reviewed".into()),
                label: None,
                anchor: None,
                evidence: Some("docs/minutes.md#L4".into()),
                evidence_kind: None,
            },
        )
        .unwrap();
    w.svc
        .node_note(&ctx().actor(author), &at(id), "later", &[], None)
        .unwrap();
    let how = |w: &World| {
        who(w, &ctx(), S1)
            .unwrap()
            .rows
            .iter()
            .map(|r| r.how.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(how(&w), [vec!["evidence_added"]], "positive control");
    let dir = GraphDir::parse("/work/g").unwrap();
    let read = |w: &World| {
        umx_core::codec::decode_nodes(&w.files.read(&dir, GraphFile::Nodes).unwrap().unwrap())
            .unwrap()
            .records
    };
    let original = read(&w);
    let session = w
        .svc
        .loaded_graph("demo")
        .unwrap()
        .provenance_node(&session_key("copilot", S1))
        .unwrap()
        .id()
        .to_text();
    for (name, shape) in [
        (
            "an evidence array",
            serde_json::json!({ "to": "agreed", "evidence": [session, true] }),
        ),
        (
            "a move array",
            serde_json::json!([{ "by_session": session, "redacted": false }]),
        ),
    ] {
        let mut nodes = original.clone();
        let holder = nodes.iter_mut().find(|n| n.id == id).unwrap();
        holder.status_history = vec![shape];
        w.files.tamper_for_tests(
            &dir,
            GraphFile::Nodes,
            &umx_core::codec::encode_nodes(&nodes),
        );
        w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
        assert!(how(&w).is_empty(), "{name}: {:?}", how(&w));
    }
}

/// Review, graph-addressing-expert's ruling: a merge can leave two session (or agent) nodes with one key in one graph.
/// Both are that key: who K lists what either stamped, and node who and who agree.
#[test]
fn every_node_holding_a_key_in_one_graph_is_joined() {
    let mut w = world();
    let alpha = seat("pij-alpha", "copilot", S1);
    add(&mut w, "demo", &alpha, "By the first session node", &[]);
    let dir = GraphDir::parse("/work/g").unwrap();
    let read = |file| w.files.read(&dir, file).unwrap().unwrap();
    let mut nodes = umx_core::codec::decode_nodes(&read(GraphFile::Nodes))
        .unwrap()
        .records;
    let mut edges = umx_core::codec::decode_edges(&read(GraphFile::Edges))
        .unwrap()
        .records;
    let key = |n: &umx_core::Node| {
        n.fields
            .get("prov_key")
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    let session = nodes
        .iter()
        .find(|n| key(n).as_deref() == Some(session_key("copilot", S1).as_str()))
        .unwrap()
        .clone();
    let agent = nodes
        .iter()
        .find(|n| key(n).as_deref() == Some("agent/pij/pij-alpha"))
        .unwrap()
        .clone();
    let content = nodes.iter().find(|n| n.kind == "problem").unwrap().clone();
    let link = edges
        .iter()
        .find(|e| e.kind == "session_of")
        .unwrap()
        .clone();
    let high = |n: u128| Id((0x7f00_u128 << 112) | n);
    // The second holder of the session key stamps one node; a third session links to a second holder of the agent
    // key and stamps another.
    let (s2, a2, s3) = (high(1), high(2), high(3));
    let mut copy = |from: &umx_core::Node, id: Id, prov: Option<String>| {
        let mut n = from.clone();
        n.id = id;
        if let Some(prov) = prov {
            n.fields.insert("prov_key".into(), serde_json::json!(prov));
        }
        nodes.push(n);
    };
    copy(&session, s2, None);
    copy(&agent, a2, None);
    copy(
        &session,
        s3,
        Some(session_key("copilot", "h-4444444444444444")),
    );
    for (n, title, stamp) in [
        (4, "By the second session node", s2),
        (5, "By a session of the second agent node", s3),
    ] {
        let mut c = content.clone();
        c.id = high(n);
        c.title = title.into();
        c.authored_by_session = Some(stamp);
        c.edited_by_session = Some(stamp);
        nodes.push(c);
    }
    for (n, from, to) in [(6, s2, agent.id), (7, s3, a2)] {
        let mut e = link.clone();
        e.id = high(n);
        e.from = from;
        e.to = to;
        edges.push(e);
    }
    nodes.sort_by_key(|n| n.id);
    edges.sort_by_key(|e| e.id);
    w.files.tamper_for_tests(
        &dir,
        GraphFile::Nodes,
        &umx_core::codec::encode_nodes(&nodes),
    );
    w.files.tamper_for_tests(
        &dir,
        GraphFile::Edges,
        &umx_core::codec::encode_edges(&edges),
    );
    let loaded = w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
    // Load names each pair (the later id and the one in use); check lists each later id (graph-storage-expert d).
    let mut pairs: Vec<(String, String)> = loaded
        .notes
        .iter()
        .filter(|p| p.code == "duplicate_prov_key")
        .map(|p| (slot(p, "id"), slot(p, "in_use")))
        .collect();
    pairs.sort();
    assert_eq!(
        pairs,
        [
            (s2.to_text(), session.id.to_text()),
            (a2.to_text(), agent.id.to_text())
        ]
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
    );
    let mut checked: Vec<String> = w
        .svc
        .check(&ctx().graph("demo"), None)
        .unwrap()
        .value
        .page
        .rows
        .iter()
        .filter(|r| r.kind == "duplicate_prov_key")
        .map(|r| r.subject.clone())
        .collect();
    checked.sort();
    let mut expected = vec![s2.to_text(), a2.to_text()];
    expected.sort();
    assert_eq!(checked, expected);
    let titles = |key: &str| {
        let listed = who(&w, &ctx(), key).unwrap();
        rows(&listed).into_iter().map(|r| r.2).collect::<Vec<_>>()
    };
    assert_eq!(
        titles(&session_key("copilot", S1)),
        ["By the first session node", "By the second session node"]
    );
    assert_eq!(
        titles("agent/pij/pij-alpha"),
        [
            "By the first session node",
            "By the second session node",
            "By a session of the second agent node"
        ]
    );
    let stamp = &w.svc.node_who(&ctx(), &at(high(4))).unwrap().value.stamps[0];
    assert_eq!(stamp.state, StampState::Recorded);
    assert_eq!(
        stamp.session.as_ref().unwrap().key,
        session_key("copilot", S1)
    );
}

#[test]
fn node_who_names_the_session_agent_role_model_and_harness_in_its_own_graph() {
    let mut w = world();
    let alpha = seat("pij-alpha", "copilot", S1);
    let id = add(&mut w, "demo", &alpha, "Made", &[]);
    w.svc
        .node_note(
            &ctx().actor(seat("pij-bravo", "claude", S2)),
            &at(id),
            "later",
            &[],
            None,
        )
        .unwrap();
    let graph = w.svc.loaded_graph("demo").unwrap();
    let node_of = |key: &str| graph.provenance_node(key).unwrap().id();
    let done = w.svc.node_who(&ctx(), &at(id)).unwrap().value;
    assert_eq!((done.alias.as_str(), done.node), ("demo", id));
    let [authored, edited] = [&done.stamps[0], &done.stamps[1]];
    assert_eq!((authored.stamp, edited.stamp), ("authored", "edited"));
    for (row, seat_key, session, harness) in [
        (
            authored,
            "agent/pij/pij-alpha",
            session_key("copilot", S1),
            "copilot",
        ),
        (
            edited,
            "agent/pij/pij-bravo",
            session_key("claude", S2),
            "claude",
        ),
    ] {
        assert_eq!(row.state, StampState::Recorded);
        let s = row.session.as_ref().unwrap();
        assert_eq!(
            (s.key.as_str(), s.id),
            (session.as_str(), node_of(&session))
        );
        let a = row.agent.as_ref().unwrap();
        assert_eq!((a.key.as_str(), a.id), (seat_key, node_of(seat_key)));
        assert_eq!(row.role.as_deref(), Some("coder"));
        // The session keeps the model as given; its node is found by the normalised key (graph-addressing-expert fix
        // 1).
        assert_eq!(row.model.as_deref(), Some("Example/Model-5"));
        let m = row.model_node.as_ref().unwrap();
        assert_eq!(m.id, node_of("model/example-model-5"));
        assert_eq!(row.harness.as_deref(), Some(harness));
        let h = row.harness_node.as_ref().unwrap();
        assert_eq!(h.id, node_of(&format!("harness/{harness}")));
    }
    // A cli session has no harness (graph-addressing-expert fix 2); an older node has no stamps.
    let shell = add(&mut w, "demo", &cli("pat"), "Shell", &[]);
    let row = &w.svc.node_who(&ctx(), &at(shell)).unwrap().value.stamps[0];
    assert_eq!(row.agent.as_ref().unwrap().key, "agent/cli/pat");
    assert_eq!((row.harness.as_deref(), &row.harness_node), (None, &None));
    let older = add(&mut w, "demo", &cli("pat"), "Older", &[]);
    let dir = GraphDir::parse("/work/g").unwrap();
    let nodes = String::from_utf8(w.files.read(&dir, GraphFile::Nodes).unwrap().unwrap()).unwrap();
    let session = w
        .svc
        .loaded_graph("demo")
        .unwrap()
        .provenance_node("session/cli/pat")
        .unwrap()
        .id();
    let bare: String = nodes
        .lines()
        .map(|line| {
            let line = if line.contains(&older.to_text()) {
                line.replace(&format!(",\"authored_by_session\":\"{session}\""), "")
                    .replace(&format!(",\"edited_by_session\":\"{session}\""), "")
            } else {
                line.to_string()
            };
            format!("{line}\n")
        })
        .collect();
    assert_ne!(bare, nodes, "the stamp was removed");
    w.files
        .tamper_for_tests(&dir, GraphFile::Nodes, bare.as_bytes());
    w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
    let stamps = w.svc.node_who(&ctx(), &at(older)).unwrap().value.stamps;
    assert_eq!(
        (stamps[0].state, stamps[1].state),
        (StampState::None, StampState::None)
    );
    // node who resolves content only: a provenance node is not found, whatever the request's flag.
    let session = at(session);
    for flag in [false, true] {
        let err = w
            .svc
            .node_who(&ctx().with_provenance(flag), &session)
            .unwrap_err();
        assert_eq!(err.code, "node_not_found", "flag {flag}");
    }
}

/// Graph-storage's report row for plan 188: `umx who`'s whole read (the shared read hold) at 100k content nodes and
/// 10k sessions under 100 agents: with no evidence, with one stamped pointer on every node, and with one status move
/// citing a stamped pointer on every node (every history decoded). Report only; over
/// 100 ms becomes a follow-up. Run with `--release -- --ignored --nocapture`.
#[test]
#[ignore = "timing report, run on demand in release"]
fn who_read_hold_at_100k_nodes_and_10k_sessions() {
    use std::time::Instant;
    use umx_core::{Edge, Node};
    const CONTENT: u128 = 100_000;
    const SESSIONS: u128 = 10_000;
    const AGENTS: u128 = 100;
    let id = |n: u128| Id((0x0194_0000_0000_u128 << 80) | n);
    let record = |n: u128, kind: &str, title: String| Node {
        id: id(n),
        kind: kind.into(),
        title,
        status: "open".into(),
        note: String::new(),
        evidence: Vec::new(),
        status_history: Vec::new(),
        created: "2026-11-13T00:00:00Z".into(),
        updated: "2026-11-13T00:00:00Z".into(),
        created_by: "bench".into(),
        locator: None,
        authored_by_session: None,
        edited_by_session: None,
        fields: Default::default(),
    };
    let (agent_base, session_base) = (1_000_000, 2_000_000);
    for (evidence, moves) in [(false, false), (true, false), (false, true)] {
        let mut w = world();
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        for a in 0..AGENTS {
            let mut agent = record(agent_base + a, "agent", format!("pij-bench-{a}"));
            agent.fields.insert(
                "prov_key".into(),
                serde_json::json!(format!("agent/pij/pij-bench-{a}")),
            );
            nodes.push(agent);
        }
        for s in 0..SESSIONS {
            let mut session = record(session_base + s, "session", format!("session {s}"));
            session.fields.insert(
                "prov_key".into(),
                serde_json::json!(format!("session/copilot/h-{s:016x}")),
            );
            nodes.push(session);
            edges.push(Edge {
                id: id(3_000_000 + s),
                kind: "session_of".into(),
                from_graph: None,
                from: id(session_base + s),
                to_graph: None,
                to: id(agent_base + s % AGENTS),
                created: "2026-11-13T00:00:00Z".into(),
                created_by: "umx".into(),
                authored_by_session: None,
                edited_by_session: None,
                evidence: Vec::new(),
                fields: Default::default(),
            });
        }
        for n in 0..CONTENT {
            let mut node = record(10 + n, "problem", format!("Content {n}"));
            let session = id(session_base + n % SESSIONS);
            node.authored_by_session = Some(session);
            if evidence {
                node.evidence.push(serde_json::json!({
                    "source": "docs/a.md", "selector": "L1", "kind": null, "fidelity": "exact",
                    "hash": null, "by_session": session.to_text(),
                }));
            }
            if moves {
                // One status move citing a stamped pointer: every node's history is decoded (graph-storage-expert).
                node.status_history.push(serde_json::json!({
                    "at": "2026-11-13T00:00:00Z", "by": "bench", "from": "open", "to": "agreed",
                    "evidence": {
                        "source": "docs/a.md", "selector": "L2", "kind": null, "fidelity": "exact",
                        "hash": null, "by_session": session.to_text(),
                    },
                }));
            }
            nodes.push(node);
        }
        nodes.sort_by_key(|n| n.id);
        let dir = GraphDir::parse("/work/g").unwrap();
        w.files.tamper_for_tests(
            &dir,
            GraphFile::Nodes,
            &umx_core::codec::encode_nodes(&nodes),
        );
        w.files.tamper_for_tests(
            &dir,
            GraphFile::Edges,
            &umx_core::codec::encode_edges(&edges),
        );
        w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
        let demo = ctx().graph("demo");
        for (row, key) in [
            (
                "session (10 rows)",
                "session/copilot/h-0000000000000007".to_string(),
            ),
            (
                "agent (100 sessions, 1000 rows)",
                "agent/pij/pij-bench-7".to_string(),
            ),
        ] {
            let mut ms: Vec<f64> = (0..9)
                .map(|_| {
                    let start = Instant::now();
                    let done = w.svc.who(&demo, &key, 100).unwrap();
                    assert!(done.value.total > 0);
                    start.elapsed().as_secs_f64() * 1000.0
                })
                .collect();
            ms.sort_by(f64::total_cmp);
            println!(
                "who {row}, evidence {evidence}, moves {moves}: median {:.3} ms, max {:.3} ms",
                ms[4], ms[8]
            );
        }
    }
}

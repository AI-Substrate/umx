//! The graph view's agent channel over the fakes (ws009 S6a, plan 105): bounds, scope (d-20, d-21), resolution, the
//! ring's waits and resets, the waiter cap, ui_no_view and the selection rules.

use std::sync::Arc;
use umx_core::ports::GraphDir;
use umx_core::ui::{UiAction, UiCmd, VIEW_GONE_MS};
use umx_core::{Id, Problem};
use umx_fakes::{FakeClock, FakeGraphFiles, FakeIds, FakeLocalState};
use umx_services::ui::UiChannel;
use umx_services::{Ctx, NodeAddRequest, Ports, Services};

struct World {
    clock: Arc<FakeClock>,
    files: Arc<FakeGraphFiles>,
    svc: Services,
    ui: UiChannel,
}

fn world() -> World {
    let clock = Arc::new(FakeClock::default());
    let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let ports = Ports {
        sources: Arc::new(umx_fakes::FakeSourceReader::open(
            GraphDir::parse("/").unwrap(),
        )),
        files: files.clone(),
        vector_cache: Arc::new(FakeGraphFiles::new(GraphDir::parse("/cache").unwrap())),
        vector_cache_root: GraphDir::parse("/cache").unwrap(),
        clock: clock.clone(),
        ids: Arc::new(FakeIds::new()),
        state: Arc::new(FakeLocalState::new()),
        scopes: Arc::new(FakeLocalState::new()),
        registry: Arc::new(FakeLocalState::new().with_absent_ok(true)),
        allows: Arc::new(FakeLocalState::new()),
        lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
    };
    let svc = Services::new(ports);
    let ui = svc.graph().ui_channel();
    World {
        clock,
        files,
        svc,
        ui,
    }
}

#[test]
fn a_selected_node_that_becomes_provenance_disappears_from_the_selection() {
    use umx_core::codec::{self, GraphFile};
    use umx_core::ports::Change;
    use umx_core::setup::GraphFilesSetup;
    let mut w = world();
    w.graph("demo", false);
    let id = w.node("demo", "Selected content");
    w.wait(None, 0, &["demo"]);
    w.ui.select(w.svc.graph(), &ctx().graph("demo"), &[id.to_text()])
        .unwrap();
    assert_eq!(
        w.ui.selection(w.svc.graph(), &ctx()).unwrap().value.len(),
        1
    );
    let g = w.svc.loaded_graph("demo").unwrap();
    let mut records: Vec<_> = g.nodes().map(|n| n.to_node()).collect();
    let changed = records.iter_mut().find(|n| n.id == id).unwrap();
    changed.kind = "agent".into();
    changed.fields.insert(
        "prov_key".into(),
        serde_json::json!("agent/pij/pij-selected-seat"),
    );
    w.files
        .write_set(
            &GraphDir::parse("/work/demo").unwrap(),
            &[Change {
                file: GraphFile::Nodes.into(),
                bytes: Some(&codec::encode_nodes(&records)),
            }],
        )
        .unwrap();
    w.svc.graph_load(&ctx(), "/work/demo", None).unwrap();
    assert!(
        w.ui.selection(w.svc.graph(), &ctx())
            .unwrap()
            .value
            .is_empty()
    );
    assert_eq!(
        w.send(&ctx(), focus(&id.to_text())).unwrap_err().code,
        "node_not_found"
    );
}

fn ctx() -> Ctx {
    Ctx::new("tester")
}

impl World {
    fn graph(&mut self, alias: &str, secured: bool) {
        self.svc
            .graph_new(&ctx(), &format!("/work/{alias}"), alias, None, false, None)
            .unwrap();
        if secured {
            let given = [("policy".to_string(), "secured".to_string())];
            self.svc.graph_set(&ctx(), alias, &given).unwrap();
        }
    }

    fn node(&mut self, alias: &str, title: &str) -> Id {
        self.svc
            .node_add(
                &ctx().graph(alias),
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

    fn send(
        &mut self,
        ctx: &Ctx,
        action: UiAction<String>,
    ) -> Result<umx_core::ui::UiEvent, Problem> {
        let cmd = UiCmd { action, note: None };
        self.ui
            .command(self.svc.graph(), ctx, cmd, None)
            .map(|d| d.value)
    }

    /// A view's wait that answers now (a reset, events, or nothing), as the daemon's loop sees it on wake.
    fn wait(
        &mut self,
        epoch: Option<&str>,
        since: u64,
        graphs: &[&str],
    ) -> Option<umx_services::ui::UiWaited> {
        let graphs: Vec<String> = graphs.iter().map(|g| g.to_string()).collect();
        let w = self
            .ui
            .begin_wait(
                self.svc.graph(),
                epoch,
                since,
                1000,
                &graphs,
                &Default::default(),
                false,
            )
            .unwrap();
        let answer = self.ui.poll(&w);
        self.ui.end_wait(&w);
        answer
    }
}

fn clear() -> UiAction<String> {
    UiAction::Clear
}

fn focus(node: &str) -> UiAction<String> {
    UiAction::Focus {
        node: node.into(),
        hops: 1,
    }
}

#[test]
fn an_unknown_node_is_refused_with_its_next_and_nothing_is_kept() {
    let mut w = world();
    w.graph("demo", false);
    w.node("demo", "Approval delays");
    let err = w
        .send(&ctx(), focus("01ZZZZZZZZZZZZZZZZZZZZZZZZ"))
        .unwrap_err();
    assert_eq!(err.code, "node_not_found");
    assert!(
        err.slot("fix").is_some_and(|f| f.starts_with("umx ")),
        "{err:?}"
    );
    let err = w
        .send(
            &ctx(),
            UiAction::Highlight {
                nodes: vec!["nope".into()],
                add: false,
            },
        )
        .unwrap_err();
    assert_eq!(err.code, "node_not_found");
    let waited = w.wait(None, 0, &["demo"]).unwrap();
    assert_eq!(
        (waited.seq, waited.events.len()),
        (0, 0),
        "a refused command is never kept"
    );
}

#[test]
fn each_bound_and_the_note_are_refused_by_the_one_check() {
    let mut w = world();
    w.graph("demo", false);
    let id = w.node("demo", "x").to_text();
    let bad = [
        UiCmd {
            action: UiAction::Focus {
                node: id.clone(),
                hops: 4,
            },
            note: None,
        },
        UiCmd {
            action: UiAction::Highlight {
                nodes: vec![id.clone(); 101],
                add: false,
            },
            note: None,
        },
        UiCmd {
            action: UiAction::Filter {
                show: vec!["colour:red".into()],
                hide: Vec::new(),
            },
            note: None,
        },
        UiCmd {
            action: UiAction::Clear,
            note: Some("look \u{202E}here".into()),
        },
        UiCmd {
            action: UiAction::Clear,
            note: Some("x".repeat(201)),
        },
    ];
    for cmd in bad {
        let err =
            w.ui.command(w.svc.graph(), &ctx(), cmd.clone(), None)
                .unwrap_err();
        assert_eq!(err.code, "usage_invalid", "{cmd:?}");
    }
}

#[test]
fn a_secured_graph_is_used_only_when_named_and_otherwise_answers_as_unloaded() {
    let mut w = world();
    w.graph("vault", true);
    let id = w.node("vault", "secret plan").to_text();
    // Only a secured graph loaded: an unnamed command answers exactly as with nothing loaded.
    let unnamed = w.send(&ctx(), clear()).unwrap_err();
    let empty = world().send(&ctx(), clear()).unwrap_err();
    assert_eq!(format!("{unnamed:?}"), format!("{empty:?}"));
    // Named by --graph or by an address, it is used.
    assert_eq!(
        w.send(&ctx().graph("vault"), clear()).unwrap().graph,
        "vault"
    );
    let address = format!("umx:vault/{id}");
    assert_eq!(w.send(&ctx(), focus(&address)).unwrap().graph, "vault");
    // With an open graph too, the unnamed command takes the open one and ambiguity never lists the secured one.
    w.graph("open", false);
    assert_eq!(w.send(&ctx(), clear()).unwrap().graph, "open");
    w.graph("other", false);
    let err = w.send(&ctx(), clear()).unwrap_err();
    assert_eq!(err.code, "graph_ambiguous");
    let named: Vec<&str> = err.candidates.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(named, ["open", "other"]);
}

#[test]
fn a_wait_returns_only_events_for_the_graphs_it_names_and_seq_still_advances() {
    let mut w = world();
    w.graph("a", false);
    w.graph("b", false);
    let first = w.wait(None, 0, &["a"]).unwrap();
    assert!(first.reset, "a view with no epoch starts with a reset");
    let epoch = first.epoch.clone();
    w.send(&ctx().graph("a"), clear()).unwrap();
    w.send(&ctx().graph("b"), clear()).unwrap();
    let got = w.wait(Some(epoch.as_str()), first.seq, &["a"]).unwrap();
    assert!(!got.reset);
    let graphs: Vec<&str> = got.events.iter().map(|e| e.graph.as_str()).collect();
    assert_eq!(graphs, ["a"], "the event for b is not returned");
    assert_eq!(got.seq, 2, "seq counts b's event too");
    // Nothing new for a: no answer now (the daemon keeps waiting), and a timeout carries the latest seq.
    w.send(&ctx().graph("b"), clear()).unwrap();
    let graphs = vec!["a".to_string()];
    let waiter =
        w.ui.begin_wait(
            w.svc.graph(),
            Some(epoch.as_str()),
            got.seq,
            1000,
            &graphs,
            &Default::default(),
            false,
        )
        .unwrap();
    assert!(w.ui.poll(&waiter).is_none());
    let late = w.ui.timed_out(&waiter);
    w.ui.end_wait(&waiter);
    assert_eq!((late.seq, late.events.len(), late.reset), (3, 0, false));
    // A wait naming a graph that is not loaded is not refused (architect): followed from a revision it has
    // never had this epoch, it is overflow at 0; not followed, it gets nothing.
    let revs = [("gone".to_string(), 3)].into();
    let gone =
        w.ui.begin_wait(
            w.svc.graph(),
            Some(epoch.as_str()),
            got.seq,
            1000,
            &["gone".to_string()],
            &revs,
            false,
        )
        .unwrap();
    let answer = w.ui.poll(&gone).unwrap();
    w.ui.end_wait(&gone);
    assert_eq!(
        answer.changes,
        [umx_core::ui::Change::Overflow {
            graph: "gone".into(),
            rev: 0
        }]
    );
}

#[test]
fn another_epoch_or_a_since_older_than_the_ring_resets_with_every_kept_event() {
    let mut w = world();
    w.graph("a", false);
    let epoch = w.wait(None, 0, &["a"]).unwrap().epoch;
    assert_eq!(epoch.len(), 16);
    w.send(&ctx(), clear()).unwrap();
    let other = w.wait(Some("0000000000000000"), 1, &["a"]).unwrap();
    assert!(other.reset);
    assert_eq!(other.events.len(), 1);
    for _ in 0..300 {
        w.send(&ctx(), clear()).unwrap();
    }
    let behind = w.wait(Some(epoch.as_str()), 1, &["a"]).unwrap();
    assert!(behind.reset, "events 2 to 45 are gone");
    assert_eq!((behind.seq, behind.events.len()), (301, 256));
    let ahead = w.wait(Some(epoch.as_str()), 999, &["a"]).unwrap();
    assert!(ahead.reset);
    let current = w.wait(Some(epoch.as_str()), 300, &["a"]).unwrap();
    assert!(!current.reset);
    assert_eq!(current.events.len(), 1);
    // A second channel (a restarted daemon) has another epoch.
    assert_ne!(w.svc.graph().ui_channel().epoch(), epoch);
}

#[test]
fn a_fifth_waiter_is_refused_and_a_closed_channel_answers_at_once() {
    let mut w = world();
    w.graph("a", false);
    let graphs = vec!["a".to_string()];
    let waiters: Vec<_> = (0..4)
        .map(|_| {
            w.ui.begin_wait(
                w.svc.graph(),
                None,
                0,
                25_000,
                &graphs,
                &Default::default(),
                false,
            )
            .unwrap()
        })
        .collect();
    let err =
        w.ui.begin_wait(
            w.svc.graph(),
            None,
            0,
            25_000,
            &graphs,
            &Default::default(),
            false,
        )
        .unwrap_err();
    assert_eq!(err.code, "ui_waiters_busy");
    w.ui.end_wait(&waiters[0]);
    let fifth = w.ui.begin_wait(
        w.svc.graph(),
        None,
        0,
        25_000,
        &graphs,
        &Default::default(),
        false,
    );
    assert!(fifth.is_ok(), "a slot frees when a wait ends");
    let epoch = w.ui.epoch();
    let quiet = w.ui.begin_wait(
        w.svc.graph(),
        Some(epoch.as_str()),
        0,
        25_000,
        &[],
        &Default::default(),
        false,
    );
    assert_eq!(quiet.unwrap_err().code, "ui_waiters_busy");
    let err =
        w.ui.begin_wait(
            w.svc.graph(),
            None,
            0,
            25_001,
            &graphs,
            &Default::default(),
            false,
        )
        .unwrap_err();
    assert_eq!(err.code, "usage_invalid");
    for waiter in &waiters {
        w.ui.end_wait(waiter);
    }
    drop(waiters);
    let waiter =
        w.ui.begin_wait(
            w.svc.graph(),
            Some(epoch.as_str()),
            0,
            25_000,
            &graphs,
            &Default::default(),
            false,
        )
        .unwrap();
    assert!(w.ui.poll(&waiter).is_none());
    w.ui.close();
    let closed = w.ui.poll(&waiter).unwrap();
    assert_eq!((closed.reset, closed.events.len()), (false, 0));
}

/// ws009 M1b2: `umx ui`'s supervisor waits apart from the views (one at a time) and is never an open view, so
/// ui_no_view still says no view is open, with the next that brings one up: `--raise` while the supervisor runs, else
/// `umx ui --detach`. An open with --raise and the supervisor up warns nothing: the supervisor opens a tab.
#[test]
fn a_supervisor_wait_is_counted_apart_and_ui_no_view_names_what_brings_a_view_up() {
    let mut w = world();
    w.graph("alpha", false);
    w.graph("beta", false);
    let send = |w: &mut World, raise: bool| {
        let cmd = UiCmd {
            action: UiAction::Open {
                graphs: vec!["alpha".into(), "beta".into()],
                only: false,
                left_out: 0,
                raise,
            },
            note: None,
        };
        let done = w.ui.open(w.svc.graph(), cmd, None).unwrap();
        done.notes
            .iter()
            .map(|p| (p.code, p.slot("fix").unwrap_or_default()))
            .collect::<Vec<_>>()
    };
    let detach = vec![(
        "ui_no_view",
        "umx ui --detach --graph alpha,beta".to_string(),
    )];
    assert_eq!(send(&mut w, true), detach, "nothing runs: start one");
    let none: Vec<String> = Vec::new();
    let sup =
        w.ui.begin_wait(
            w.svc.graph(),
            None,
            0,
            25_000,
            &none,
            &Default::default(),
            true,
        )
        .unwrap();
    let second = w.ui.begin_wait(
        w.svc.graph(),
        None,
        0,
        25_000,
        &none,
        &Default::default(),
        true,
    );
    assert_eq!(
        second.unwrap_err().code,
        "ui_waiters_busy",
        "one supervisor wait at a time"
    );
    assert!(!w.ui.view_open(), "a supervisor is not a view");
    let views: Vec<_> = (0..4)
        .map(|_| {
            w.ui.begin_wait(
                w.svc.graph(),
                None,
                0,
                25_000,
                &none,
                &Default::default(),
                false,
            )
            .unwrap()
        })
        .collect();
    for v in &views {
        w.ui.end_wait(v);
    }
    w.clock.advance(VIEW_GONE_MS);
    assert_eq!(
        send(&mut w, false),
        [(
            "ui_no_view",
            "umx ui open --graph alpha,beta --raise".to_string()
        )],
        "a supervisor but no tab: ask again with --raise"
    );
    assert!(send(&mut w, true).is_empty(), "the supervisor opens a tab");
    w.ui.end_wait(&sup);
    assert!(w.ui.supervisor_live(), "between two waits it is still live");
    w.clock.advance(VIEW_GONE_MS);
    assert_eq!(send(&mut w, true), detach, "gone 30 s: start one");
}

#[test]
fn a_command_with_no_view_for_30_s_warns_ui_no_view_and_is_still_kept() {
    let mut w = world();
    w.graph("a", false);
    let cmd = || UiCmd {
        action: UiAction::<String>::Clear,
        note: Some("for later".into()),
    };
    let done = w.ui.command(w.svc.graph(), &ctx(), cmd(), None).unwrap();
    let codes: Vec<&str> = done.notes.iter().map(|p| p.code).collect();
    assert_eq!(codes, ["ui_no_view"]);
    w.wait(None, 0, &["a"]);
    let done = w.ui.command(w.svc.graph(), &ctx(), cmd(), None).unwrap();
    assert!(done.notes.is_empty(), "a view waited just now");
    w.clock.advance(VIEW_GONE_MS);
    let done = w.ui.command(w.svc.graph(), &ctx(), cmd(), None).unwrap();
    assert_eq!(done.notes.len(), 1, "30 s with no wait: no view");
    let kept = w.wait(None, 0, &["a"]).unwrap();
    assert_eq!(kept.events.len(), 3);
    assert_eq!(kept.events[0].command.note.as_deref(), Some("for later"));
}

#[test]
fn highlight_resolves_every_node_to_its_id_and_replaces_unless_add() {
    let mut w = world();
    w.graph("a", false);
    let x = w.node("a", "first title");
    let y = w.node("a", "second title");
    let e = w
        .send(
            &ctx(),
            UiAction::Highlight {
                nodes: vec![x.to_text().to_lowercase(), format!("umx:a/{y}")],
                add: false,
            },
        )
        .unwrap();
    assert_eq!(
        e.command.action,
        UiAction::Highlight {
            nodes: vec![x, y],
            add: false
        },
        "each node resolved to its id; replace is the default"
    );
    let e = w
        .send(
            &ctx(),
            UiAction::Highlight {
                nodes: vec![y.to_text()],
                add: true,
            },
        )
        .unwrap();
    assert!(matches!(
        e.command.action,
        UiAction::Highlight { add: true, .. }
    ));
}

#[test]
fn selection_shows_open_graphs_counts_secured_ones_and_clears_with_the_view() {
    let mut w = world();
    w.graph("open", false);
    w.graph("vault", true);
    let a = w.node("open", "Visible title").to_text();
    let s1 = w.node("vault", "Hidden one").to_text();
    let s2 = w.node("vault", "Hidden two").to_text();
    w.wait(None, 0, &["open", "vault"]);
    // The view must name the graph of a selection.
    let err =
        w.ui.select(w.svc.graph(), &ctx(), std::slice::from_ref(&a))
            .unwrap_err();
    assert_eq!(err.code, "usage_invalid");
    let picked =
        w.ui.select(w.svc.graph(), &ctx().graph("open"), &[a.clone(), a.clone()])
            .unwrap();
    assert_eq!(picked.value.nodes, 1, "a node selected twice counts once");
    w.ui.select(w.svc.graph(), &ctx().graph("vault"), &[s1, s2])
        .unwrap();
    let unnamed = w.ui.selection(w.svc.graph(), &ctx()).unwrap();
    let titles: Vec<&str> = unnamed.value.iter().map(|r| r.title.as_str()).collect();
    assert_eq!(titles, ["Visible title"]);
    let codes: Vec<&str> = unnamed.notes.iter().map(|p| p.code).collect();
    assert_eq!(codes, ["ui_selection_hidden"]);
    assert_eq!(unnamed.notes[0].slot("n").as_deref(), Some("2"));
    let named =
        w.ui.selection(w.svc.graph(), &ctx().graph("vault"))
            .unwrap();
    assert_eq!(named.value.len(), 2);
    assert!(named.notes.is_empty());
    // An empty selection clears that graph's.
    w.ui.select(w.svc.graph(), &ctx().graph("vault"), &[])
        .unwrap();
    assert!(
        w.ui.selection(w.svc.graph(), &ctx())
            .unwrap()
            .notes
            .is_empty()
    );
    // 30 s with no wait: the view is gone and so is the selection.
    w.clock.advance(VIEW_GONE_MS);
    assert!(
        w.ui.selection(w.svc.graph(), &ctx().graph("open"))
            .unwrap()
            .value
            .is_empty()
    );
}

#[test]
fn a_long_title_is_cut_in_a_selection_row() {
    let mut w = world();
    w.graph("a", false);
    let long = "t".repeat(500);
    let id = w.node("a", &long).to_text();
    w.wait(None, 0, &["a"]);
    w.ui.select(w.svc.graph(), &ctx().graph("a"), &[id])
        .unwrap();
    let rows = w.ui.selection(w.svc.graph(), &ctx()).unwrap().value;
    assert_eq!(rows[0].title.chars().count(), umx_services::TITLE_CHARS);
    assert!(rows[0].cut);
}

/// R203-1: a view that comes back after a 30 s gap must not revive the selection that expired in the gap.
#[test]
fn selection_does_not_revive_after_a_view_gap() {
    let mut w = world();
    w.graph("a", false);
    let x = w.node("a", "Kept while the view lives").to_text();
    let rows = |w: &mut World| {
        w.ui.selection(w.svc.graph(), &ctx().graph("a"))
            .unwrap()
            .value
            .len()
    };
    w.wait(None, 0, &["a"]);
    w.ui.select(w.svc.graph(), &ctx().graph("a"), std::slice::from_ref(&x))
        .unwrap();
    // Control: a wait just before the deadline keeps it.
    w.clock.advance(VIEW_GONE_MS - 1);
    w.wait(None, 0, &["a"]);
    assert_eq!(rows(&mut w), 1);
    // Exactly VIEW_GONE_MS after the last wait the view is gone; a new wait, begun and ended before any read,
    // must find the old selection expired.
    w.clock.advance(VIEW_GONE_MS);
    w.wait(None, 0, &["a"]);
    assert_eq!(rows(&mut w), 0, "the old selection came back");
    // A selection published by the returning view is read as usual.
    w.ui.select(w.svc.graph(), &ctx().graph("a"), &[x]).unwrap();
    assert_eq!(rows(&mut w), 1);
}

/// R203-2: once the channel closes (a stop or restart), every wait answers empty, whatever is queued or owed.
#[test]
fn quiesce_discards_pending_events_for_wait_replies() {
    let graphs = vec!["a".to_string()];
    let ready = || {
        let mut w = world();
        w.graph("a", false);
        let epoch = w.ui.epoch();
        (w, epoch)
    };
    // Control: a live channel delivers the queued event, and a reset carries the backlog.
    let (mut w, epoch) = ready();
    let waiter =
        w.ui.begin_wait(
            w.svc.graph(),
            Some(epoch.as_str()),
            0,
            1000,
            &graphs,
            &Default::default(),
            false,
        )
        .unwrap();
    w.send(&ctx(), clear()).unwrap();
    assert_eq!(w.ui.poll(&waiter).unwrap().events.len(), 1);
    let fresh =
        w.ui.begin_wait(
            w.svc.graph(),
            None,
            0,
            1000,
            &graphs,
            &Default::default(),
            false,
        )
        .unwrap();
    let reset = w.ui.poll(&fresh).unwrap();
    assert!(reset.reset && reset.events.len() == 1);
    // Closed: a current-epoch waiter with an event queued, no epoch, a wrong epoch, and a cursor ahead of a
    // nonempty ring each get an immediate empty reply.
    let owed: [(Option<&str>, u64); 4] = [
        (Some(epoch.as_str()), 0),
        (None, 0),
        (Some("0000000000000000"), 0),
        (Some(epoch.as_str()), 999),
    ];
    for (epoch_given, since) in owed {
        let (mut w, epoch) = ready();
        let epoch_given = epoch_given.map(|e| {
            if e == "0000000000000000" {
                e.to_string()
            } else {
                epoch.clone()
            }
        });
        let waiter =
            w.ui.begin_wait(
                w.svc.graph(),
                epoch_given.as_deref(),
                since,
                1000,
                &graphs,
                &Default::default(),
                false,
            )
            .unwrap();
        w.send(&ctx(), clear()).unwrap();
        w.ui.close();
        let answer =
            w.ui.poll(&waiter)
                .expect("a closed channel answers at once");
        assert_eq!(
            (answer.reset, answer.events.len()),
            (false, 0),
            "{epoch_given:?} since {since}"
        );
    }
}

#[test]
fn an_unloaded_graphs_selection_is_omitted_and_not_counted() {
    let mut w = world();
    w.graph("a", false);
    w.graph("b", false);
    let x = w.node("b", "In a graph about to go").to_text();
    w.wait(None, 0, &["a", "b"]);
    w.ui.select(w.svc.graph(), &ctx().graph("b"), &[x]).unwrap();
    assert_eq!(
        w.ui.selection(w.svc.graph(), &ctx()).unwrap().value.len(),
        1
    );
    w.svc.graph_unload(&ctx(), "b").unwrap();
    let after = w.ui.selection(w.svc.graph(), &ctx()).unwrap();
    assert!(after.value.is_empty() && after.notes.is_empty());
}

fn open(
    w: &mut World,
    graphs: &[&str],
    only: bool,
) -> Result<umx_services::Done<umx_services::ui::UiOpened>, Vec<Problem>> {
    let cmd = UiCmd {
        action: UiAction::Open {
            graphs: graphs.iter().map(|g| g.to_string()).collect(),
            only,
            left_out: 0,
            raise: false,
        },
        note: None,
    };
    w.ui.open(w.svc.graph(), cmd, None)
}

#[test]
fn ui_open_refuses_a_secured_graph_even_loaded_and_keeps_only_accepted_graphs() {
    let mut w = world();
    w.graph("alpha", false);
    w.graph("beta", false);
    w.graph("vault", true);
    w.graph("gone", false);
    w.svc.graph_unload(&ctx(), "gone").unwrap();
    let epoch = w.wait(None, 0, &["alpha"]).unwrap().epoch;
    // S1: a loaded secured graph is refused, however the command reaches the channel.
    let err = open(&mut w, &["vault"], false).unwrap_err();
    assert_eq!(err.len(), 1);
    assert_eq!(err[0].code, "ui_open_refused");
    // A mix: the event names only the accepted graphs, under the first (S2); the rest are warnings with their kind.
    let done = open(&mut w, &["vault", "beta", "ghost", "alpha", "gone"], true).unwrap();
    let opened = &done.value;
    assert_eq!(opened.event.graph, "beta");
    assert_eq!(
        opened.event.command.action,
        UiAction::Open {
            graphs: vec!["beta".into(), "alpha".into()],
            only: true,
            left_out: 3,
            raise: false
        }
    );
    assert_eq!(
        opened.refused,
        [
            ("vault".to_string(), "secured"),
            ("ghost".to_string(), "unknown"),
            ("gone".to_string(), "unloaded")
        ]
    );
    let codes: Vec<&str> = done.notes.iter().map(|p| p.code).collect();
    assert_eq!(codes, ["ui_open_left_out"; 3]);
    // Every view gets the open (it changes the open set), and the secured alias never enters the ring.
    let got = w.wait(Some(epoch.as_str()), 0, &["gamma"]).unwrap();
    let kept = format!("{:?}", got.events);
    assert_eq!(got.events.len(), 1, "{kept}");
    assert!(!kept.contains("vault") && !kept.contains("ghost"), "{kept}");
    // None accepted refuses the whole command with every graph's refusal, in order, and keeps nothing.
    let err = open(&mut w, &["ghost", "vault", "gone"], false).unwrap_err();
    let refused: Vec<(&str, String)> = err
        .iter()
        .map(|p| (p.code, format!("{:?}", p.slots)))
        .collect();
    assert_eq!(refused.len(), 3, "{refused:?}");
    assert!(
        refused.iter().all(|(c, _)| *c == "ui_open_refused"),
        "{refused:?}"
    );
    for (at, alias) in ["ghost", "vault", "gone"].iter().enumerate() {
        assert!(refused[at].1.contains(alias), "{refused:?}");
    }
    let after = w.wait(Some(epoch.as_str()), got.seq, &["alpha"]);
    let events = after.map(|a| a.events.len());
    assert!(events.is_none_or(|n| n == 0), "{events:?}");
}

#[test]
fn ui_open_bounds_are_checked_before_any_graph() {
    let mut w = world();
    w.graph("alpha", false);
    let names: Vec<String> = (0..13).map(|i| format!("g{i}")).collect();
    let many: Vec<&str> = names.iter().map(String::as_str).collect();
    assert_eq!(
        open(&mut w, &many, false).unwrap_err()[0].code,
        "usage_invalid"
    );
    assert_eq!(
        open(&mut w, &[], false).unwrap_err()[0].code,
        "usage_invalid"
    );
    assert_eq!(
        open(&mut w, &["alpha", "alpha"], false).unwrap_err()[0].code,
        "usage_invalid"
    );
}

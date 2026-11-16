//! ws009 S6a: the graph view's wait on a real daemon. A wait holds no services lock, so a write lands while it
//! sleeps; a new event wakes it; a fifth wait is refused; a stop or restart answers every wait, reply written.

use super::*;
use umx_protocol::{Op, UiCmd};

fn build() -> Build {
    Build {
        version: "t".into(),
        commit: "t".into(),
        built: "t".into(),
    }
}

fn op(op: Op, graph: Option<&str>) -> Request {
    Request::op(op, graph.map(str::to_owned), None, build())
}

fn wait(timeout_ms: u64, since: u64, epoch: Option<String>) -> Request {
    op(
        Op::UiWait {
            epoch,
            since,
            timeout_ms,
            graphs: vec!["g".into()],
            revs: Default::default(),
            supervisor: false,
        },
        None,
    )
}

fn clear() -> Request {
    op(
        Op::UiCommand {
            command: UiCmd::Clear { note: None },
        },
        Some("g"),
    )
}

/// A daemon over real adapters in a fresh folder, with graph `g` and its epoch (from a first wait).
fn daemon(name: &str) -> (Arc<Daemon>, std::path::PathBuf, String) {
    let root = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("umx-s6a-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let config = Config::at(root.join("home"), Some(true)).roots([root.to_str().unwrap()]);
    let d = Daemon::new(&config, build());
    let made = d.handle(&op(
        Op::GraphNew {
            path: root.join("g").display().to_string(),
            alias: "g".into(),
            title: None,
            empty: false,
            embedding_model: None,
            secured: false,
            no_graphfile: false,
            graphfile: false,
        },
        None,
    ));
    assert!(made.errors.is_empty(), "{}", made.to_json());
    let first = d.handle(&wait(0, 0, None));
    let epoch = first.data["epoch"].as_str().unwrap().to_owned();
    (Arc::new(d), root, epoch)
}

fn timed(d: &Daemon, request: &Request) -> (Envelope, Duration) {
    let started = Instant::now();
    let env = d.handle(request);
    (env, started.elapsed())
}

#[test]
fn a_write_and_a_read_land_while_a_wait_sleeps_and_an_event_wakes_it() {
    let (d, root, epoch) = daemon("write");
    std::thread::scope(|s| {
        let waiting = s.spawn(|| timed(&d, &wait(20_000, 0, Some(epoch.clone()))));
        std::thread::sleep(Duration::from_millis(300));
        let add = op(
            Op::NodeAdd {
                on_key: None,
                raised_in: None,
                kind: "problem".into(),
                title: "Approval delays".into(),
                status: None,
                note: None,
                sets: Vec::new(),
                parent: None,
                evidence: Vec::new(),
                evidence_kind: None,
                locator: None,
            },
            Some("g"),
        );
        let (added, took) = timed(&d, &add);
        assert!(added.errors.is_empty(), "{}", added.to_json());
        assert!(took < Duration::from_secs(5), "the write waited {took:?}");
        let (listed, took) = timed(
            &d,
            &op(
                Op::GraphList {
                    all: false,
                    graphfile: None,
                },
                None,
            ),
        );
        assert!(listed.errors.is_empty() && took < Duration::from_secs(5));
        assert!(
            !waiting.is_finished(),
            "a write is not an event for the view"
        );
        let sent = d.handle(&clear());
        assert!(sent.errors.is_empty(), "{}", sent.to_json());
        assert!(sent.warnings.is_empty(), "a view is waiting: no ui_no_view");
        let (got, took) = waiting.join().unwrap();
        assert!(took < Duration::from_secs(10), "woke after {took:?}");
        assert_eq!(got.data["events"][0]["command"]["kind"], "clear");
        assert_eq!(got.data["seq"], 1);
    });
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_fifth_wait_is_refused_ui_waiters_busy() {
    let (d, root, epoch) = daemon("busy");
    std::thread::scope(|s| {
        let waits: Vec<_> = (0..4)
            .map(|_| s.spawn(|| d.handle(&wait(20_000, 0, Some(epoch.clone())))))
            .collect();
        let until = Instant::now() + Duration::from_secs(10);
        while d.ui.lock().unwrap().waiters() < 4 {
            assert!(Instant::now() < until, "the four waits never began");
            std::thread::sleep(Duration::from_millis(20));
        }
        let fifth = d.handle(&wait(20_000, 0, Some(epoch.clone())));
        assert_eq!(fifth.errors[0].code(), "ui_waiters_busy");
        d.handle(&clear());
        for w in waits {
            assert!(w.join().unwrap().errors.is_empty());
        }
    });
    let _ = fs::remove_dir_all(root);
}

/// F64: before a stop or restart every view's wait answers at once, empty, and its reply reaches the view over the
/// socket before the quiesce returns.
#[test]
fn a_restart_quiesce_answers_every_socket_wait_empty() {
    let (d, root, epoch) = daemon("quiesce");
    let socket = root.join("s.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let in_flight = Arc::new(AtomicUsize::new(0));
    let accepting = Arc::clone(&d);
    std::thread::spawn(move || accept(&accepting, listener, SLOTS, &in_flight));
    let ask = |request: Request| {
        let socket = socket.clone();
        std::thread::spawn(move || {
            let mut stream = UnixStream::connect(socket).unwrap();
            stream.write_all(request.to_json().as_bytes()).unwrap();
            stream.shutdown(Shutdown::Write).unwrap();
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).unwrap();
            Envelope::from_json(&bytes).unwrap()
        })
    };
    let views: Vec<_> = (0..2)
        .map(|_| ask(wait(20_000, 0, Some(epoch.clone()))))
        .collect();
    let until = Instant::now() + Duration::from_secs(10);
    while d.ui_replies.load(Ordering::SeqCst) < 2 {
        assert!(Instant::now() < until, "the waits never arrived");
        std::thread::sleep(Duration::from_millis(20));
    }
    let started = Instant::now();
    d.ui_quiesce();
    assert!(
        started.elapsed() < QUIESCE,
        "every reply written within the bound"
    );
    assert_eq!(d.ui_replies.load(Ordering::SeqCst), 0);
    for view in views {
        let got = view.join().unwrap();
        assert!(got.errors.is_empty(), "{}", got.to_json());
        assert_eq!(got.data["events"], json!([]));
        assert_eq!(got.data["reset"], false);
    }
    let late = d.handle(&wait(20_000, 0, Some(epoch)));
    assert_eq!(
        late.data["events"],
        json!([]),
        "a wait after the quiesce answers at once"
    );
    let _ = fs::remove_dir_all(root);
}

/// ws009 M1b2 (ui-expert N4): a view whose wait hung up frees its slot at once, not when the 25 s hold ends,
/// so a view that reconnects (as it does for each graph it opens) never meets ui_waiters_busy. A client that only
/// shut its write half still gets its reply, the probe's spaces before it.
#[test]
fn a_wait_whose_client_hung_up_ends_at_once_and_a_live_one_still_gets_its_reply() {
    let (d, root, epoch) = daemon("hangup");
    let socket = root.join("s.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let in_flight = Arc::new(AtomicUsize::new(0));
    let accepting = Arc::clone(&d);
    std::thread::spawn(move || accept(&accepting, listener, SLOTS, &in_flight));
    let waiters = |n: usize| {
        let until = Instant::now() + Duration::from_secs(10);
        while d.ui.lock().unwrap().waiters() != n {
            assert!(Instant::now() < until, "never {n} waits");
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    let begin = || {
        let mut stream = UnixStream::connect(&socket).unwrap();
        stream
            .write_all(wait(20_000, 0, Some(epoch.clone())).to_json().as_bytes())
            .unwrap();
        stream.shutdown(Shutdown::Write).unwrap();
        stream
    };
    // Twelve in a row, each hung up as a reconnecting view does: never more than one counted, never busy.
    for _ in 0..12 {
        let gone = begin();
        waiters(1);
        drop(gone);
        let dropped = Instant::now();
        waiters(0);
        assert!(
            dropped.elapsed() < Duration::from_secs(2),
            "dropped after {:?}",
            dropped.elapsed()
        );
    }
    // Four that hang up together and a fifth at once, before any slice has looked: the fifth wakes them to look, and
    // gets in.
    let gone: Vec<_> = (0..4).map(|_| begin()).collect();
    waiters(4);
    drop(gone);
    let fifth = d.handle(&wait(0, 0, Some(epoch.clone())));
    assert!(fifth.errors.is_empty(), "{}", fifth.to_json());
    waiters(0);
    let mut live = begin();
    waiters(1);
    std::thread::sleep(crate::ui_channel::HANGUP_CHECK * 3);
    assert_eq!(
        d.ui.lock().unwrap().waiters(),
        1,
        "a client that waits is not gone"
    );
    let sent = d.handle(&clear());
    assert!(
        sent.errors.is_empty() && sent.warnings.is_empty(),
        "{}",
        sent.to_json()
    );
    let mut bytes = Vec::new();
    live.read_to_end(&mut bytes).unwrap();
    assert!(bytes.starts_with(b" "), "the probe wrote before the reply");
    let got = Envelope::from_json(&bytes).unwrap();
    assert_eq!(got.data["events"][0]["command"]["kind"], "clear");
    waiters(0);
    let _ = fs::remove_dir_all(root);
}

/// Plan 129 (architect FU2, security N3): a `umx ui` command's event carries the sender the CLI claimed, checked
/// as for a write, in the shape the view's AgentActor parser reads (apps/ui lib/umx/socket.ts: seat, role, harness
/// and user as text). The session is always h-<16 hex> or absent, never raw; no claim, or a refused one, is no `by`.
#[test]
fn a_ui_command_event_carries_the_checked_sender_and_never_a_raw_session() {
    let (d, root, epoch) = daemon("by");
    let claim = |session: &str| umx_protocol::ActorClaim {
        tier: "pij".into(),
        harness: Some("copilot".into()),
        session: Some(session.into()),
        seat: Some("pij-calm-otter".into()),
        role: Some("coder".into()),
        model: Some("openai/GPT-5.6".into()),
        effort: None,
        user: Some("writer".into()),
    };
    let raw = "raw-session-0123456789";
    for actor in [Some(claim("h-0123456789abcdef")), None, Some(claim(raw))] {
        let mut sent = clear();
        sent.actor = actor;
        let env = d.handle(&sent);
        assert!(env.errors.is_empty(), "{}", env.to_json());
    }
    let waited = d.handle(&wait(0, 0, Some(epoch)));
    let events = waited.data["events"].as_array().unwrap();
    assert_eq!(events.len(), 3, "{}", waited.to_json());
    let by = events[0]["by"].as_object().expect("a checked sender");
    for (key, want) in [
        ("seat", "pij-calm-otter"),
        ("role", "coder"),
        ("harness", "copilot"),
        ("user", "writer"),
    ] {
        assert_eq!(by[key].as_str(), Some(want), "{key}: {by:?}");
    }
    let session = by["harness_session"].as_str().unwrap();
    assert!(
        session.len() == 18
            && session.starts_with("h-")
            && session[2..]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "{session}"
    );
    assert!(events[1].get("by").is_none(), "no claim: {}", events[1]);
    assert!(
        events[2].get("by").is_none(),
        "refused claim: {}",
        events[2]
    );
    assert!(!waited.to_json().contains(raw));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_raw_ui_open_naming_a_secured_graph_is_refused_and_never_kept() {
    let (d, root, epoch) = daemon("open");
    let made = d.handle(&op(
        Op::GraphNew {
            path: root.join("vault").display().to_string(),
            alias: "vault".into(),
            title: None,
            empty: false,
            embedding_model: None,
            secured: true,
            no_graphfile: false,
            graphfile: false,
        },
        None,
    ));
    assert!(made.errors.is_empty(), "{}", made.to_json());
    let open = |graphs: &[&str]| {
        op(
            Op::UiCommand {
                command: UiCmd::Open {
                    // A sender's count is never taken (the daemon counts what it refused).
                    left_out: 9,
                    graphs: graphs.iter().map(|g| (*g).to_owned()).collect(),
                    only: false,
                    note: None,
                    raise: false,
                },
            },
            None,
        )
    };
    // Bypassing the CLI: the daemon refuses the secured graph itself (S1); with every graph refused, each one's
    // refusal in order, no data (architect).
    let alone = d.handle(&open(&["vault", "ghost"]));
    let codes: Vec<(&str, &str)> = alone.errors.iter().map(|e| (e.code(), e.next())).collect();
    assert_eq!(
        codes,
        [
            ("ui_open_refused", "umx ui --graph vault"),
            ("ui_open_refused", "umx graph list --all")
        ],
        "{}",
        alone.to_json()
    );
    assert!(alone.data.is_null());
    let mixed = d.handle(&open(&["vault", "g"]));
    assert!(mixed.errors.is_empty(), "{}", mixed.to_json());
    assert_eq!(mixed.data["sent"], serde_json::json!(["g"]));
    assert_eq!(mixed.data["graph"], "g");
    assert_eq!(
        mixed.data["refused"],
        serde_json::json!([{"graph": "vault", "kind": "secured"}])
    );
    assert_eq!(mixed.data["command"]["left_out"], 1);
    // S2: the ring a view reads holds the accepted graph only, with the refused one counted.
    let got = d.handle(&wait(0, 0, Some(epoch)));
    let events = got.data["events"].to_string();
    assert!(
        events.contains("\"g\"") && !events.contains("vault") && !events.contains("ghost"),
        "{events}"
    );
    assert_eq!(got.data["events"][0]["command"]["left_out"], 1, "{events}");
    let _ = fs::remove_dir_all(&root);
}

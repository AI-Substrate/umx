//! The cli tests: `umx ui` and the view verbs.
//! A module of cli.rs, whose `Home` and helpers it uses, in its own file so no file nears the seed gate's
//! size cap.

use super::*;

#[test]
fn ui_lan_password_is_local_private_reused_and_rotated_without_a_daemon() {
    use std::os::unix::fs::PermissionsExt;
    let home = Home::new("ui-lan-password");
    let (first, exit) = home.umx(&["ui", "lan-password"]);
    assert_eq!(exit, 0, "{}", first.to_json());
    let password = first.data["password"].as_str().unwrap();
    assert_eq!(password.len(), 14);
    let path = first.data["path"].as_str().unwrap();
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let (again, exit) = home.umx(&["ui", "lan-password"]);
    assert_eq!(exit, 0);
    assert_eq!(again.data["password"], password);
    let (rotated, exit) = home.umx(&["ui", "lan-password", "--rotate"]);
    assert_eq!(exit, 0);
    assert_ne!(rotated.data["password"], password);
    let (bad, exit) = home.umx(&["ui", "lan-password", "--unknown"]);
    assert_eq!((exit, bad.errors[0].code()), (2, "usage_invalid"));
    let (lan, exit) = home.umx(&["ui", "--lan", "--no-open"]);
    assert_eq!((exit, lan.errors[0].code()), (3, "daemon_not_running"));
}

/// ws009 s2 (plan 095): `umx ui` refuses before it starts anything, each refusal with a next: the daemon down,
/// an alias that is not loaded, a folder that is missing or not built, and no node. `--no-open` with JSON is fine: the
/// URL holds no secret.
#[test]
fn ui_refuses_with_a_next_before_it_starts_anything() {
    let mut home = Home::new("ui");
    let (env, exit) = home.umx(&["ui"]);
    assert_eq!((exit, env.errors[0].code()), (3, "daemon_not_running"));
    assert_eq!(env.command, "ui");
    let (env, exit) = home.umx(&["ui", "--no-open"]);
    assert_eq!((exit, env.errors[0].code()), (3, "daemon_not_running"));
    let (env, exit) = home.umx(&["ui", "--port", "0"]);
    assert_eq!((exit, env.errors[0].code()), (2, "usage_invalid"));

    home.start();
    let (env, exit) = home.umx(&["ui", "--graph", "nope"]);
    assert_eq!((exit, env.errors[0].code()), (1, "graph_not_loaded"));
    let (env, exit) = home.umx_with(&["ui"], &[("UMX_UI_DIR", "/nonexistent/apps/ui")]);
    assert_eq!((exit, env.errors[0].code()), (1, "ui_dir_refused"));

    let app = home.path.join("app");
    fs::create_dir_all(app.join("bin")).unwrap();
    fs::write(app.join("bin/serve.mjs"), "").unwrap();
    let dir = app.display().to_string();
    let (env, exit) = home.umx_with(&["ui"], &[("UMX_UI_DIR", &dir)]);
    assert_eq!((exit, env.errors[0].code()), (1, "ui_not_built"));
    assert!(
        env.errors[0]
            .message()
            .contains("--frozen-lockfile && pnpm --dir"),
        "{}",
        env.errors[0].message()
    );
    assert_eq!(env.errors[0].next(), "umx docs get ui");

    fs::create_dir_all(app.join(".next/standalone")).unwrap();
    fs::write(app.join(".next/standalone/server.js"), "").unwrap();
    let (env, exit) = home.umx_with(&["ui"], &[("UMX_UI_DIR", &dir), ("PATH", "/nonexistent")]);
    assert_eq!((exit, env.errors[0].code()), (1, "ui_node_not_found"));
    assert_eq!(env.errors[0].next(), "umx docs get ui");
}

/// ws009 S2 follow-up (c), plan 129: `umx ui --graph` on a graph that unloaded when idle loads it on demand (F72) and
/// goes on to start the view; one unloaded on purpose, or never known, is refused with graph_not_loaded and a next
/// that works. The view's folder is missing here, so a preflight that passes ends at ui_dir_refused.
#[test]
fn ui_graph_loads_an_idle_remembered_graph_and_refuses_an_unloaded_one() {
    let mut home = Home::new("ui-idle");
    home.start_with(&["--idle-unload", "3s"]);
    let path = home.path.join("g").display().to_string();
    home.ok(&["graph", "new", &path, "--alias", "demo"]);
    home.wait_until_idle();
    let no_app = [("UMX_UI_DIR", "/nonexistent/apps/ui")];

    let (env, exit) = home.umx_with(&["ui", "--graph", "demo"], &no_app);
    assert_eq!(
        (exit, env.errors[0].code()),
        (1, "ui_dir_refused"),
        "{}",
        env.to_json()
    );
    // Loaded by the preflight's named read; it unloads again after 3 s idle, so look straight away.
    assert_eq!(home.aliases(), ["demo"], "loaded on demand");

    home.ok(&["graph", "unload", "demo"]);
    let (env, exit) = home.umx_with(&["ui", "--graph", "demo"], &no_app);
    assert_eq!((exit, env.command.as_str()), (1, "ui"), "{}", env.to_json());
    let refused = issue(&env, Code::GraphNotLoaded);
    assert!(
        refused.message().contains("it was unloaded"),
        "{}",
        refused.message()
    );
    assert!(
        refused.next().starts_with("umx graph load ") && refused.next().ends_with("--as demo"),
        "{}",
        refused.next()
    );
    assert!(
        home.aliases().is_empty(),
        "an unloaded graph stays unloaded"
    );

    let (env, exit) = home.umx_with(&["ui", "--graph", "nope"], &no_app);
    assert_eq!((exit, env.errors[0].code()), (1, "graph_not_loaded"));
    assert_eq!(env.errors[0].next(), "umx graph list");
}

/// ws009 S6a (plan 105): the agent verbs through the binary, with a raw wait standing in for the graph view.
#[test]
fn ui_verbs_reach_a_waiting_view_as_events_with_ids_and_notes() {
    let mut home = Home::new("ui-verbs");
    home.graph();
    let (a, _) = home.umx(&[
        "node",
        "add",
        "--type",
        "problem",
        "--title",
        "Approval delays",
    ]);
    let (b, _) = home.umx(&[
        "node",
        "add",
        "--type",
        "decision",
        "--title",
        "Launch date",
    ]);
    let a = a.data["node"]["id"].as_str().unwrap().to_owned();
    let b = b.data["node"]["id"].as_str().unwrap().to_owned();
    let first = raw(&home.socket(), &view_wait(0, &["demo"])).0;
    assert_eq!(first.data["reset"], true);
    let (epoch, since) = (
        first.data["epoch"].as_str().unwrap().to_owned(),
        first.data["seq"].as_u64().unwrap(),
    );
    let socket = home.socket();
    let waiting = {
        let epoch = epoch.clone();
        std::thread::spawn(move || {
            let op = umx_protocol::Op::UiWait {
                epoch: Some(epoch),
                since,
                timeout_ms: 20_000,
                graphs: vec!["demo".into()],
                revs: Default::default(),
                supervisor: false,
            };
            raw(&socket, &Request::op(op, None, None, older_cli())).0
        })
    };
    std::thread::sleep(Duration::from_millis(300));
    let address = format!("umx:demo/{a}");
    let (env, exit) = home.umx(&[
        "ui",
        "focus",
        &address,
        "--hops",
        "2",
        "--note",
        "start here",
    ]);
    assert_eq!(exit, 0, "{}", env.to_json());
    assert!(
        env.warnings.is_empty(),
        "a view is waiting: {}",
        env.to_json()
    );
    assert_eq!(
        env.data["command"]["node"],
        a.as_str(),
        "the address resolved to its id"
    );
    let got = waiting.join().unwrap();
    let event = &got.data["events"][0];
    assert_eq!(
        (
            event["graph"].as_str(),
            event["command"]["kind"].as_str(),
            event["command"]["hops"].as_u64()
        ),
        (Some("demo"), Some("focus"), Some(2))
    );
    assert_eq!(event["command"]["note"], "start here");
    assert!(
        !got.to_json().contains("Approval delays"),
        "no node text in events"
    );
    assert!(
        got.untrusted
            .iter()
            .any(|p| p == "data.events[].command.note")
    );

    // Each verb, its bounds and refusals.
    let lower = a.to_lowercase();
    for args in [
        vec!["ui", "highlight", lower.as_str(), b.as_str()],
        vec!["ui", "highlight", b.as_str(), "--add"],
        vec![
            "ui",
            "filter",
            "--hide",
            "relation:similar_to",
            "--show",
            "type:problem",
        ],
        vec!["ui", "fit"],
        vec!["ui", "clear", "--note", "done"],
    ] {
        let (env, exit) = home.umx(&args);
        assert_eq!(exit, 0, "{args:?}: {}", env.to_json());
    }
    let (env, exit) = home.umx(&["ui", "focus", "01ZZZZZZZZZZZZZZZZZZZZZZZZ"]);
    assert_eq!((exit, env.errors[0].code()), (1, "node_not_found"));
    assert!(env.errors[0].next().starts_with("umx "));
    let (env, exit) = home.umx(&["ui", "highlight", &a, "--note", "look \u{202E}here"]);
    assert_eq!((exit, env.errors[0].code()), (2, "usage_invalid"));
    let (env, _) = home.umx(&["ui", "focus", &a, "--hops", "4"]);
    assert_eq!(env.errors[0].code(), "usage_invalid");
    let (env, _) = home.umx(&["ui", "filter", "--hide", "colour:red"]);
    assert_eq!(env.errors[0].code(), "usage_invalid");
    let (env, exit) = home.umx(&["ui", "wait"]);
    assert_eq!(
        (exit, env.errors[0].code()),
        (2, "usage_invalid"),
        "only the view waits"
    );

    let caught = raw(&home.socket(), &{
        let op = umx_protocol::Op::UiWait {
            epoch: Some(epoch),
            since: got.data["seq"].as_u64().unwrap(),
            timeout_ms: 0,
            graphs: vec!["demo".into()],
            revs: Default::default(),
            supervisor: false,
        };
        Request::op(op, None, None, older_cli())
    })
    .0;
    let kinds: Vec<&str> = caught.data["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["command"]["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["highlight", "highlight", "filter", "fit", "clear"]);
    let events = &caught.data["events"];
    assert_eq!(
        events[0]["command"]["nodes"],
        json!([a, b]),
        "each node as its full id"
    );
    assert_eq!(events[0]["command"]["add"], false);
    assert_eq!(events[1]["command"]["add"], true);
    assert_eq!(events[2]["command"]["hide"], json!(["relation:similar_to"]));
    assert_eq!(events[2]["command"]["show"], json!(["type:problem"]));

    // The person's selection (S7): published by the view, read by the agent.
    raw(&home.socket(), &view_select("demo", &[&b]));
    let (env, exit) = home.umx(&["ui", "selection"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    assert_eq!(env.data["rows"][0]["title"], "Launch date");
    assert_eq!(env.data["rows"][0]["address"], format!("umx:demo/{b}"));
    assert!(env.untrusted.iter().any(|p| p == "data.rows[].title"));

    // `--select` (PROTOCOL 111): the event asks the view to select the node too; without it the key is absent.
    let (env, exit) = home.umx(&["ui", "focus", &a, "--select"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    assert_eq!(env.data["command"]["select"], true, "{}", env.to_json());
    let (env, _) = home.umx(&["ui", "focus", &a]);
    assert!(
        env.data["command"].get("select").is_none(),
        "{}",
        env.to_json()
    );
}

/// ws009 plan 179 (security, as F188): `umx ui selection` keeps each title and names one umx's secret check flags in
/// `secret_suspected` (["field:title"]), with this machine's allows; a plain title, or one allowed, has none and the
/// column shows only when a row has some. The flag names the field, never the value: the reply holds the title once.
#[test]
fn ui_selection_names_a_flagged_title_and_holds_it_once() {
    let mut home = Home::new("ui-selection-flag");
    home.graph();
    let id = |env: &Envelope| env.data["node"]["id"].as_str().unwrap().to_owned();
    let add = |home: &Home, title: &str| {
        let (env, exit) = home.umx(&["node", "add", "--type", "problem", "--title", title]);
        assert_eq!(exit, 0, "{}", env.to_json());
        env
    };
    let plain = id(&add(&home, "Launch date"));
    let flagged_env = add(&home, "Cache AKIAPLANTEDTITLE7F3A");
    assert!(
        issues(&flagged_env, Code::SecretSuspected).len() == 1,
        "positive control: the write warns: {}",
        flagged_env.to_json()
    );
    let flagged = id(&flagged_env);
    let allowed = id(&add(&home, "Box AKIAALLOWEDTITLE0001"));
    let (env, exit) = home.umx(&[
        "embeddings",
        "allow",
        "--node",
        &allowed,
        "--field",
        "title",
    ]);
    assert_eq!(
        (exit, &env.data["allowed"]),
        (0, &json!(true)),
        "{}",
        env.to_json()
    );

    raw(&home.socket(), &view_wait(0, &["demo"]));
    raw(&home.socket(), &view_select("demo", &[&plain]));
    let (env, exit) = home.umx(&["ui", "selection"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    assert!(env.data["rows"][0].get("secret_suspected").is_none());
    assert!(
        !env.data["columns"]
            .as_array()
            .unwrap()
            .contains(&json!("secret_suspected")),
        "no row flagged, no column: {}",
        env.to_json()
    );

    raw(
        &home.socket(),
        &view_select("demo", &[&plain, &flagged, &allowed]),
    );
    let (env, exit) = home.umx(&["ui", "selection"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    let rows = env.data["rows"].as_array().unwrap();
    let flags = |id: &str| {
        rows.iter()
            .find(|r| r["id"] == id)
            .unwrap()
            .get("secret_suspected")
            .cloned()
    };
    assert_eq!(
        flags(&flagged),
        Some(json!(["field:title"])),
        "{}",
        env.to_json()
    );
    assert_eq!(flags(&plain), None);
    assert_eq!(flags(&allowed), None, "an allowed title is not flagged");
    assert!(
        env.data["columns"]
            .as_array()
            .unwrap()
            .contains(&json!("secret_suspected"))
    );
    let text = env.to_json();
    assert_eq!(
        text.matches("AKIAPLANTEDTITLE7F3A").count(),
        1,
        "the title once, no second copy: {text}"
    );
    assert!(
        !text.contains("rule") && !text.contains("aws_access_key"),
        "no rule or hash: {text}"
    );

    let human = home.human(&["ui", "selection"]);
    assert!(
        human.contains("secret_suspected") && human.contains("field:title"),
        "{human}"
    );
    // F224: the human table hides the flagged title; the JSON above holds it once.
    assert!(
        !human.contains("AKIAPLANTEDTITLE7F3A") && human.contains("[secret suspected, hidden]"),
        "{human}"
    );
}

#[test]
fn ui_open_sends_the_open_graphs_and_refuses_a_secured_one_with_its_next() {
    let mut home = Home::new("ui-open");
    home.graph();
    let vault = home.path.join("vault").display().to_string();
    home.ok(&["graph", "new", &vault, "--alias", "vault", "--secured"]);
    let (env, exit) = home.umx(&[
        "ui",
        "open",
        "--graph",
        "vault,demo,ghost",
        "--note",
        "look",
    ]);
    assert_eq!(exit, 0, "{}", env.to_json());
    assert_eq!(env.data["sent"], json!(["demo"]));
    assert_eq!(
        env.data["refused"],
        json!([{"graph": "vault", "kind": "secured"}, {"graph": "ghost", "kind": "unknown"}])
    );
    assert_eq!(env.data["command"]["graphs"], json!(["demo"]));
    let codes: Vec<&str> = env.warnings.iter().map(|w| w.code()).collect();
    assert_eq!(
        codes,
        ["ui_open_left_out", "ui_open_left_out", "ui_no_view"]
    );
    assert_eq!(env.warnings[0].next(), "umx ui --graph vault");
    // Every graph refused: one error each, in order, with its own next; no data (architect).
    let (env, exit) = home.umx(&["ui", "open", "--graph", "ghost,vault"]);
    let errors: Vec<(&str, &str)> = env.errors.iter().map(|e| (e.code(), e.next())).collect();
    assert_eq!(
        (errors, exit, env.data.is_null()),
        (
            vec![
                ("ui_open_refused", "umx graph list --all"),
                ("ui_open_refused", "umx ui --graph vault")
            ],
            1,
            true
        ),
        "{}",
        env.to_json()
    );
    for bad in ["demo,demo", "a,b,c,d,e,f,g,h,i,j,k,l,m"] {
        let (env, _) = home.umx(&["ui", "open", "--graph", bad]);
        assert_eq!(
            env.errors[0].code(),
            "usage_invalid",
            "{bad}: {}",
            env.to_json()
        );
    }
}

/// ws009 M1b2 (plan 161): `umx ui --detach` returns once the view is up, with no launch code in what it prints;
/// the view outlives the CLI; one runs per UMX_HOME (a second is refused with what to do instead); `status` says
/// running, port, pid and when it started, never the graphs; `stop` ends it and its server; a daemon restart ends it
/// too (the start id changed); and a supervisor killed with -9 takes its server down (the lifeline). Agents asking a
/// view to open graphs are told what brings one up.
#[test]
fn ui_detach_runs_the_view_in_the_background_until_stopped_or_the_daemon_restarts() {
    let mut home = Home::new("ui-detach");
    home.graph();
    let env = home.fake_view();
    let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let (said, _) = home.umx(&["ui", "open", "--graph", "demo"]);
    assert_eq!(said.warnings[0].code(), "ui_no_view");
    assert_eq!(said.warnings[0].next(), "umx ui --detach --graph demo");

    let (up, exit) = home.umx_with(
        &[
            "ui",
            "--detach",
            "--no-open",
            "--graph",
            "demo",
            "--port",
            "47123",
        ],
        &env,
    );
    assert_eq!(exit, 0, "{}", up.to_json());
    assert_eq!(
        (
            up.data["url"].as_str(),
            up.data["port"].as_u64(),
            up.data["detached"].as_bool(),
            up.data["opened"].as_bool()
        ),
        (
            Some("http://127.0.0.1:47123/"),
            Some(47123),
            Some(true),
            Some(false)
        )
    );
    assert_eq!(up.data["graphs"], json!(["demo"]));
    assert!(!up.to_json().contains("#t="), "{}", up.to_json());
    let pid = up.data["pid"].as_u64().unwrap();
    assert!(
        home.server_alive(),
        "the view outlived the CLI that started it"
    );
    let status = home.ok(&["ui", "status"]);
    assert_eq!(
        (
            status.data["running"].as_bool(),
            status.data["port"].as_u64(),
            status.data["pid"].as_u64()
        ),
        (Some(true), Some(47123), Some(pid))
    );
    assert!(
        status.data["started"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
    assert!(
        !status.to_json().contains("demo"),
        "status never names the graphs"
    );
    let record = fs::read_to_string(home.path.join("run/ui.json")).unwrap();
    let mode = fs::metadata(home.path.join("run/ui.json"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "{record}");

    // One per UMX_HOME, in the background or in a terminal.
    for args in [
        vec!["ui", "--detach", "--no-open", "--graph", "demo"],
        vec!["ui", "--detach", "--no-open"],
    ] {
        let (again, exit) = home.umx_with(&args, &env);
        assert_eq!(
            (exit, again.errors[0].code()),
            (1, "ui_already_running"),
            "{}",
            again.to_json()
        );
    }
    let (again, _) = home.umx_with(&["ui", "--detach", "--no-open", "--graph", "demo"], &env);
    assert_eq!(again.errors[0].next(), "umx ui open --graph demo --raise");

    // A supervisor but no tab: the agent is told to raise.
    let (said, _) = home.umx(&["ui", "open", "--graph", "demo"]);
    assert_eq!(said.warnings[0].next(), "umx ui open --graph demo --raise");

    let stopped = home.ok(&["ui", "stop"]);
    assert_eq!(stopped.data, json!({ "stopped": true, "pid": pid }));
    home.eventually("the server stops with its supervisor", |h| {
        !h.server_alive()
    });
    assert!(!home.ui_running());
    assert_eq!(home.ok(&["ui", "stop"]).data, json!({ "stopped": false }));
    home.eventually("the stopped supervisor gone", |_| !Home::alive(pid));

    // A daemon restart ends it: a new daemon is a new start id.
    let (up, exit) = home.umx_with(&["ui", "--detach", "--no-open"], &env);
    assert_eq!(exit, 0, "{}", up.to_json());
    let pid = up.data["pid"].as_u64().unwrap();
    home.ok(&["daemon", "restart"]);
    home.eventually(
        "the view ends with the daemon it was started against",
        |h| !h.server_alive() && !h.ui_running(),
    );
    home.eventually("the ended supervisor gone", |_| !Home::alive(pid));

    // kill -9 on the supervisor: the server sees its stdin close and goes.
    let (up, exit) = home.umx_with(&["ui", "--detach", "--no-open"], &env);
    assert_eq!(exit, 0, "{}", up.to_json());
    let pid = up.data["pid"].as_u64().unwrap().to_string();
    assert!(
        Command::new("/bin/kill")
            .args(["-s", "KILL", &pid])
            .bounded_status()
            .unwrap()
            .success()
    );
    home.eventually("the server dies with its supervisor", |h| !h.server_alive());
    assert!(!home.ui_running(), "a dead supervisor's record is stale");
    assert_eq!(
        home.ok(&["ui", "stop"]).data,
        json!({ "stopped": false }),
        "and never signalled"
    );
}

/// Review B1: launchers started at the same moment, in the background and in a terminal, against no
/// ui.json, give exactly one view. The one that takes the view first runs; every other is refused with
/// ui_already_running and its next before it starts a server.
#[test]
fn ui_launchers_started_together_give_exactly_one_view() {
    let mut home = Home::new("ui-race");
    home.graph();
    let env = home.fake_view();
    let spawn = |args: &[&str]| {
        let mut cmd = home.command(args);
        cmd.envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        cmd.stderr(Stdio::null()).spawn_captured().unwrap()
    };
    let detached: Vec<Captured> = (0..3)
        .map(|_| spawn(&["ui", "--detach", "--no-open", "--graph", "demo"]))
        .collect();
    // A person's refusal is printed on stderr, so the terminal one keeps it.
    let (term_out, to_out) = Capture::new();
    let (term_err, to_err) = Capture::new();
    let mut terminal = {
        let mut cmd = home.command(&["--human", "ui", "--no-open"]);
        cmd.envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        cmd.stdout(to_out).stderr(to_err).spawn().unwrap()
    };
    let mut ready = 0;
    let mut refused = 0;
    for child in detached {
        let out = child.finish(LIMIT).stdout;
        let env = Envelope::from_json(String::from_utf8_lossy(&out).trim().as_bytes()).unwrap();
        match env.errors.first().map(Issue::code) {
            None => ready += 1,
            Some("ui_already_running") => {
                assert_eq!(env.errors[0].next(), "umx ui open --graph demo --raise");
                refused += 1;
            }
            Some(other) => panic!("{other}: {}", env.to_json()),
        }
    }
    // The terminal one either runs (its link printed) or was refused and exits; whichever it is, within LIMIT.
    let link = "umx ui: open http://127.0.0.1:";
    let deadline = Instant::now() + LIMIT;
    let runs = loop {
        let exited = terminal.try_wait().unwrap().is_some();
        let out = fs::read_to_string(&term_out.0).unwrap();
        if out.starts_with(link) && out.contains('\n') {
            break true;
        }
        if exited {
            assert_eq!(out, "", "a refused terminal prints nothing on stdout");
            break false;
        }
        assert!(
            Instant::now() < deadline,
            "umx --human ui --no-open neither printed its link nor exited within {} s",
            LIMIT.as_secs()
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    if runs {
        ready += 1;
    } else {
        let said = String::from_utf8_lossy(&term_err.read()).into_owned();
        assert!(said.contains("ui_already_running"), "{said}");
        refused += 1;
    }
    assert_eq!((ready, refused), (1, 3), "exactly one view");
    let starts = fs::read_to_string(home.path.join("nodebin/servers.log")).unwrap();
    assert_eq!(starts.lines().count(), 1, "one server ever started");
    let _ = home.umx(&["ui", "stop"]);
    let _ = terminal.kill();
    let _ = terminal.wait();
    home.eventually("the one server stops", |h| !h.server_alive());
}

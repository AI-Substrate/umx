//! The cli tests: the daemon, its service and restart, output modes, doctor and the client's bounds.
//! A module of cli.rs, whose `Home` and helpers it uses, in its own file so no file nears the seed gate's
//! size cap.

use super::*;

#[test]
fn lifecycle_with_an_owner_only_socket() {
    let mut home = Home::new("life");
    let ready = home.start();
    assert_eq!(ready.status, Status::Ok);
    assert_eq!(ready.data["socket"], home.socket().display().to_string());
    assert_eq!(mode(&home.path.join("run")), 0o700);
    assert_eq!(mode(&home.socket()), 0o600);

    let (status, exit) = home.umx(&["daemon", "status"]);
    assert_eq!((exit, &status.data["running"]), (0, &true.into()));
    assert_eq!(status.data["pid"], ready.data["pid"]);
    if cfg!(not(feature = "stub")) {
        assert_eq!(status.data["mode"], "real");
        assert_eq!(
            status.data["adapters"],
            umx_protocol::json!({
                "clock": "system.clock",
                "config_files": "fs.config-files",
                "graph_files": "fs.graph-files",
                "id_generator": "system.id-generator",
                "local_state": "fs.local-state",
                "model_fetcher": "hf.model-fetcher",
                "secret_allows": "fs.local-state",
                "lint_terms": "fs.local-state",
                "scopes": "fs.local-state",
                "registry": "fs.local-state",
                "secrets": "fs.secrets",
                "source_reader": "fs.source-reader",
                "vector_cache": "fs.graph-files",
                "service_manager": if cfg!(target_os = "macos") {
                    "launchd.service-manager"
                } else {
                    "systemd.service-manager"
                },
            })
        );
    }

    let (stop, exit) = home.umx(&["daemon", "stop"]);
    assert_eq!((exit, &stop.data["stopped"]), (0, &true.into()));
    assert!(
        home.daemon
            .take()
            .unwrap()
            .bounded_wait()
            .unwrap()
            .success()
    );
    assert!(!home.socket().exists());

    let (status, exit) = home.umx(&["daemon", "status"]);
    assert_eq!((exit, &status.data["running"]), (0, &false.into()));
}

#[test]
fn with_the_daemon_down_only_the_offline_commands_work() {
    let home = Home::new("down");
    for args in [
        &["daemon", "stop"][..],
        &["graph", "list"],
        &["node", "add", "--type", "problem", "--title", "x"],
    ] {
        let (env, exit) = home.umx(args);
        assert_eq!(exit, 3, "{args:?}");
        assert_eq!(
            issue(&env, Code::DaemonNotRunning).next(),
            "umx daemon start"
        );
    }
    for args in [
        &["version"][..],
        &["agents-start-here"],
        &["docs", "list"],
        &["docs", "get", "daemon"],
        &["daemon", "status"],
    ] {
        let (env, exit) = home.umx(args);
        assert_eq!((exit, env.status), (0, Status::Ok), "{args:?}");
    }
    // umx doctor works too, says the daemon is down, and starts nothing.
    let (env, exit) = home.umx(&["doctor"]);
    assert_eq!((exit, env.data["verdict"].as_str()), (5, Some("degraded")));
    assert_eq!(
        issue(&env, Code::DoctorDaemonDown).next(),
        "umx daemon start"
    );
    assert!(!home.socket().exists(), "doctor must not start a daemon");
}

#[test]
fn json_is_the_default_and_human_is_on_request() {
    // req-63, piped: one envelope with no flag; --human or UMX_OUTPUT=human for text; last flag wins.
    let mut home = Home::new("output");
    // Offline, daemon down: the same rule.
    for args in [
        &["version"][..],
        &["agents-start-here"],
        &["docs", "list"],
        &["docs", "get", "output"],
        &["daemon", "status"],
    ] {
        assert_output_modes(&home, args);
    }
    // Daemon up, one graph loaded, so the human list is not empty.
    home.start();
    let graph = home.path.join("g");
    let (_, exit) = home.umx(&["graph", "new", graph.to_str().unwrap(), "--alias", "g"]);
    assert_eq!(exit, 0);
    assert_output_modes(&home, &["graph", "list"]);
    let rows = home.human(&["graph", "list"]);
    assert!(
        rows.lines().any(|l| l.split("  ").any(|cell| cell == "g")),
        "{rows}"
    );
}

#[test]
fn daemon_start_human_prints_the_banner_and_ready_line_on_stderr() {
    // 007 section 3 and req-64: JSON (the default) is one envelope on stdout (Home::start); --human is the
    // banner then the ready line on stderr, stdout empty.
    let mut home = Home::new("ready");
    let (stdout, to) = Capture::new();
    let mut child = home
        .command(&["daemon", "start", "--human"])
        .stdout(to)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = child.stderr.take().unwrap();
    let limit = home.limit();
    let seen = lines_until(
        stderr,
        "umx daemon ready: pid ",
        &mut child,
        "umx daemon start --human",
        limit,
    );
    home.daemon = Some(child);
    assert!(seen.contains(BANNER_ROW), "{seen}");
    assert!(
        !seen.contains('\x1b'),
        "piped stderr has no colour: {seen:?}"
    );
    let (_, exit) = home.umx(&["daemon", "stop"]);
    assert_eq!(exit, 0);
    home.daemon.take().unwrap().bounded_wait().unwrap();
    let rest = String::from_utf8(stdout.read()).unwrap();
    assert_eq!(rest, "", "stdout stays empty in human mode");
}

#[test]
fn human_mode_has_a_banner_tables_and_colour_only_where_colour_is_allowed() {
    // req-64. CLICOLOR_FORCE=1 stands in for a terminal; NO_COLOR and TERM=dumb win over it.
    let mut home = Home::new("colour");
    home.start();
    let graph = home.path.join("g");
    assert_eq!(
        home.umx(&["graph", "new", graph.to_str().unwrap(), "--alias", "g"])
            .1,
        0
    );
    let mut ids = Vec::new();
    for title in ["Approval delays", "Launch date"] {
        let (node, _) = home.umx(&["node", "add", "--type", "problem", "--title", title]);
        ids.push(node.data["node"]["id"].as_str().unwrap().to_owned());
    }
    assert_eq!(home.umx(&["edge", "add", &ids[0], "blocks", &ids[1]]).1, 0);
    let commands: [&[&str]; 7] = [
        &["version"],
        &["agents-start-here"],
        &["docs", "list"],
        &["node", "list"],
        &["node", "find", "approval"],
        &["edge", "list"],
        &["graph", "list"],
    ];
    let force = [("CLICOLOR_FORCE", "1")];
    for args in commands {
        // JSON: identical with forced colour, never a banner or an escape code. graph list counts down
        // unloads_in_s, so it is compared without that field.
        let (plain, _) = output_with(&home, args, &[]);
        let (forced, _) = output_with(&home, args, &force);
        let strip = |s: &str| {
            s.split("\"unloads_in_s\":")
                .map(|part| part.trim_start_matches(|c: char| c.is_ascii_digit()))
                .collect::<Vec<_>>()
                .join("")
        };
        if args == ["graph", "list"] {
            assert_eq!(strip(&plain), strip(&forced), "{args:?}");
        } else {
            assert_eq!(plain, forced, "{args:?}: JSON changed with colour forced");
        }
        assert!(
            !forced.contains('\x1b') && !forced.contains(BANNER_ROW),
            "{args:?}"
        );

        let human = [args, &["--human"]].concat();
        let (coloured, _) = output_with(&home, &human, &force);
        assert!(
            coloured.contains("\x1b["),
            "{args:?}: no colour when forced"
        );
        for env in [
            &[][..],
            &[("NO_COLOR", "1"), ("CLICOLOR_FORCE", "1")],
            &[("TERM", "dumb"), ("CLICOLOR_FORCE", "1")],
        ] {
            let (out, err) = output_with(&home, &human, env);
            assert!(
                !out.contains('\x1b') && !err.contains('\x1b'),
                "{args:?} {env:?}: {out:?}"
            );
            let banner = matches!(args[0], "version" | "agents-start-here");
            assert_eq!(out.contains(BANNER_ROW), banner, "{args:?} {env:?}");
        }
    }
    // Tables: a header row, then aligned columns.
    let (list, _) = output_with(&home, &["node", "list", "--human"], &[]);
    let lines: Vec<&str> = list.lines().collect();
    assert!(
        lines[0].starts_with("id ") && lines[0].contains("title"),
        "{list}"
    );
    assert_eq!(lines[2].find("problem"), lines[0].find("type"), "{list}");
    // Warning and error lines are coloured when allowed.
    let (_, err) = output_with(&home, &["node", "get", "nope00", "--human"], &force);
    assert!(err.contains("\x1b[1;31merror:\x1b[0m"), "{err:?}");
}

#[test]
fn output_follows_the_reader() {
    // req-63 (D22a): a terminal gets text, anything else JSON; UMX_OUTPUT=json|human pins the shape over the
    // terminal; --json and --human win over both, the last one first. JSON is the same bytes either way.
    let home = Home::new("pty");
    for args in [&["version"][..], &["docs", "list"], &["daemon", "status"]] {
        let (piped, _) = output_with(&home, args, &[]);
        assert!(Envelope::from_json(piped.as_bytes()).is_ok(), "{piped}");
        for (flags, env, json) in [
            (&[][..], &[][..], false),
            (&["--json"], &[], true),
            (&["--human"], &[], false),
            (&[], &[("UMX_OUTPUT", "json")], true),
            (&[], &[("UMX_OUTPUT", "human")], false),
            (&["--human"], &[("UMX_OUTPUT", "json")], false),
            (&["--json"], &[("UMX_OUTPUT", "human")], true),
            (&["--human", "--json"], &[], true),
            (&["--json", "--human"], &[], false),
        ] {
            let out = on_a_pty(&home, &[args, flags].concat(), env);
            let what = format!("{args:?} {flags:?} {env:?}: {out:?}");
            if json {
                assert_eq!(json_part(&out), piped.trim_end(), "{what}");
            } else {
                assert!(!out.contains("{\"umx\":"), "{what}");
                assert!(
                    out.contains("\x1b["),
                    "text on a terminal has colour: {what}"
                );
            }
        }
    }
}

#[test]
fn graph_text_cannot_drive_the_terminal() {
    // A title carrying a clipboard write (OSC 52), a screen clear, a C1 CSI and a line break prints as text.
    let mut home = Home::new("inject");
    home.start();
    let graph = home.path.join("g");
    assert_eq!(
        home.umx(&["graph", "new", graph.to_str().unwrap(), "--alias", "g"])
            .1,
        0
    );
    let title = "evil\x1b]52;c;aGk=\x07\x1b[2J\u{9b}31m\nrow title";
    let (node, _) = home.umx(&["node", "add", "--type", "problem", "--title", title]);
    let id = node.data["node"]["id"].as_str().unwrap().to_owned();
    // Review B1: a --columns name is printed as a header (OSC 8 link, CSI, bidi override, Arabic mark).
    let column = "\x1b]8;;https://x.example\x07f\x1b]8;;\x07\x1b[31m\u{202e}\u{061c}ld";
    for args in [
        &["node", "list"][..],
        &["node", "get", &id],
        &["node", "find", "row"],
        &["node", "list", "--columns", column],
    ] {
        for env in [&[][..], &[("CLICOLOR_FORCE", "1")], &[("NO_COLOR", "1")]] {
            let (out, _) = output_with(&home, &[args, &["--human"]].concat(), env);
            for bad in [
                "]52;", "]8;", "[2J", "[31m", "\u{9b}", "\x07", "\u{202e}", "\u{061c}",
            ] {
                assert!(!out.contains(bad), "{args:?} {env:?}: {out:?}");
            }
            if env != [("CLICOLOR_FORCE", "1")] {
                assert!(!out.contains('\x1b'), "{args:?}: {out:?}");
            }
        }
    }
    let (list, _) = output_with(&home, &["node", "list", "--human"], &[]);
    assert!(list.contains("evil row title"), "{list}");
}

#[test]
fn a_stale_socket_does_not_block_start() {
    let mut home = Home::new("stale");
    home.run_dir(0o700);
    drop(UnixListener::bind(home.socket()).unwrap());
    assert!(home.socket().exists());
    assert_eq!(home.start().status, Status::Ok);
}

#[test]
fn socket_unavailable_names_each_cause() {
    let long = Home::new(&"x".repeat(100));
    let (env, exit) = long.umx(&["daemon", "start"]);
    assert_eq!(exit, 4);
    assert!(
        issue(&env, Code::SocketUnavailable)
            .message()
            .contains("104-byte")
    );

    let squatted = Home::new("squat");
    squatted.run_dir(0o700);
    fs::write(squatted.socket(), "not a socket").unwrap();
    let (env, exit) = squatted.umx(&["daemon", "start"]);
    assert_eq!(exit, 4);
    assert!(
        issue(&env, Code::SocketUnavailable)
            .message()
            .contains("not a socket")
    );
}

#[test]
fn an_idle_daemon_uses_no_cpu() {
    let mut home = Home::new("idle");
    let pid = home.start().data["pid"].to_string();
    let cpu = || {
        let out = Command::new("ps")
            .args(["-o", "time=", "-p", &pid])
            .bounded_output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    std::thread::sleep(Duration::from_millis(300));
    let before = cpu();
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(cpu(), before);
}

#[test]
fn rejected_automatic_login_never_reaches_graph_files_or_the_reply() {
    let mut home = Home::new("safe-login");
    let path = home.graph();
    let token = format!("ghp_{}", "a".repeat(36));
    let output = home
        .command(&["node", "add", "--type", "problem", "--title", "Safe author"])
        .env("USER", &token)
        .env_remove("LOGNAME")
        .env_remove("UMX_AUTHOR")
        .bounded_output()
        .unwrap();
    assert!(output.status.success());
    let reply = Envelope::from_json(&output.stdout).unwrap();
    assert!(!reply.to_json().contains(&token));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(&token));
    let nodes = fs::read_to_string(Path::new(&path).join("nodes.jsonl")).unwrap();
    let content = nodes
        .lines()
        .map(|line| umx_protocol::parse_json(line).unwrap())
        .find(|n| n["title"] == "Safe author")
        .unwrap();
    assert_eq!(content["created_by"], "unknown");
    for file in ["graph.json", "nodes.jsonl", "edges.jsonl"] {
        assert!(
            !fs::read_to_string(Path::new(&path).join(file))
                .unwrap()
                .contains(&token)
        );
    }
    let (manual, exit) = home.umx(&[
        "node",
        "add",
        "--type",
        "problem",
        "--title",
        "Explicit author",
        "--by",
        "chosen author",
    ]);
    assert_eq!(exit, 0);
    assert!(!manual.to_json().contains(&token));
    let nodes = fs::read_to_string(Path::new(&path).join("nodes.jsonl")).unwrap();
    let explicit = nodes
        .lines()
        .map(|line| umx_protocol::parse_json(line).unwrap())
        .find(|n| n["title"] == "Explicit author")
        .unwrap();
    assert_eq!(explicit["created_by"], "chosen author");
}

#[test]
fn already_running_names_the_pid() {
    let mut home = Home::new("pid");
    let pid = home.start().data["pid"].to_string();
    let (env, _) = home.umx(&["daemon", "start"]);
    assert!(
        issue(&env, Code::DaemonAlreadyRunning)
            .message()
            .contains(&format!("(pid {pid})"))
    );
}

#[test]
fn dev_restart_replaces_a_running_daemon_with_the_new_build() {
    let home = Home::new("dev");
    for _ in 0..2 {
        let out = dev_restart(&home);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(String::from_utf8_lossy(&out.stdout).contains("daemon.built: dev"));
    }
    let (status, _) = home.umx(&["daemon", "status"]);
    assert_eq!(status.data["running"], true);
}

#[test]
fn dev_restart_reinstalls_the_service_so_the_new_build_runs() {
    let home = Home::new("dev-svc");
    let out = dev_restart(&home);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Run the installed umx as a service instead, through the stub service manager.
    home.umx(&["daemon", "stop"]);
    home.stub_services();
    home.service_state("run", "");
    let installed = home.path.join("bin/umx");
    let mut cmd = Command::new(&installed);
    sandbox(&mut cmd, &home.path);
    let env = cmd
        .args(["daemon", "install"])
        .env("UMX_HOME", &home.path)
        .env("UMX_ROOTS", &home.path)
        .bounded_output()
        .unwrap();
    assert!(
        env.status.success(),
        "{}",
        String::from_utf8_lossy(&env.stdout)
    );
    let out = dev_restart(&home);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (status, _) = home.umx(&["daemon", "status"]);
    assert_eq!(status.data["running"], true, "{}", status.to_json());
    assert_eq!(status.data["service"]["installed"], true);
    // Two installs: the one above and the one just dev made; each loaded the service file.
    let starts = if cfg!(target_os = "macos") {
        "bootstrap gui/"
    } else {
        "--user restart umx-sandbox.service"
    };
    assert_eq!(home.argv().matches(starts).count(), 2, "{}", home.argv());
}

#[test]
fn dev_restart_fails_loudly_when_the_daemon_cannot_start() {
    let home = Home::new("devfail");
    home.run_dir(0o755);
    let out = dev_restart(&home);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("the daemon exited at start") && stderr.contains("socket_unavailable"),
        "{stderr}"
    );
}

#[test]
fn a_request_from_another_protocol_writes_nothing_and_stop_still_works() {
    // A7: an older CLI's node add is refused before it runs; status and stop answer it, so its next works.
    let mut home = Home::new("proto");
    home.graph();
    let mut add = Request::op(
        umx_protocol::Op::NodeAdd {
            on_key: None,
            related: None,
            raised_in: None,
            kind: "problem".into(),
            title: "from an older cli".into(),
            status: None,
            note: None,
            sets: vec![],
            parent: None,
            evidence: vec![],
            evidence_kind: None,
            locator: None,
        },
        None,
        Some("old".into()),
        older_cli(),
    );
    add.umx -= 1;
    let (env, exit) = raw(&home.socket(), &add);
    assert_eq!(exit, 1);
    let refused = issue(&env, Code::ProtocolMismatch);
    assert_eq!(refused.next(), "umx daemon stop && umx daemon start");
    // Round 1: the same write under a lifecycle label is refused too.
    for label in ["daemon status", "daemon stop"] {
        add.command = label.into();
        let (env, _) = raw(&home.socket(), &add);
        issue(&env, Code::ProtocolMismatch);
    }
    let (list, _) = home.umx(&["node", "list"]);
    assert_eq!(list.data["total"], 0, "nothing was written");
    let nodes = fs::read_to_string(home.path.join("g/nodes.jsonl")).unwrap();
    assert_eq!(nodes, "");
    for command in ["daemon status", "daemon stop"] {
        let mut r = Request::new(command, vec![], older_cli());
        r.umx -= 1;
        let (env, exit) = raw(&home.socket(), &r);
        assert_eq!(
            (exit, env.errors.len()),
            (0, 0),
            "{command}: {}",
            env.to_json()
        );
    }
    assert!(
        home.daemon
            .take()
            .unwrap()
            .bounded_wait()
            .unwrap()
            .success()
    );
}

/// F113, security ruling b: naming every loaded graph is only for counting. Doctor's reply never carries an edge or
/// node id from a named-visibility graph's check.
#[test]
fn doctor_gives_only_counts_for_a_named_visibility_graph() {
    let mut home = Home::new("doc-counts");
    let ids = home.linked_graphs_and_a_gone_one();
    let (env, _) = home.umx(&["doctor"]);
    let text = env.to_json();
    for id in &ids {
        assert!(!text.contains(id.as_str()), "doctor shows {id}: {text}");
    }
}

#[test]
fn doctor_says_ok_when_everything_is_healthy() {
    let mut home = Home::new("doc-ok");
    home.graph();
    home.add("problem", "One");
    let (env, exit) = home.umx(&["doctor"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    assert_eq!(env.data["verdict"], "ok");
    // The only findings are information: no embedder is configured, and the daemon is not a service.
    let codes: Vec<&str> = env.warnings.iter().map(Issue::code).collect();
    assert_eq!(
        codes,
        [
            "doctor_embedder_none",
            "doctor_service_none",
            "doctor_skill_missing"
        ]
    );
    let rows = env.data["rows"].as_array().unwrap();
    let checks: Vec<&str> = rows.iter().map(|r| r["check"].as_str().unwrap()).collect();
    for check in [
        "daemon",
        "protocol",
        "build",
        "writer",
        "permissions",
        "embedder",
        "graph folder",
        "graph check",
    ] {
        assert!(checks.contains(&check), "no {check} row in {checks:?}");
    }
    // A live socket's mode is checked; so are UMX_HOME and the run folder.
    let checked: Vec<&str> = rows
        .iter()
        .filter(|r| r["check"] == "permissions")
        .map(|r| r["subject"].as_str().unwrap())
        .collect();
    assert_eq!(checked.len(), 3, "{checked:?}");
    assert!(
        checked.iter().any(|p| p.ends_with("run/umx.sock")),
        "{checked:?}"
    );
    let human = home
        .command(&["doctor", "--human"])
        .bounded_output()
        .unwrap();
    assert!(String::from_utf8_lossy(&human.stdout).contains("verdict: ok"));
}

/// F77: build.rs and doctor fingerprint the skill the same way, so the skill this umx was built with is ok whether
/// linked (just skill-link) or copied (npx skills add, which may leave dotfiles such as .DS_Store), and one changed
/// byte is out of date (weight 5).
#[test]
fn doctor_knows_the_skill_this_umx_was_built_with() {
    let skill_row = |env: &Envelope| {
        let rows = env.data["rows"].as_array().unwrap();
        let row = rows.iter().find(|r| r["check"] == "skill").cloned();
        row.unwrap_or_else(|| panic!("no skill row in {}", env.to_json()))
    };
    let home = Home::new("doc-skill-link");
    home.skill_link(&repo_skill());
    let (env, _) = home.umx(&["doctor"]);
    assert_eq!(skill_row(&env)["outcome"], "ok", "{}", env.to_json());

    let home = Home::new("doc-skill-copy");
    let copy = home.skill_copy(&home.path.join(".agents/skills/umx"));
    fs::write(copy.join(".DS_Store"), "finder\n").unwrap();
    fs::create_dir_all(copy.join("references/.cache")).unwrap();
    fs::write(copy.join("references/.cache/x"), "y").unwrap();
    let (env, _) = home.umx(&["doctor"]);
    assert_eq!(skill_row(&env)["outcome"], "ok", "{}", env.to_json());

    let route = copy.join("references/routes/start.md");
    let mut text = fs::read(&route).unwrap();
    *text.last_mut().unwrap() ^= 1;
    fs::write(&route, text).unwrap();
    let (env, _) = home.umx(&["doctor"]);
    let row = skill_row(&env);
    assert_eq!(
        (row["outcome"].as_str(), row["code"].as_str()),
        (Some("degraded"), Some("doctor_skill_out_of_date")),
        "{}",
        env.to_json()
    );
    // F94: the installer is pinned, and the repo is pinned to this build's own commit when the build knows it. The
    // agent runs the line (D76a Q10), so it carries -y and never stops to ask.
    let message = issue(&env, Code::DoctorSkillOutOfDate).message().to_owned();
    assert!(
        message.contains("npx skills@1.7.6 add AI-Substrate/umx")
            && message.ends_with(" --skill umx -g --agent '*' -y"),
        "{message}"
    );
    let source = message
        .split("add ")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .unwrap();
    assert!(
        source == "AI-Substrate/umx"
            || source
                .strip_prefix("AI-Substrate/umx#")
                .is_some_and(|c| c.len() == 40),
        "{source}"
    );
}

/// ws012 s9 (D76a Q10): a binary install, as an agent meets it: umx copied out of any checkout into
/// `~/.local/bin`, a fresh HOME and UMX_HOME, no daemon, no skill and no service. Doctor prints both commands the
/// agent runs next, in JSON and in human text: `umx daemon install` and the skill line, pinned to this binary's own
/// commit when its build knows it, with -y so it runs without a prompt.
#[test]
fn doctor_in_a_binary_install_prints_the_service_and_pinned_skill_commands() {
    let mut home = Home::new("doc-bin");
    let bin = home.path.join(".local/bin");
    fs::create_dir_all(&bin).unwrap();
    let copied = bin.join("umx");
    fs::copy(env!("CARGO_BIN_EXE_umx"), &copied).unwrap();
    home.binary = Box::leak(copied.display().to_string().into_boxed_str());
    home.cwd = Some(home.path.clone());
    let commit = home.ok(&["version"]).data["cli"]["commit"]
        .as_str()
        .unwrap()
        .to_owned();

    // The daemon is down, so doctor's verdict is not ok; only the two rows matter here.
    let (env, _) = home.umx(&["doctor"]);
    let rows = env.data["rows"].as_array().unwrap();
    let row = |check: &str| {
        rows.iter()
            .find(|r| r["check"] == check)
            .unwrap_or_else(|| panic!("no {check} row in {}", env.to_json()))
    };
    assert_eq!(row("service")["code"], "doctor_service_none");
    assert_eq!(
        issue(&env, Code::DoctorServiceNone).next(),
        "umx daemon install"
    );
    assert_eq!(row("skill")["code"], "doctor_skill_missing");
    let message = issue(&env, Code::DoctorSkillMissing).message().to_owned();
    let line = message
        .split("Install it: ")
        .nth(1)
        .and_then(|rest| rest.split(" (or ").next())
        .unwrap_or_else(|| panic!("{message}"));
    let source = line
        .strip_prefix("npx skills@1.7.6 add ")
        .and_then(|rest| rest.strip_suffix(" --skill umx -g --agent '*' -y"))
        .unwrap_or_else(|| panic!("{line}"));
    // Pinned when the build knew its full commit (skills/umx as committed), and then to this binary's commit.
    assert!(
        source == "AI-Substrate/umx"
            || source
                .strip_prefix("AI-Substrate/umx#")
                .is_some_and(|c| c.len() == 40 && c.starts_with(&commit)),
        "{source} for build {commit}"
    );

    let human = home
        .command(&["doctor", "--human"])
        .bounded_output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&human.stdout),
        String::from_utf8_lossy(&human.stderr)
    );
    for want in [line, "umx daemon install"] {
        assert!(text.contains(want), "{want} not in {text}");
    }
}

#[test]
fn doctor_fix_removes_a_stale_socket_and_tightens_modes_and_nothing_else() {
    let home = Home::new("doc-fix");
    home.owner_only();
    drop(UnixListener::bind(home.socket()).unwrap());
    fs::set_permissions(&home.path, fs::Permissions::from_mode(0o755)).unwrap();
    home.run_dir(0o750);
    let config = home.path.join("config.toml");
    fs::write(&config, "# mine\n").unwrap();
    // Group-writable: the bit the config adapter refuses (read by others is allowed).
    fs::set_permissions(&config, fs::Permissions::from_mode(0o664)).unwrap();
    // A graph folder doctor must never touch, open to others on purpose.
    let graph = home.path.join("g");
    fs::create_dir_all(&graph).unwrap();
    fs::write(graph.join("graph.json"), "{}").unwrap();
    fs::set_permissions(&graph, fs::Permissions::from_mode(0o755)).unwrap();

    let (before, exit) = home.umx(&["doctor"]);
    assert_eq!((exit, before.data["verdict"].as_str()), (6, Some("broken")));
    let out = home.command(&["doctor", "--fix"]).bounded_output().unwrap();
    let env = Envelope::from_json(&out.stdout).unwrap();
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("remove the stale socket"), "{said}");
    assert!(said.contains("chmod 700"), "{said}");
    assert!(said.contains("chmod 644"), "{said}");
    let fixed = env.data["fixed"].as_array().unwrap();
    assert_eq!(fixed.len(), 4, "{}", env.to_json());
    assert!(fixed.iter().all(|f| f["done"] == true), "{}", env.to_json());
    // After the fixes only the daemon being down is left, and doctor started nothing.
    assert_eq!(out.status.code(), Some(5));
    assert_eq!(env.data["verdict"], "degraded");
    assert!(!home.socket().exists());
    assert_eq!(
        (
            mode(&home.path),
            mode(&home.path.join("run")),
            mode(&config)
        ),
        (0o700, 0o700, 0o644)
    );
    assert_eq!(fs::read_to_string(&config).unwrap(), "# mine\n");
    assert_eq!(mode(&graph), 0o755);
    assert_eq!(fs::read_to_string(graph.join("graph.json")).unwrap(), "{}");
}

#[test]
fn doctor_takes_only_fix() {
    let home = Home::new("doc-usage");
    let (env, exit) = home.umx(&["doctor", "--everything"]);
    assert_eq!(exit, 2);
    assert_eq!(
        issue(&env, Code::UsageInvalid).next(),
        "umx docs get doctor"
    );
}

#[test]
fn install_runs_this_umx_reinstall_upgrades_and_uninstall_leaves_nothing() {
    let home = Home::new("svc");
    let program = home.install();
    assert_eq!(Path::new(&program), fs::canonicalize(home.binary).unwrap());
    let unit = fs::read_to_string(home.unit_file()).unwrap();
    assert!(unit.contains("daemon start --service") || unit.contains("<string>--service</string>"));
    assert!(unit.contains(&fs::canonicalize(&home.path).unwrap().display().to_string()));
    let (status, exit) = home.umx(&["daemon", "status"]);
    assert_eq!(exit, 0);
    assert_eq!(status.data["service"]["installed"], true);
    assert_eq!(status.data["service"]["loaded"], true);
    // Installing again is the upgrade path (just dev): same file, reloaded.
    home.install();
    let (env, exit) = home.umx(&["daemon", "uninstall"]);
    assert_eq!(
        (exit, env.data["service"]["installed"].as_bool()),
        (0, Some(false)),
        "{}",
        env.to_json()
    );
    assert!(!home.unit_file().exists());
    let (env, exit) = home.umx(&["daemon", "uninstall"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    let argv = home.argv();
    assert!(!argv.contains("sudo"), "{argv}");
    if cfg!(target_os = "macos") {
        assert!(
            argv.contains("bootstrap gui/") && argv.contains("bootout gui/"),
            "{argv}"
        );
    } else {
        assert!(
            argv.contains("--user enable umx-sandbox.service")
                && argv.contains("--user disable --now umx-sandbox.service"),
            "{argv}"
        );
    }
}

#[test]
fn a_service_start_stands_aside_for_a_running_daemon_and_a_hand_start_still_refuses() {
    let mut home = Home::new("svc-aside");
    home.start();
    let (env, exit) = home.umx(&["daemon", "start", "--service"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    issue(&env, Code::DaemonAlreadyRunning);
    let (_, exit) = home.umx(&["daemon", "start"]);
    assert_eq!(exit, 1);
}

#[test]
fn doctor_fix_restarts_a_stale_service_daemon_through_the_service_manager() {
    let home = Home::new("svc-fix");
    home.install();
    home.service_state("pid", "4242");
    let mut status = home.healthy_status();
    status.data["pid"] = json!(4242);
    status.data["version"]["built"] = json!("another-build");
    home.fake_daemon(
        status,
        Some(Envelope::ok("graph list", json!({ "rows": [] }))),
    );
    let out = home.command(&["doctor", "--fix"]).bounded_output().unwrap();
    let env = Envelope::from_json(&out.stdout).unwrap();
    assert!(String::from_utf8_lossy(&out.stderr).contains("restart the umx service"));
    assert_eq!(
        env.data["fixed"][0]["fix"],
        "restart_service",
        "{}",
        env.to_json()
    );
    assert_eq!(env.data["fixed"][0]["done"], true, "{}", env.to_json());
    let argv = home.argv();
    if cfg!(target_os = "macos") {
        assert!(argv.contains("kickstart -k gui/"), "{argv}");
    } else {
        assert!(
            argv.contains("--user restart umx-sandbox.service"),
            "{argv}"
        );
    }
}

/// Review B1: a graph whose files do not decode is never reported clean, and nothing from the file reaches the report.
#[test]
fn doctor_never_calls_an_unreadable_graph_clean() {
    let (env, exit) = trigger(Code::DoctorGraphUnchecked);
    assert_eq!(
        (exit, env.data["verdict"].as_str()),
        (5, Some("degraded")),
        "{}",
        env.to_json()
    );
    let found = issue(&env, Code::DoctorGraphUnchecked);
    assert_eq!(found.next(), "umx check --graph demo");
    assert!(!env.to_json().contains("not-json"), "{}", env.to_json());
}

#[test]
fn help_works_offline_for_every_spelling_and_names_a_nouns_topic() {
    // ws002 s12: every agent tries --help first. All of this runs with the daemon down.
    let home = Home::new("help");
    for args in [&["--help"][..], &["-h"], &["help"]] {
        let (env, exit) = home.umx(args);
        assert_eq!(
            (exit, env.status, env.command.as_str()),
            (0, Status::Ok, "help"),
            "{args:?}"
        );
        let md = env.data["markdown"].as_str().unwrap();
        assert!(
            md.contains("umx node --help") && md.contains("umx daemon start"),
            "{md}"
        );
    }
    let (node_doc, _) = home.umx(&["docs", "get", "node"]);
    for args in [
        &["node", "--help"][..],
        &["node", "-h"],
        &["node", "add", "--help"],
        &["help", "node"],
        &["node", "add", "--type", "problem", "--help"],
    ] {
        let (env, exit) = home.umx(args);
        assert_eq!((exit, env.status), (0, Status::Ok), "{args:?}");
        assert_eq!(env.data["markdown"], node_doc.data["markdown"], "{args:?}");
    }
    let (graph_doc, _) = home.umx(&["docs", "get", "graph"]);
    assert_eq!(
        home.umx(&["check", "--help"]).0.data["markdown"],
        graph_doc.data["markdown"]
    );
    // The query verbs added after help: the project lead's smoke test got help_unknown here.
    let (walk, exit) = home.umx(&["walk", "--help"]);
    assert_eq!((exit, walk.data["topic"].as_str()), (0, Some("explore")));
    // Help is checked before anything runs: this prints the daemon topic and starts nothing.
    let (env, exit) = home.umx(&["daemon", "start", "--help"]);
    assert_eq!((exit, env.data["topic"].as_str()), (0, Some("daemon")));
    assert!(
        !home.socket().exists(),
        "daemon start --help started a daemon"
    );

    let (env, exit) = home.umx(&["no_such_noun", "--help"]);
    assert_eq!(exit, 2);
    let unknown = issue(&env, Code::HelpUnknown);
    assert_eq!(unknown.next(), "umx --help");
    assert!(
        unknown.message().contains("no_such_noun"),
        "{}",
        unknown.message()
    );

    // Human mode prints the topic's text, not an envelope.
    let text = home.human(&["node", "--help"]);
    assert!(
        text.starts_with("# ") && !text.contains("\"umx\":"),
        "{text}"
    );
}

#[test]
fn version_flags_are_umx_version() {
    // F70: --version and -V are `umx version` in every way, with the daemon down and up.
    let mut home = Home::new("version-flag");
    let same = |home: &Home| {
        for output in [&["--json"][..], &["--human"]] {
            let run = |first: &str| {
                let out = home
                    .command(&[&[first][..], output].concat())
                    .bounded_output()
                    .unwrap();
                let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
                (out.status.code(), text(&out.stdout), text(&out.stderr))
            };
            let want = run("version");
            assert_eq!(want.0, Some(0), "{output:?}");
            for alias in ["--version", "-V"] {
                assert_eq!(run(alias), want, "{alias} {output:?}");
            }
        }
        let (env, exit) = home.umx(&["--version"]);
        assert_eq!((exit, env.command.as_str()), (0, "version"));
        env.data["daemon"].clone()
    };
    assert_eq!(same(&home), Value::Null);
    home.start();
    assert!(same(&home).is_object());

    // Extra words and help behave as they do after `version`.
    for (alias, word) in [
        (&["--version", "extra"][..], &["version", "extra"][..]),
        (&["-V", "--help"], &["version", "--help"]),
        (&["--json", "-V"], &["--json", "version"]),
    ] {
        assert_eq!(home.umx(alias), home.umx(word), "{alias:?}");
    }
    // Only the first word: any other leading flag still refuses, and `--help` is still help.
    for args in [&["--verbose"][..], &["-v"], &["--Version"], &["--versions"]] {
        let (env, exit) = home.umx(args);
        assert_eq!(exit, 2, "{args:?}");
        issue(&env, Code::UsageInvalid);
    }
    let (env, exit) = home.umx(&["--help"]);
    assert_eq!((exit, env.command.as_str()), (0, "help"));
}

/// Review B1: a service file umx cannot read back is unreadable (degraded), on either OS's format; a valid file for
/// another program stays the informational doctor_service_elsewhere (its trigger is the control).
#[test]
fn doctor_calls_an_unparseable_service_file_unreadable() {
    let mut home = Home::new("svc-garbage");
    let pid = home.start().data["pid"].to_string();
    home.install();
    home.service_state("pid", &pid);
    let garbage = if cfg!(target_os = "macos") {
        "this is not a plist\n"
    } else {
        "[Service]\nExecStart=/bin/true\n"
    };
    fs::write(home.unit_file(), garbage).unwrap();
    let (env, exit) = home.umx(&["doctor"]);
    assert_eq!(
        (exit, env.data["verdict"].as_str()),
        (5, Some("degraded")),
        "{}",
        env.to_json()
    );
    issue(&env, Code::DoctorServiceUnreadable);
    let codes: Vec<&str> = env.warnings.iter().map(Issue::code).collect();
    assert!(!codes.contains(&"doctor_service_elsewhere"), "{codes:?}");
}

/// Review B2: when the service manager fails to inspect, doctor says unreadable (not "not loaded"), and uninstall
/// fails without removing the service file, so a loaded service is never orphaned.
/// F58: `umx daemon service` reports the installed service from launchd or systemd alone. With no daemon, and
/// with a daemon that takes requests but never answers (status then fails with no `service`, as before), it shows
/// `installed: true`, exit 0. When the service manager cannot be read it fails with service_failed.
#[test]
fn daemon_service_shows_the_installed_service_without_the_daemon() {
    let home = Home::new("svc-view");
    let program = home.install();
    let shown = |home: &Home| {
        let (env, exit) = home.umx(&["daemon", "service"]);
        assert_eq!(exit, 0, "{}", env.to_json());
        assert_eq!(env.data["service"]["installed"], true, "{}", env.to_json());
        assert_eq!(env.data["service"]["program"], json!(program));
    };
    shown(&home);
    // A daemon that takes the request and closes without a reply.
    home.owner_only();
    let listener = UnixListener::bind(home.socket()).unwrap();
    fs::set_permissions(home.socket(), fs::Permissions::from_mode(0o600)).unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            drop(stream);
        }
    });
    // The control: status cannot carry the service on its error path (007: an error's data is null).
    let (status, exit) = home.umx(&["daemon", "status"]);
    assert_ne!(exit, 0, "{}", status.to_json());
    assert!(status.data.is_null(), "{}", status.to_json());
    shown(&home);
    let broken = if cfg!(target_os = "macos") {
        "inspectfail"
    } else {
        "fail"
    };
    home.service_state(broken, "");
    let (env, exit) = home.umx(&["daemon", "service"]);
    assert_eq!(exit, 4, "{}", env.to_json());
    issue(&env, Code::ServiceFailed);
}

#[test]
fn a_failing_service_tool_is_reported_and_uninstall_leaves_the_service_alone() {
    let home = Home::new("svc-toolfail");
    home.install();
    // launchd: only `print` fails (the review's case: uninstall must not read that as "not loaded"). systemd's
    // uninstall never inspects (it runs disable --now), so there the whole tool fails.
    let broken = if cfg!(target_os = "macos") {
        "inspectfail"
    } else {
        "fail"
    };
    home.service_state(broken, "");
    let (env, exit) = home.umx(&["doctor"]);
    assert_eq!(exit, 5, "{}", env.to_json());
    issue(&env, Code::DoctorServiceUnreadable);
    let codes: Vec<&str> = env.warnings.iter().map(Issue::code).collect();
    assert!(!codes.contains(&"doctor_service_not_loaded"), "{codes:?}");
    let (env, exit) = home.umx(&["daemon", "uninstall"]);
    assert_eq!(exit, 4, "{}", env.to_json());
    issue(&env, Code::ServiceFailed);
    assert!(home.unit_file().exists(), "the service file stays");
    // The absent-service control: once the tool works, uninstall succeeds and repeats.
    fs::remove_file(home.path.join("bin").join(broken)).unwrap();
    assert_eq!(home.umx(&["daemon", "uninstall"]).1, 0);
    assert_eq!(home.umx(&["daemon", "uninstall"]).1, 0);
    assert!(!home.unit_file().exists());
}

/// F46: a hand-started daemon restarts in place: the same pid, every acknowledged write kept, its start flags kept,
/// and no socket of the old process left open in the new one.
#[test]
fn daemon_restart_replaces_a_hand_started_daemon_in_place() {
    let mut home = Home::new("restart");
    let pid = home.start_with(&["--idle-unload", "2h"]).data["pid"]
        .as_u64()
        .unwrap();
    let graph = home.path.join("g").display().to_string();
    home.ok(&["graph", "new", &graph, "--alias", "demo"]);
    // Control for the fd count: before the restart the daemon holds only its listener.
    assert_eq!(unix_sockets(pid), 1);
    let env = restart_while_writing(&home);
    assert_eq!(
        (env.data["how"].as_str(), env.data["pid"].as_u64()),
        (Some("in_place"), Some(pid))
    );
    // The restarted daemon kept --idle-unload 2h and holds one socket: its new listener, none inherited.
    home.ok(&["graph", "load", &graph]);
    let listed = home.ok(&["graph", "list"]);
    assert!(
        listed.data["rows"][0]["unloads_in_s"].as_u64().unwrap() > 3600,
        "{}",
        listed.to_json()
    );
    assert_eq!(unix_sockets(pid), 1);
}

/// F46: a client connected but silent while the daemon restarts sees the end of the stream or an error, never a hang.
#[test]
fn an_idle_client_during_a_restart_is_not_left_hanging() {
    let mut home = Home::new("restart-idle");
    home.start();
    let mut idle = UnixStream::connect(home.socket()).unwrap();
    idle.set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    let (env, exit) = home.umx(&["daemon", "restart"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    let started = Instant::now();
    let mut buf = [0u8; 64];
    if let Err(e) = idle.read(&mut buf) {
        assert!(
            !matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ),
            "the idle client hung: {e}"
        );
    }
    assert!(started.elapsed() < Duration::from_secs(10));
}

/// F46: with no daemon, restart starts nothing (D13).
#[test]
fn daemon_restart_never_starts_a_daemon_from_nothing() {
    let home = Home::new("restart-none");
    let (env, exit) = home.umx(&["daemon", "restart"]);
    assert_eq!(exit, 3, "{}", env.to_json());
    assert_eq!(
        issue(&env, Code::DaemonNotRunning).next(),
        "umx daemon start"
    );
    assert!(!home.socket().exists());
}

/// F46: a loaded service restarts through the service manager, never by hand.
#[test]
fn daemon_restart_goes_through_the_service_manager_when_installed() {
    let home = Home::new("restart-svc");
    let old = home.run_service();
    let graph = home.path.join("g").display().to_string();
    home.ok(&["graph", "new", &graph, "--alias", "demo"]);
    let env = restart_while_writing(&home);
    assert_eq!(env.data["how"], "service");
    // The reply is from the new daemon, even when the manager returns before the old one is gone.
    assert_ne!(env.data["pid"], old, "{}", env.to_json());
    let argv = home.argv();
    // Review B5: the old daemon drained its writer and stopped serving before the manager was asked.
    assert!(!argv.contains("serving"), "{argv}");
    if cfg!(target_os = "macos") {
        assert!(argv.contains("kickstart -k gui/"), "{argv}");
    } else {
        assert!(
            argv.contains("--user restart umx-sandbox.service"),
            "{argv}"
        );
    }
}

/// ws012 s3b B1: the harness has ALL of a command's output once it exits, whoever still holds its writer and however
/// late it reads. Staged, not timed: a holder keeps the writer open for 120 s, and nothing reads until the command
/// has exited (a [`Captured`] reads only then). A reply bigger than a pipe proves no reader was needed meanwhile.
#[test]
fn output_is_whole_once_umx_exits_with_its_writer_held_and_no_reader_until_then() {
    let mut home = Home::new("writer-held");
    let pid_file = held_writer_stand_in(&mut home);
    let (env, exit) = home.umx(&["node", "add"]);
    let holder = fs::read_to_string(&pid_file).unwrap();
    let _ = Command::new("/bin/kill")
        .arg(holder.trim())
        .bounded_status();
    assert_eq!((exit, env.command.as_str()), (0, "node add"));
    assert_eq!(env.data["big"].as_str().map(str::len), Some(256 * 1024));
}

/// ws012 s3b B1, the control: the same stand-in and the same late reader on a pipe, the medium 790c090 read through.
/// A pipe's output is whole only if a reader keeps up while the command runs: read late, the command cannot even
/// finish writing, so the bound fails it. That dependence on when the reader runs is what the files remove.
#[test]
#[should_panic(expected = "did not finish within 2 s")]
fn a_pipe_read_only_after_exit_never_has_the_whole_reply() {
    let mut home = Home::new("writer-held-pipe");
    let pid_file = held_writer_stand_in(&mut home);
    let mut child = Command::new(home.binary)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let ended = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wait_within(&mut child, "umx-stand-in on a pipe", Duration::from_secs(2))
    }));
    if let Ok(holder) = fs::read_to_string(&pid_file) {
        let _ = Command::new("/bin/kill")
            .arg(holder.trim())
            .bounded_status();
    }
    if let Err(panic) = ended {
        std::panic::resume_unwind(panic);
    }
}

/// ws012 s3b: a planted hang fails at the bound, naming the command, instead of hanging the suite.
#[test]
#[should_panic(expected = "/bin/sleep [\"60\"] did not finish within 1 s")]
fn a_command_that_never_finishes_fails_at_the_bound() {
    Command::new("/bin/sleep")
        .arg("60")
        .spawn_captured()
        .unwrap()
        .finish(Duration::from_secs(1));
}

/// ws012 s3b: a daemon that never prints its ready line fails at the bound, naming it.
#[test]
#[should_panic(expected = "stand-in daemon printed no")]
fn a_daemon_that_never_gets_ready_fails_at_the_bound() {
    let mut child = Command::new("/bin/sleep")
        .arg("60")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    ready_line(&mut child, "stand-in daemon", Duration::from_secs(1));
}

/// Review B1: the service runs another UMX_HOME, so restarting this home's hand-started daemon happens in
/// place and leaves that service alone.
#[test]
fn daemon_restart_leaves_a_service_for_another_home_alone() {
    let a = Home::new("restart-home-a");
    a.run_service();
    let a_before = a.pid_and_start();
    let mut b = Home::new("restart-home-b");
    let tool = if cfg!(target_os = "macos") {
        "launchctl"
    } else {
        "systemctl"
    };
    // Same user and service files as a, another UMX_HOME.
    b.env = vec![
        ("HOME".into(), a.path.display().to_string()),
        (
            "UMX_SERVICE_TOOL".into(),
            a.path.join("bin").join(tool).display().to_string(),
        ),
    ];
    let pid = b.start().data["pid"].clone();
    let mark = a.argv().len();
    let (env, exit) = b.umx(&["daemon", "restart"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    assert_eq!(
        (&env.data["how"], &env.data["pid"]),
        (&json!("in_place"), &pid)
    );
    assert_eq!(a.pid_and_start(), a_before);
    a.no_manager_restart_since(mark);
}

/// Review B2: a service that is installed and loaded but not running is not started: restart needs a
/// running daemon (D13).
#[test]
fn daemon_restart_does_not_start_a_stopped_service() {
    let home = Home::new("restart-stopped");
    home.install();
    home.service_state("nopid", "");
    let mark = home.argv().len();
    let (env, exit) = home.umx(&["daemon", "restart"]);
    assert_eq!(exit, 3, "{}", env.to_json());
    issue(&env, Code::DaemonNotRunning);
    home.no_manager_restart_since(mark);
    assert!(!home.socket().exists());
}

/// Review B3: when the service cannot be inspected, restart says so and changes nothing.
#[test]
fn daemon_restart_stops_when_the_service_cannot_be_inspected() {
    let mut home = Home::new("restart-inspect");
    home.start();
    home.install();
    let before = home.pid_and_start();
    home.service_state("inspectfail", "");
    let mark = home.argv().len();
    let (env, exit) = home.umx(&["daemon", "restart"]);
    assert_ne!(exit, 0, "{}", env.to_json());
    issue(&env, Code::ServiceFailed);
    fs::remove_file(home.path.join("bin/inspectfail")).unwrap();
    assert_eq!(home.pid_and_start(), before);
    home.no_manager_restart_since(mark);
}

/// Review B4: a daemon that trickles its status reply one byte at a time cannot hold restart past its
/// deadline.
#[test]
fn daemon_restart_gives_up_on_a_trickling_daemon() {
    let home = Home::new("restart-trickle");
    home.owner_only();
    let status = home.healthy_status();
    let listener = UnixListener::bind(home.socket()).unwrap();
    std::thread::spawn(move || {
        let mut statuses = 0;
        for mut stream in listener.incoming().flatten() {
            let mut bytes = Vec::new();
            let _ = stream.read_to_end(&mut bytes);
            let command = Request::from_json(&bytes)
                .map(|r| r.command)
                .unwrap_or_default();
            if command == "daemon restart" {
                let ok = Envelope::ok("daemon restart", json!({ "restarting": true }));
                let _ = stream.write_all(ok.to_json().as_bytes());
                continue;
            }
            if command != "daemon status" {
                continue;
            }
            statuses += 1;
            if statuses == 1 {
                let _ = stream.write_all(status.to_json().as_bytes());
                continue;
            }
            // The restarted "daemon" answers, but never finishes the answer.
            std::thread::spawn(move || {
                while stream.write_all(b" ").is_ok() {
                    std::thread::sleep(Duration::from_millis(200));
                }
            });
        }
    });
    let started = Instant::now();
    let (env, exit) = home.umx(&["daemon", "restart"]);
    assert_eq!(exit, 3, "{}", env.to_json());
    issue(&env, Code::DaemonNotRunning);
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn models_fetch_dry_run_names_the_pin_and_writes_nothing() {
    let home = Home::new("models-dry");
    let cache = home.path.join("cache");
    let cache_text = cache.display().to_string();
    let (env, exit) = home.umx_with(
        &["models", "fetch", "embeddinggemma-2", "--dry-run"],
        &[("XDG_CACHE_HOME", &cache_text)],
    );
    assert_eq!(exit, 0, "{}", env.to_json());
    assert_eq!(env.data["repo"], "ggml-org/embeddinggemma-2-GGUF");
    assert_eq!(env.data["files"][0]["size"], 557_950_176u64);
    assert_eq!(env.data["needed"], 557_950_176u64);
    let folder = env.data["folder"].as_str().unwrap();
    assert!(folder.starts_with(&cache_text), "{folder}");
    assert!(!cache.exists(), "a dry run made {}", cache.display());
}

#[test]
fn models_fetch_takes_one_name() {
    let home = Home::new("models-usage");
    for bad in [
        &["models", "fetch"][..],
        &["models", "fetch", "a", "b"],
        &["models", "pull", "a"],
    ] {
        let (env, exit) = home.umx(bad);
        assert_eq!(exit, 2, "{bad:?}: {}", env.to_json());
        assert_eq!(
            issue(&env, Code::UsageInvalid).next(),
            "umx docs get models",
            "{bad:?}"
        );
    }
}

/// F50: a daemon whose reply starts and then stalls cannot hold the CLI: it gives up at the 10 s idle limit with
/// internal `reply_timed_out`.
#[test]
fn a_reply_that_stalls_gives_up_at_the_idle_limit() {
    let home = Home::new("reply-stall");
    home.owner_only();
    let listener = UnixListener::bind(home.socket()).unwrap();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut bytes = Vec::new();
            let _ = stream.read_to_end(&mut bytes);
            // Lifecycle requests (the teardown's `daemon stop`) get a closed stream at once.
            if Request::from_json(&bytes).is_ok_and(|r| r.op.is_some()) {
                std::thread::spawn(move || {
                    let _ = stream.write_all(b"{");
                    std::thread::sleep(Duration::from_secs(30));
                });
            }
        }
    });
    let started = Instant::now();
    let (env, exit) = home.umx(&["graph", "list"]);
    let took = started.elapsed();
    assert_ne!(exit, 0, "{}", env.to_json());
    assert!(
        issue(&env, Code::Internal)
            .message()
            .contains("reply_timed_out"),
        "{}",
        env.to_json()
    );
    assert!(
        took >= Duration::from_secs(10) && took < Duration::from_secs(15),
        "{took:?}"
    );
}

/// F56: with 64 connections held open, a command gets daemon_busy (exit 1) and nothing runs, while `daemon status`
/// still answers through the reserved slots; once they close, commands run again.
#[test]
fn a_full_daemon_is_busy_but_answers_status() {
    let mut home = Home::new("busy");
    home.start();
    let held: Vec<UnixStream> = (0..64)
        .map(|_| UnixStream::connect(home.socket()).unwrap())
        .collect();
    let deadline = Instant::now() + Duration::from_secs(10);
    let busy = loop {
        let (env, exit) = home.umx(&["graph", "list"]);
        if env
            .errors
            .first()
            .is_some_and(|e| e.code() == "daemon_busy")
        {
            break (env, exit);
        }
        assert!(Instant::now() < deadline, "never busy: {}", env.to_json());
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(busy.1, 1, "{}", busy.0.to_json());
    assert_eq!(issue(&busy.0, Code::DaemonBusy).next(), "umx daemon status");
    let (status, exit) = home.umx(&["daemon", "status"]);
    assert_eq!(
        (exit, &status.data["running"]),
        (0, &json!(true)),
        "{}",
        status.to_json()
    );
    drop(held);
    let deadline = Instant::now() + Duration::from_secs(10);
    while home.umx(&["graph", "list"]).1 != 0 {
        assert!(
            Instant::now() < deadline,
            "still busy after the connections closed"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Home::umx reads umx's stdout while umx runs, so a reply bigger than a pipe's buffer (64 KiB on macOS) comes back
/// instead of leaving umx blocked on its write (ws004 s25: 400 instances' status rows passed 64 KiB).
#[test]
fn a_reply_bigger_than_a_pipe_buffer_comes_back() {
    let mut home = Home::new("bigreply");
    home.write_config(&many_instances(400));
    home.start();
    let (status, exit) = home.umx(&["daemon", "status"]);
    assert_eq!(exit, 0);
    let len = status.to_json().len();
    assert!(
        len > 64 * 1024,
        "{len} bytes: the reply no longer outgrows a pipe buffer, so this checks nothing"
    );
}

/// Review R129-B1: 72 clients ask for a big status reply (400 embedder instances) and never read it. Each reply write gives up at its
/// deadline (2 s in the lane), so `daemon status` answers again within seconds instead of staying busy for good. A
/// reply only blocks when it is bigger than the socket buffers (macOS; Linux buffers hold it).
#[test]
fn replies_nobody_reads_do_not_pin_the_daemon() {
    let mut home = Home::new("unread");
    home.write_config(&many_instances(400));
    home.start();
    let (status, _) = home.umx(&["daemon", "status"]);
    assert!(
        status.to_json().len() > 30_000,
        "{}",
        status.to_json().len()
    );
    let build = Build {
        version: "t".into(),
        commit: "t".into(),
        built: "t".into(),
    };
    let ask = Request::new("daemon status", Vec::new(), build).to_json();
    let held: Vec<UnixStream> = (0..72)
        .map(|_| {
            let mut s = UnixStream::connect(home.socket()).unwrap();
            s.write_all(ask.as_bytes()).unwrap();
            s.shutdown(std::net::Shutdown::Write).unwrap();
            s
        })
        .collect();
    std::thread::sleep(Duration::from_millis(500));
    let started = Instant::now();
    loop {
        let (env, exit) = home.umx(&["daemon", "status"]);
        if exit == 0 && env.data["running"] == true {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "still pinned: {}",
            env.to_json().chars().take(300).collect::<String>()
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    drop(held);
}

/// F64 (architect note f:64): a client connecting in a tight loop while the daemon replaces itself is never carried
/// into the new process. On macOS a connection accepted just as the image is replaced used to survive unread, so
/// the new daemon held two sockets and restart's wait for it ran out.
#[test]
fn clients_connecting_during_a_restart_are_not_carried_into_the_new_daemon() {
    let mut home = Home::new("restart-hammer");
    let pid = home.start().data["pid"].as_u64().unwrap();
    let socket = home.socket();
    let stop = std::sync::atomic::AtomicBool::new(false);
    let build = Build {
        version: "t".into(),
        commit: "t".into(),
        built: "t".into(),
    };
    let ask = Request::new("daemon status", Vec::new(), build).to_json();
    let (env, exit) = std::thread::scope(|scope| {
        scope.spawn(|| {
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                if let Ok(mut s) = UnixStream::connect(&socket) {
                    let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
                    let _ = s.write_all(ask.as_bytes());
                    let _ = s.shutdown(std::net::Shutdown::Write);
                    let mut reply = Vec::new();
                    let _ = s.read_to_end(&mut reply);
                }
            }
        });
        std::thread::sleep(Duration::from_millis(50));
        let restarted = home.umx(&["daemon", "restart"]);
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        restarted
    });
    assert_eq!(exit, 0, "{}", env.to_json());
    assert_eq!(
        (env.data["how"].as_str(), env.data["pid"].as_u64()),
        (Some("in_place"), Some(pid))
    );
    // The hammer's connections are closed; the new daemon holds its listener and nothing else.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(unix_sockets(pid), 1);
}

/// ws002 F72: graphs the registry remembers come back after an in-place restart (F64) and after stop then start. A
/// graph whose folder is gone stays unloaded and is reported by daemon status and doctor; one unloaded on purpose
/// stays away, and graph_not_loaded says how to load it.
#[test]
fn graphs_come_back_after_a_daemon_restart() {
    let mut home = Home::new("f72-restart");
    let demo = home.graph();
    let folder = |alias: &str| {
        let path = home.path.join(alias).display().to_string();
        home.ok(&["graph", "new", &path, "--alias", alias]);
        real(&path)
    };
    let (other, gone, away) = (folder("other"), folder("gone"), folder("away"));
    home.ok(&["graph", "unload", "away"]);
    let file = home.path.join("registry/graphs.toml");
    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(file.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );

    let before = home.ok(&["daemon", "status"]);
    let restarted = home.ok(&["daemon", "restart"]);
    assert_eq!(restarted.data["pid"], before.data["pid"], "in place");
    assert_eq!(home.aliases(), ["demo", "gone", "other"]);

    fs::remove_dir_all(&gone).unwrap();
    home.restart();
    assert_eq!(home.aliases(), ["demo", "other"], "the rest come back");
    let status = home.ok(&["daemon", "status"]);
    let registry = &status.data["registry"];
    assert_eq!(
        registry["not_reloaded"],
        json!([{ "alias": "gone", "folder": gone, "code": "graph_not_found" }]),
        "{}",
        status.to_json()
    );
    assert_eq!(registry["reloaded"].as_array().unwrap().len(), 2);
    let (doctor, exit) = home.umx(&["doctor"]);
    assert_eq!(exit, 5, "{}", doctor.to_json());
    let found = issue(&doctor, Code::DoctorGraphNotReloaded);
    assert_eq!(found.next(), format!("umx graph load {gone} --as gone"));
    assert!(
        found.message().contains("graph_not_found"),
        "{}",
        found.message()
    );
    // Named, the on-demand load runs and returns the load's own error.
    let (env, exit) = home.umx(&["node", "list", "--graph", "gone"]);
    assert_eq!((exit, env.errors[0].code()), (1, "graph_not_found"));

    // Unloaded on purpose: it does not come back, and nothing loads it on demand.
    let (env, exit) = home.umx(&["node", "list", "--graph", "away"]);
    assert_eq!((exit, env.errors[0].code()), (1, "graph_not_loaded"));
    let found = issue(&env, Code::GraphNotLoaded);
    assert_eq!(
        found.message(),
        "Graph away is not loaded: it was unloaded."
    );
    assert_eq!(found.next(), format!("umx graph load {away} --as away"));
    // An alias umx never had says when the daemon started.
    let (env, _) = home.umx(&["node", "list", "--graph", "never"]);
    let found = issue(&env, Code::GraphNotLoaded);
    assert!(
        found.message().contains("the daemon started at 20"),
        "{}",
        found.message()
    );
    assert_eq!(found.next(), "umx graph list");

    // Unloading the remembered graph that did not reload forgets it.
    home.ok(&["graph", "unload", "gone"]);
    home.restart();
    let status = home.ok(&["daemon", "status"]);
    assert_eq!(status.data["registry"]["not_reloaded"], json!([]));
    assert_eq!(home.aliases(), ["demo", "other"]);
    let _ = (demo, other);
}

/// F80 (d-21 across a restart, graph-addressing-expert and architect): a graph id whose primary was unloaded with
/// two copies left still has no primary after a restart, so a read that names no graph is copy_ambiguous naming
/// both, and naming one works. If only one copy comes back, it is primary again and status says so.
#[test]
fn a_graph_without_a_primary_stays_so_across_a_restart() {
    let mut home = Home::new("f80");
    home.design();
    home.design_copy("design_b");
    home.design_copy("design_c");
    home.ok(&["graph", "unload", "design"]);
    let count = "MATCH (n) RETURN count(*) AS nodes";
    home.restart();
    let (env, exit) = home.query(count, &[]);
    assert_eq!(exit, 1, "{}", env.to_json());
    let refusal = issue(&env, Code::CopyAmbiguous);
    let named: Vec<&str> = refusal
        .candidates()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(named, ["design_b", "design_c"]);
    let (env, exit) = home.query(count, &["--graph", "design_b"]);
    assert_eq!(exit, 0, "{}", env.to_json());

    home.stop();
    fs::rename(home.path.join("design_c"), home.path.join("moved")).unwrap();
    home.start();
    let status = home.ok(&["daemon", "status"]);
    assert_eq!(
        status.data["registry"]["primary_restored"],
        json!(["design_b"])
    );
    issue(&status, Code::PrimaryRestored);
    let (env, exit) = home.query(count, &[]);
    assert_eq!(exit, 0, "the one copy back is primary: {}", env.to_json());
}

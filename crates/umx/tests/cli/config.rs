//! The cli tests: config, roots, embedders, probes and graph policy.
//! A module of cli.rs, whose `Home` and helpers it uses, in its own file so no file nears the seed gate's
//! size cap.

use super::*;

#[test]
fn path_outside_roots_names_the_roots_and_where_to_read_more() {
    let mut home = Home::new("roots");
    home.start();
    let (env, exit) = home.umx(&["graph", "new", "/umx-outside-roots/g", "--alias", "x"]);
    assert_eq!(exit, 1);
    let found = issue(&env, Code::PathOutsideRoots);
    let root = home.path.display().to_string();
    assert!(
        found.message().contains(&format!("use: {root}.")),
        "{}",
        found.message()
    );
    assert!(found.message().contains("UMX_ROOTS"), "{}", found.message());
    assert_eq!(found.next(), "umx docs get graph");
}

#[test]
fn invalid_roots_stay_closed_until_the_human_chooses_roots() {
    let mut home = Home::new("cfg-roots");
    home.roots_env = false;
    home.write_config("roots = [\"relative\"]\n");
    let ready = home.start();
    assert_closed(&home, &ready);
    let (check, exit) = home.umx(&["config", "check"]);
    assert_eq!(exit, 1);
    // umx never suggests a roots value (usage note 031): the fix is the guidance, not a folder list.
    let fix = issue(&check, Code::ConfigValueInvalid).next();
    assert_eq!(fix, "umx docs get config");
    // The human chooses the roots; after a restart a graph can be made there.
    let chosen = home.path.display().to_string();
    let set = home.ok(&["config", "set", "roots", &chosen]);
    issue(&set, Code::ConfigRestartNeeded);
    let ready = home.restart();
    assert_open(&home, &ready);
    let path = home.path.join("g").display().to_string();
    home.ok(&["graph", "new", &path, "--alias", "demo"]);
}

#[test]
fn an_unrelated_rewrite_never_drops_or_repairs_invalid_roots() {
    let mut home = Home::new("cfg-keep");
    home.roots_env = false;
    let file = home.write_config("roots = false\nembedder.active = \"none\"\n");
    let ready = home.start();
    assert_closed(&home, &ready);
    home.ok(&["config", "set", "daemon.idle_unload", "1h"]);
    home.ok(&["config", "unset", "embedder.active"]);
    let text = fs::read_to_string(&file).unwrap();
    assert!(text.contains("roots = false\n"), "{text}");
    assert!(!text.contains("embedder.active"), "{text}");
    let ready = home.restart();
    assert_closed(&home, &ready);
}

#[test]
fn replacing_a_secret_shaped_value_never_echoes_it() {
    let mut home = Home::new("cfg-was");
    let planted = "sk-PR50_FAKE_SECRET_DO_NOT_USE";
    home.write_config(&format!(
        "embedders.az.kind = \"azure_openai\"\nembedders.az.deployment = \"{planted}\"\n"
    ));
    home.start();
    let (check, _) = home.umx(&["config", "check"]);
    issue(&check, Code::ConfigValueInvalid);
    let set = home.ok(&["config", "set", "embedders.az.deployment", "safe"]);
    assert_eq!(
        (&set.data["was"], &set.data["replaced"]),
        (&json!(null), &json!(true))
    );
    let again = home.ok(&["config", "set", "embedders.az.deployment", "other"]);
    assert_eq!(
        (&again.data["was"], &again.data["replaced"]),
        (&json!("safe"), &json!(true))
    );
    for env in [check, set, again] {
        assert!(!env.to_json().contains("PR50_FAKE"), "{}", env.to_json());
    }
}

#[test]
fn an_unparseable_config_closes_folders_and_is_never_rewritten() {
    let mut home = Home::new("cfg-parse");
    let file = home.write_config("embedder.active = \"none\"\nroots = [\n");
    let ready = home.start();
    assert_closed(&home, &ready);
    let (env, exit) = home.umx(&["config", "set", "daemon.idle_unload", "1h"]);
    let refused = issue(&env, Code::ConfigFileInvalid);
    assert_eq!((exit, refused.next()), (1, "umx config check"));
    assert!(
        refused.message().contains("line 2 column 10"),
        "{}",
        refused.message()
    );
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "embedder.active = \"none\"\nroots = [\n",
        "a file umx cannot read is never rewritten"
    );
    home.umx(&["config", "unset", "roots"]);
    assert_eq!(fs::read_to_string(&file).unwrap().len(), 35);
    let (check, _) = home.umx(&["config", "check"]);
    assert_eq!(
        issue(&check, Code::ConfigFileInvalid).next(),
        "umx docs get config"
    );
    // show says what is in force: no roots, not the defaults.
    let roots = setting(&home.ok(&["config", "show"]), "roots").clone();
    assert_eq!(
        (&roots["value"], &roots["source"]),
        (&json!(null), &json!("invalid"))
    );
    fs::write(&file, "embedder.active = \"none\"\n").unwrap();
    let ready = home.restart();
    assert_open(&home, &ready);
}

/// F51 (review R108-F1): a config the owner cannot open (0022) closes the folders and is still
/// config_file_refused with its chmod repair, not write_failed.
#[test]
fn an_unopenable_config_file_is_refused_not_a_write_failure() {
    let mut home = Home::new("cfg-unopenable");
    let file = home.write_config("daemon.idle_unload = \"1h\"\n");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o022)).unwrap();
    let ready = home.start();
    assert_closed(&home, &ready);
    let write_failed = Code::WriteFailed.spec().code;
    for args in [
        &["config", "set", "daemon.idle_unload", "2h"][..],
        &["config", "unset", "daemon.idle_unload"],
    ] {
        let (env, exit) = home.umx(args);
        assert_eq!(exit, 1, "{args:?}");
        let refused = issue(&env, Code::ConfigFileRefused);
        assert!(
            refused.message().contains("chmod 600"),
            "{}",
            refused.message()
        );
        assert!(
            !env.errors
                .iter()
                .chain(&env.warnings)
                .any(|i| i.code() == write_failed),
            "{}",
            env.to_json()
        );
    }
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    let ready = home.restart();
    assert_open(&home, &ready);
}

#[test]
fn a_refused_config_file_closes_folders_until_it_is_owner_only() {
    let mut home = Home::new("cfg-refused");
    let file = home.write_config("daemon.idle_unload = \"1h\"\n");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o666)).unwrap();
    let ready = home.start();
    assert_closed(&home, &ready);
    for args in [
        &["config", "set", "daemon.idle_unload", "2h"][..],
        &["config", "unset", "daemon.idle_unload"],
    ] {
        let (env, exit) = home.umx(args);
        let refused = issue(&env, Code::ConfigFileRefused);
        assert_eq!(exit, 1);
        assert!(
            refused.message().contains("chmod 600"),
            "{}",
            refused.message()
        );
    }
    assert_eq!(mode(&file), 0o666, "nothing was written");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    let ready = home.restart();
    assert_open(&home, &ready);
    assert_eq!(home.ok(&["daemon", "status"]).data["running"], true);
}

#[test]
fn show_names_each_value_and_where_it_came_from() {
    let mut home = Home::new("cfg-show");
    let repo = home.repo();
    home.write_config("daemon.idle_unload = \"2h\"\n");
    home.start_with(&["--idle-unload", "5m"]);
    home.nomic();
    home.ok(&["config", "set", "embedder.active", "nomic", "--repo"]);
    let show = home.ok(&["config", "show"]);
    let machine = home.path.join("config.toml").display().to_string();
    // The repo is found by its physical path (/tmp is /private/tmp on macOS).
    let repo_file = repo
        .canonicalize()
        .unwrap()
        .join(".umx/config.toml")
        .display()
        .to_string();
    assert_eq!(show.data["files"]["machine"], machine);
    assert_eq!(show.data["files"]["repo"], repo_file);
    assert_eq!(show.data["problems"], 0);
    let roots = setting(&show, "roots");
    assert_eq!(
        (&roots["source"], &roots["env"]),
        (&"env".into(), &"UMX_ROOTS".into())
    );
    let idle = setting(&show, "daemon.idle_unload");
    assert_eq!(
        (&idle["source"], &idle["applies"]),
        (&"flag".into(), &"daemon restart".into())
    );
    let active = setting(&show, "embedder.active");
    assert_eq!(
        (&active["value"], &active["source"]),
        (&"nomic".into(), &"repo".into())
    );
    assert_eq!(active["file"], repo_file);
    let kind = setting(&show, "embedders.nomic.kind");
    assert_eq!(
        (&kind["source"], &kind["file"]),
        (&"machine".into(), &machine.clone().into())
    );
    assert_eq!(setting(&show, "embedders.nomic.auth")["source"], "default");

    // Run from elsewhere, the repo layer does not apply.
    home.cwd = Some(home.path.clone());
    let show = home.ok(&["config", "show"]);
    assert_eq!(setting(&show, "embedder.active")["source"], "default");
    assert!(show.data["files"]["repo"].is_null());
}

#[test]
fn set_and_unset_write_a_deterministic_owner_only_file() {
    let mut home = Home::new("cfg-write");
    home.start();
    home.nomic();
    home.ok(&["config", "set", "embedder.active", "nomic"]);
    let file = home.path.join("config.toml");
    let first = fs::read_to_string(&file).unwrap();
    assert_eq!(mode(&file), 0o600);
    assert_eq!(
        first.lines().skip(1).collect::<Vec<_>>(),
        [
            "embedder.active = \"nomic\"",
            "embedders.nomic.kind = \"local\"",
            "embedders.nomic.model = \"nomic-embed-text-v1.5\"",
            "embedders.nomic.dims = 256",
        ]
    );
    // The same settings in another order give the same bytes.
    fs::remove_file(&file).unwrap();
    home.ok(&["config", "set", "embedder.active", "none"]);
    home.ok(&["config", "set", "embedders.nomic.dims", "256"]);
    home.ok(&[
        "config",
        "set",
        "embedders.nomic.model",
        "nomic-embed-text-v1.5",
    ]);
    home.ok(&["config", "set", "embedders.nomic.kind", "local"]);
    let set = home.ok(&["config", "set", "embedder.active", "nomic"]);
    assert_eq!(
        (&set.data["was"], &set.data["layer"]),
        (&"none".into(), &"machine".into())
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), first);

    let unset = home.ok(&["config", "unset", "embedders.nomic"]);
    assert_eq!(unset.data["removed"], 3);
    let (check, _) = home.umx(&["config", "check"]);
    issue(&check, Code::ConfigInstanceUnknown);
    home.ok(&["config", "unset", "embedder.active"]);
    let (check, exit) = home.umx(&["config", "check"]);
    assert_eq!((exit, check.status), (0, Status::Ok), "{}", check.to_json());
}

#[test]
fn a_repo_config_only_narrows() {
    let mut home = Home::new("cfg-narrow");
    let repo = home.repo();
    home.start();
    home.nomic();
    home.az();
    home.ok(&["config", "set", "embedder.active", "az"]);
    // Instances and embedder.active apply at daemon start; restart so check reports only the repo file.
    home.restart();
    let (env, _) = home.umx(&["config", "set", "embedder.active", "az", "--repo"]);
    assert!(
        issue(&env, Code::ConfigRepoRefused)
            .message()
            .contains("local")
    );
    for key in ["embedders.az.endpoint", "roots"] {
        let (env, _) = home.umx(&["config", "set", key, "https://evil.example", "--repo"]);
        issue(&env, Code::ConfigRepoRefused);
    }
    home.ok(&["config", "set", "embedder.active", "nomic", "--repo"]);
    // A hand-written (or cloned) repo file that tries to widen is ignored, loudly.
    fs::write(
        repo.join(".umx/config.toml"),
        "embedder.active = \"az\"\nembedders.az.endpoint = \"https://evil.example\"\n",
    )
    .unwrap();
    let (check, exit) = home.umx(&["config", "check"]);
    assert_eq!((exit, check.warnings.len()), (0, 2), "{}", check.to_json());
    let show = home.ok(&["config", "show"]);
    assert_eq!(setting(&show, "embedder.active")["source"], "machine");
    assert_eq!(
        setting(&show, "embedders.az.endpoint")["value"],
        "https://example.openai.azure.com"
    );
}

#[test]
fn secrets_are_shown_only_as_set_or_unset() {
    let sentinel = "sentinel0123456789abcdef0123456789abcdef";
    let mut home = Home::new("cfg-secret");
    fs::create_dir_all(&home.path).unwrap();
    let secrets = home.path.join("secrets.env");
    fs::write(&secrets, format!("FILE_KEY={sentinel}\n")).unwrap();
    fs::set_permissions(&secrets, fs::Permissions::from_mode(0o600)).unwrap();
    home.env.push(("ENV_KEY".into(), sentinel.into()));
    home.start();
    home.az();
    home.ok(&["config", "set", "embedders.az.auth", "api_key"]);
    let mut seen = String::new();
    for (name, shown) in [
        ("FILE_KEY", "set (secrets.env)"),
        ("ENV_KEY", "set (env)"),
        ("NO_KEY", "unset"),
    ] {
        seen += &home
            .ok(&["config", "set", "embedders.az.api_key_env", name])
            .to_json();
        let show = home.ok(&["config", "show"]);
        assert_eq!(setting(&show, "embedders.az.api_key_env")["secret"], shown);
        seen += &show.to_json();
        seen += &home.umx(&["config", "check"]).0.to_json();
    }
    seen += &fs::read_to_string(home.path.join("config.toml")).unwrap();
    assert!(
        !seen.contains("sentinel"),
        "a secret value reached output or the config file"
    );
    // A value shaped like a key is refused, so it never lands in the file.
    let (env, _) = home.umx(&["config", "set", "embedders.az.deployment", sentinel]);
    assert!(!env.to_json().contains("sentinel"));
    issue(&env, Code::ConfigValueInvalid);
    // An open secrets file is ignored, and check says how to fix it.
    fs::set_permissions(&secrets, fs::Permissions::from_mode(0o644)).unwrap();
    home.ok(&["config", "set", "embedders.az.api_key_env", "FILE_KEY"]);
    let (check, _) = home.umx(&["config", "check"]);
    issue(&check, Code::ConfigFileRefused);
    issue(&check, Code::SecretMissing);
}

/// req-73, neither credential: auth cli (the default) with no az login (no az or azd on PATH) fails before
/// anything is sent, and the error names `az login` as the fix.
#[test]
fn probe_with_no_az_login_names_az_login() {
    let mut home = Home::new("no-login");
    home.az_active("");
    let ready = home.start();
    assert_eq!(ready.status, Status::Ok, "{}", ready.to_json());
    let (env, exit) = home.umx(&["embeddings", "probe"]);
    assert_eq!(exit, 1);
    let found = issue(&env, Code::EmbedderUnavailable);
    assert!(found.message().contains("az login"), "{}", found.message());
    assert!(
        found.message().contains("not found on PATH"),
        "{}",
        found.message()
    );
    assert_eq!(found.next(), "umx embeddings probe");
    let again = home.umx(&["embeddings", "probe", "--embedder", "az"]).0;
    assert_eq!(
        issue(&again, Code::EmbedderUnavailable).next(),
        "umx embeddings probe --embedder az"
    );
    let status = home.ok(&["daemon", "status"]);
    let embedder = &status.data["embedder"];
    assert_eq!(embedder["active"], "az");
    assert_eq!(embedder["credential"], json!(null));
    assert_eq!(embedder["az"], "az", "not found on PATH, so run by name");
    assert_eq!(status.data["adapters"]["embedder"], "azure-openai.embedder");
}

/// req-73, az login: the token comes from az found on PATH; the probe then reaches for the endpoint (a closed local
/// port here, so it fails as unreachable, naming it). Status names the credential that worked and az's path, never
/// the token.
#[test]
fn probe_with_az_login_signs_in_and_status_names_az_never_the_token() {
    let mut home = Home::new("az-login");
    let bin = home.az_active("");
    let az = fake_az(&bin, "tok-DO-NOT-LEAK");
    home.start();
    let (env, exit) = home.umx(&["embeddings", "probe"]);
    assert_eq!(exit, 1);
    let found = issue(&env, Code::EmbedderUnavailable);
    assert!(
        found
            .message()
            .contains("could not reach https://127.0.0.1:9"),
        "{}",
        found.message()
    );
    let all = printed(&home, &env);
    assert!(!all.contains("DO-NOT-LEAK"), "{all}");
    let status = home.ok(&["daemon", "status"]);
    let embedder = &status.data["embedder"];
    assert_eq!(
        embedder["credential"],
        json!({ "kind": "az", "from": az.display().to_string() })
    );
    assert_eq!(embedder["az"], az.display().to_string());
}

/// req-46: a secured graph never reaches a hosted instance, whatever embedder.active or --embedder say; az is never
/// asked for a token. Its policy is read from graph.json.
#[test]
fn a_secured_graph_is_never_probed_with_a_hosted_embedder() {
    let mut home = Home::new("secured");
    let bin = home.az_active("");
    let log = bin.join("ran");
    let az = bin.join("az");
    fs::write(
        &az,
        format!("#!/bin/sh\n/usr/bin/touch '{}'\nexit 1\n", log.display()),
    )
    .unwrap();
    fs::set_permissions(&az, fs::Permissions::from_mode(0o755)).unwrap();
    home.start();
    let path = home.path.join("g");
    home.ok(&["graph", "new", path.to_str().unwrap(), "--alias", "demo"]);
    home.ok(&["graph", "unload", "demo"]);
    let file = path.join("graph.json");
    let mut meta = umx_protocol::parse_json(&fs::read_to_string(&file).unwrap()).unwrap();
    meta["umx"] = json!(2);
    meta["embedding"] = json!({ "key": null, "text_form": 1, "policy": "secured" });
    fs::write(&file, meta.to_string()).unwrap();
    home.ok(&["graph", "load", path.to_str().unwrap()]);
    for args in [
        &["embeddings", "probe", "--graph", "demo"][..],
        &["embeddings", "probe", "--graph", "demo", "--embedder", "az"],
    ] {
        let (env, exit) = home.umx(args);
        assert_eq!(exit, 1, "{}", env.to_json());
        let found = issue(&env, Code::EmbedderUnavailable);
        assert!(found.message().contains("secured"), "{}", found.message());
        // ws004 s08 done_when: the error names the graph and the instance.
        assert!(
            found.message().contains("graph demo") && found.message().contains("az is hosted"),
            "{}",
            found.message()
        );
    }
    assert!(!log.exists(), "az ran for a secured graph");
    // The same graph opened: the probe goes ahead and asks az.
    let open = home.umx(&["embeddings", "probe"]).0;
    assert!(
        issue(&open, Code::EmbedderUnavailable)
            .message()
            .contains("az exited")
    );
    assert!(log.exists());
}

/// The daemon builds the instances and wires `embedder.active` when it starts, so one embedder serves every graph
/// and the embed worker until a restart: changing either warns `config_restart_needed`, and the restart applies it.
/// The probe reads the config as it is now, so it can check an instance before the restart, if the daemon built it.
#[test]
fn embedder_choice_applies_at_restart() {
    let mut home = Home::new("switch");
    home.az_active("");
    home.start();
    let status = home.ok(&["daemon", "status"]);
    assert_eq!(status.data["adapters"]["embedder"], "azure-openai.embedder");
    let set = home.umx(&["config", "set", "embedder.active", "none"]).0;
    issue(&set, Code::ConfigRestartNeeded);
    let status = home.ok(&["daemon", "status"]);
    assert_eq!(status.data["adapters"]["embedder"], "azure-openai.embedder");
    let probe = home.umx(&["embeddings", "probe"]).0;
    issue(&probe, Code::EmbedderNotConfigured);
    let set = home
        .umx(&["config", "set", "embedders.nomic.kind", "local"])
        .0;
    issue(&set, Code::ConfigRestartNeeded);
    home.nomic();
    home.ok(&["config", "set", "embedder.active", "nomic"]);
    // ws004 s27: the probe builds an instance added since start from the config now (here its model is not one umx
    // ships, so it is not set up right) instead of refusing it as not defined at start; the daemon still runs none.
    let (env, _) = home.umx(&["embeddings", "probe"]);
    assert!(
        issue(&env, Code::EmbedderInvalid)
            .message()
            .contains("embedders.nomic.model"),
        "{}",
        env.to_json()
    );
    home.ok(&["config", "set", "embedder.active", "none"]);
    home.restart();
    let status = home.ok(&["daemon", "status"]);
    assert_eq!(status.data["adapters"]["embedder"], json!(null));
}

#[test]
fn an_edited_graph_json_never_lowers_a_secured_graph() {
    // Security's three shapes: the value edited, the policy key deleted, the embedding section deleted.
    type Edit = fn(&mut Value);
    let shapes: [(&str, Edit); 3] = [
        ("value", |v| v["embedding"]["policy"] = json!("open")),
        ("key", |v| {
            v["embedding"].as_object_mut().unwrap().remove("policy");
        }),
        ("section", |v| {
            v.as_object_mut().unwrap().remove("embedding");
        }),
    ];
    for (name, edit) in shapes {
        let mut home = Home::new(&format!("downgrade-{name}"));
        let path = home.secured_graph();
        home.umx(&["graph", "unload", "demo"]);
        edit_graph_json(&path, edit);
        let (env, exit) = home.umx(&["graph", "load", &path]);
        assert_eq!(exit, 0, "{name}: {}", env.to_json());
        let found = issue(&env, Code::PolicyDowngradeUnconfirmed);
        assert!(
            found.message().contains("stays secured"),
            "{name}: {}",
            found.message()
        );
        assert_eq!(
            found.next(),
            "umx graph confirm-policy demo",
            "its owner's option, never run by umx"
        );
        assert_eq!(env.data["graph"]["effective_policy"], "secured", "{name}");
        assert_eq!(
            listed(&home, "demo")["effective_policy"],
            "secured",
            "{name}"
        );
    }
}

#[test]
fn a_secured_graph_stays_secured_with_a_new_id_or_in_a_new_folder() {
    // By folder: the id in graph.json changed too.
    let mut home = Home::new("downgrade-id");
    let path = home.secured_graph();
    home.umx(&["graph", "unload", "demo"]);
    edit_graph_json(&path, |v| {
        v["id"] = json!("01JC8X0000000000000000000Z");
        v["embedding"]["policy"] = json!("open");
    });
    let (env, _) = home.umx(&["graph", "load", &path]);
    assert!(
        has(&env, Code::PolicyDowngradeUnconfirmed),
        "{}",
        env.to_json()
    );
    assert_eq!(env.data["graph"]["effective_policy"], "secured");

    // By id: the folder moved (a copy, a worktree).
    let mut home = Home::new("downgrade-move");
    let path = home.secured_graph();
    home.umx(&["graph", "unload", "demo"]);
    let moved = home.path.join("moved").display().to_string();
    fs::rename(&path, &moved).unwrap();
    edit_graph_json(&moved, |v| v["embedding"]["policy"] = json!("open"));
    let (env, _) = home.umx(&["graph", "load", &moved]);
    assert!(
        has(&env, Code::PolicyDowngradeUnconfirmed),
        "{}",
        env.to_json()
    );
    assert_eq!(env.data["graph"]["effective_policy"], "secured");
}

#[test]
fn an_unusable_state_file_counts_every_graph_as_secured_and_is_left_alone() {
    let mut home = Home::new("state-corrupt");
    let path = home.graph();
    for (bytes, mode) in [
        (&b"{\"umx_state\": 1, \"graphs\": [oops"[..], 0o600),
        (b"{}", 0o644),
        (b"", 0o600),
    ] {
        let file = home.state_file(bytes);
        fs::set_permissions(&file, fs::Permissions::from_mode(mode)).unwrap();
        let (env, exit) = home.umx(&["graph", "load", &path]);
        assert_eq!(exit, 0);
        assert!(has(&env, Code::LocalStateUnreadable), "{}", env.to_json());
        assert_eq!(env.data["graph"]["effective_policy"], "secured");
        // Raising the policy would record it; the unusable file is never rewritten.
        home.umx(&["graph", "set", "demo", "--policy", "secured"]);
        assert_eq!(fs::read(&file).unwrap(), bytes, "mode {mode:o}");
    }
}

/// D83: an agent (no terminal, nothing typed) lowers a secured graph in one call. The effective policy changes at
/// once, the record holds the downgrade with its time, and the reply warns what loosened; a raise needs nothing.
#[test]
fn an_agent_lowers_a_graph_in_one_call_and_is_warned() {
    let mut home = Home::new("agent-lowers");
    home.secured_graph();
    home.no_tty = true;
    let (env, exit) = home.umx(&["graph", "set", "demo", "--embedder", "trusted"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    let found = issue(&env, Code::PolicyLowered);
    assert!(
        found.message().contains(
            "is now named+trusted on this machine (it was secured): embedder rule local to trusted."
        ),
        "{}",
        found.message()
    );
    assert!(
        found
            .message()
            .contains("The change is recorded on this machine at 20"),
        "{}",
        found.message()
    );
    let row = listed(&home, "demo");
    assert_eq!(row["effective_policy"], "named+trusted", "{row}");
    assert!(
        row["downgraded"].as_str().is_some_and(|d| d.ends_with('Z')),
        "{row}"
    );
    let record = fs::read_to_string(home.path.join("state/graph-policies.json")).unwrap();
    assert!(record.contains("secured to named+trusted"), "{record}");
    // A raise needs nothing and warns nothing.
    let (env, exit) = home.umx(&["graph", "set", "demo", "--policy", "secured"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    assert!(!has(&env, Code::PolicyLowered), "{}", env.to_json());
    let row = listed(&home, "demo");
    assert_eq!(
        (row["effective_policy"].clone(), row["downgraded"].clone()),
        (json!("secured"), json!(null))
    );
}

/// D83, the prime's ruling (b): a graph.json lowered outside umx (a pull, an edit) never loosens the graph by itself;
/// one explicit umx call by any caller does, and warns.
#[test]
fn a_pulled_lower_policy_waits_for_one_umx_call_by_any_caller() {
    let mut home = Home::new("pulled-lower");
    let path = home.secured_graph();
    home.umx(&["graph", "unload", "demo"]);
    edit_graph_json(&path, |v| v["embedding"]["policy"] = json!("open"));
    let (env, _) = home.umx(&["graph", "load", &path]);
    let found = issue(&env, Code::PolicyDowngradeUnconfirmed);
    assert_eq!(found.next(), "umx graph confirm-policy demo");
    assert_eq!(env.data["graph"]["effective_policy"], "secured");
    // Another write that does not lower it (A1) leaves it as it is.
    let (env, _) = home.umx(&["graph", "set", "demo", "--title", "Renamed"]);
    assert!(
        has(&env, Code::PolicyDowngradeUnconfirmed),
        "{}",
        env.to_json()
    );
    assert!(!has(&env, Code::PolicyLowered), "{}", env.to_json());
    assert_eq!(listed(&home, "demo")["effective_policy"], "secured");
    home.no_tty = true;
    let (env, exit) = home.umx(&["graph", "confirm-policy", "demo"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    assert_eq!(
        (env.data["was"].clone(), env.data["policy"].clone()),
        (json!("secured"), json!("open"))
    );
    let found = issue(&env, Code::PolicyLowered);
    assert!(
        found
            .message()
            .contains("visibility named to open and embedder rule local to any")
            && found
                .message()
                .contains("New vectors now go in the graph folder itself"),
        "{}",
        found.message()
    );
    let row = listed(&home, "demo");
    assert_eq!(row["effective_policy"], "open");
    let record = fs::read_to_string(home.path.join("state/graph-policies.json")).unwrap();
    assert!(
        record.contains("\"from\": \"secured\"") && record.contains("\"to\": \"open\""),
        "{record}"
    );
    // Confirming a policy that does not lower needs nothing and changes nothing.
    let (env, exit) = home.umx(&["graph", "confirm-policy", "demo"]);
    assert_eq!(
        (exit, env.data["changed"].clone()),
        (0, json!(false)),
        "{}",
        env.to_json()
    );
}

#[test]
fn a_removed_state_file_keeps_every_graph_secured() {
    // Security: rm of the record (its folder stays) must not lower a graph.
    let mut home = Home::new("state-removed");
    let path = home.secured_graph();
    let file = home.path.join("state/graph-policies.json");
    assert!(file.exists(), "the secured graph was recorded");
    home.umx(&["graph", "set", "demo", "--policy", "open"]);
    fs::remove_file(&file).unwrap();
    let (env, _) = home.umx(&["graph", "load", &path]);
    assert!(has(&env, Code::LocalStateUnreadable), "{}", env.to_json());
    assert_eq!(env.data["graph"]["effective_policy"], "secured");
    assert!(!file.exists(), "never recreated");
}

#[test]
fn a_secured_policy_that_could_not_be_recorded_is_never_lowered() {
    // Review R1: the record of a new secured graph fails; lowering it is refused until the record lands.
    let mut home = Home::new("unsaved-lower");
    home.state_file(b"{\"umx_state\": 1, \"graphs\": []}");
    let state = home.path.join("state");
    fs::set_permissions(&state, fs::Permissions::from_mode(0o500)).unwrap();
    home.start();
    let path = home.path.join("g").display().to_string();
    let (env, _) = home.umx(&["graph", "new", &path, "--alias", "demo", "--secured"]);
    assert!(has(&env, Code::LocalStateNotSaved), "{}", env.to_json());
    let (env, exit) = home.umx(&["graph", "set", "demo", "--policy", "open"]);
    assert_eq!(exit, 1, "{}", env.to_json());
    assert!(has(&env, Code::PolicyNotRecorded), "{}", env.to_json());
    let graph_json = fs::read_to_string(home.path.join("g/graph.json")).unwrap();
    assert!(
        graph_json.contains("\"secured\""),
        "graph.json unchanged: {graph_json}"
    );
    assert_eq!(listed(&home, "demo")["effective_policy"], "secured");
    let (env, _) = home.umx(&["graph", "load", &path]);
    assert_eq!(env.data["graph"]["effective_policy"], "secured");
    // Writable again: the held secured policy is recorded first, then this call lowers it, recorded and warned.
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    let (env, exit) = home.umx(&["graph", "set", "demo", "--policy", "open"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    assert!(has(&env, Code::PolicyLowered), "{}", env.to_json());
    let record = fs::read_to_string(state.join("graph-policies.json")).unwrap();
    assert!(
        record.contains("\"from\": \"secured\"") && record.contains("\"to\": \"open\""),
        "{record}"
    );
    assert_eq!(listed(&home, "demo")["effective_policy"], "open");
}

#[test]
fn doctor_checks_the_configured_embedder_without_sending_text() {
    let mut home = Home::new("doc-az");
    home.start();
    home.az();
    home.ok(&["config", "set", "embedder.active", "az"]);
    home.restart();
    // Logged in: ready, and the verdict stays ok (the endpoint is never called; it does not exist).
    let path = stub_az(&home, true);
    let (env, exit) = home.umx_with(&["doctor"], &[("PATH", &path)]);
    assert_eq!(
        (exit, env.data["verdict"].as_str()),
        (0, Some("ok")),
        "{}",
        env.to_json()
    );
    let row = env.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["check"] == "embedder")
        .unwrap()
        .clone();
    assert_eq!(
        (row["outcome"].as_str(), row["subject"].as_str()),
        (Some("ok"), Some("text-embedding-3-small@256"))
    );
    // Logged out: its own finding, degraded, with what to run.
    let path = stub_az(&home, false);
    let (env, exit) = home.umx_with(&["doctor"], &[("PATH", &path)]);
    assert_eq!(
        (exit, env.data["verdict"].as_str()),
        (5, Some("degraded")),
        "{}",
        env.to_json()
    );
    let found = issue(&env, Code::DoctorEmbedderUnready);
    assert!(found.message().contains("az login"), "{}", found.message());
    assert!(
        !found.message().contains("Please run"),
        "az's stderr never reaches a message"
    );
}

/// Review B2: a live daemon that started on a bad config is broken until it restarts, even after the file is fixed;
/// doctor says restart, not start a second daemon. After the restart it is ok.
#[test]
fn doctor_sees_a_daemon_still_closed_after_its_config_was_fixed() {
    let mut home = Home::new("doc-cfg-live");
    home.owner_only();
    let file = home.write_config("roots = [\n");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    home.start();
    fs::remove_file(&file).unwrap();
    let (env, exit) = home.umx(&["doctor"]);
    assert_eq!(
        (exit, env.data["verdict"].as_str()),
        (6, Some("broken")),
        "{}",
        env.to_json()
    );
    assert_eq!(
        issue(&env, Code::DoctorDaemonConfigClosed).next(),
        "umx daemon restart"
    );
    issue(&env, Code::DoctorGraphsUnlisted);
    let codes: Vec<&str> = env.warnings.iter().map(Issue::code).collect();
    assert!(!codes.contains(&"doctor_daemon_down"), "{codes:?}");
    // The restart the finding asks for clears it: the daemon rereads the fixed config in place.
    home.ok(&["daemon", "restart"]);
    home.reloaded();
    let (env, exit) = home.umx(&["doctor"]);
    assert_eq!(
        (exit, env.data["verdict"].as_str()),
        (0, Some("ok")),
        "{}",
        env.to_json()
    );
}

/// Review B3: after --fix makes the config usable again, the re-check uses the repaired file, so a healthy loaded
/// graph is not reported outside the roots.
#[test]
fn doctor_fix_checks_again_with_the_config_it_repaired() {
    let mut home = Home::new("doc-fix-cfg");
    home.graph();
    let file = home.write_config("# mine\n");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o666)).unwrap();
    let out = home.command(&["doctor", "--fix"]).bounded_output().unwrap();
    let env = Envelope::from_json(&out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", env.to_json());
    assert_eq!(env.data["verdict"], "ok", "{}", env.to_json());
    assert_eq!(
        env.data["fixed"][0]["fix"],
        "tighten_mode",
        "{}",
        env.to_json()
    );
    assert_eq!(mode(&file), 0o644);
    assert_eq!(fs::read_to_string(&file).unwrap(), "# mine\n");
}

/// While the config file is unusable there are no roots to judge a loaded graph by: doctor says the config is
/// closed and does not call a healthy graph outside the roots or suggest unloading it.
#[test]
fn a_closed_config_does_not_make_a_healthy_graph_look_outside_the_roots() {
    let mut home = Home::new("doc-closed-roots");
    home.graph();
    let file = home.write_config("# mine\n");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o666)).unwrap();
    let (env, exit) = home.umx(&["doctor"]);
    assert_eq!(exit, 6, "{}", env.to_json());
    issue(&env, Code::DoctorConfigClosed);
    let codes: Vec<&str> = env.warnings.iter().map(Issue::code).collect();
    assert!(!codes.contains(&"doctor_graph_folder"), "{codes:?}");
}

/// Security gate (e): a graph that goes open to secured keeps the tracked shards it made while open. umx says so,
/// counting and naming them (in candidates), on the change and on every later load, until a person moves them.
#[test]
fn going_secured_names_the_tracked_shards_left_from_the_open_period() {
    let mut home = Home::new("left-shards");
    home.graph();
    let quiet = home.ok(&["graph", "set", "demo", "--policy", "secured"]);
    assert!(
        quiet.warnings.is_empty(),
        "no shards, no warning: {}",
        quiet.to_json()
    );
    let path = home.path.join("h").display().to_string();
    home.ok(&["graph", "new", &path, "--alias", "other"]);
    open_period_shards(&path, &["ab", "zz"]);
    let (env, exit) = home.umx(&["graph", "set", "other", "--policy", "secured"]);
    assert_eq!(exit, 0);
    let found = issue(&env, Code::SecuredShardsRemain);
    assert!(
        found.message().contains(" 2 vector shard files"),
        "{}",
        found.message()
    );
    let names: Vec<&str> = found
        .candidates()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "embeddings/text-embedding-3-small@256/ab.tsv",
            "embeddings/text-embedding-3-small@256/zz.tsv"
        ]
    );
    home.ok(&["graph", "unload", "other"]);
    let (env, _) = home.umx(&["graph", "load", &path]);
    issue(&env, Code::SecuredShardsRemain);
}

/// A repository's config may narrow the embedder for graphs in it (req-72): with `embedder.active = "none"` in the
/// repo, the embed worker never sends that graph's nodes to the machine's hosted instance; `az` is never even asked
/// for a token. The same node in a graph outside the repository does reach it (the control).
#[test]
fn a_repo_that_narrows_to_none_keeps_its_graphs_away_from_the_hosted_embedder() {
    let mut home = Home::new("narrow");
    let bin = home.az_active("");
    let log = bin.join("ran");
    fs::write(
        bin.join("az"),
        format!("#!/bin/sh\n/usr/bin/touch '{}'\nexit 1\n", log.display()),
    )
    .unwrap();
    fs::set_permissions(bin.join("az"), fs::Permissions::from_mode(0o755)).unwrap();
    let repo = home.repo();
    fs::create_dir_all(repo.join(".umx")).unwrap();
    fs::write(
        repo.join(".umx/config.toml"),
        "embedder.active = \"none\"\n",
    )
    .unwrap();
    home.start();
    let inside = repo.join("g").display().to_string();
    home.ok(&["graph", "new", &inside, "--alias", "inside"]);
    home.ok(&[
        "node",
        "add",
        "--graph",
        "inside",
        "--type",
        "problem",
        "--title",
        "client text",
    ]);
    std::thread::sleep(Duration::from_secs(2));
    assert!(
        !log.exists(),
        "az ran for a graph whose repo narrows to none"
    );
    let outside = home.path.join("h").display().to_string();
    home.ok(&["graph", "new", &outside, "--alias", "outside"]);
    home.ok(&[
        "node",
        "add",
        "--graph",
        "outside",
        "--type",
        "problem",
        "--title",
        "open text",
    ]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !log.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        log.exists(),
        "the control: az is asked for a graph outside the repo"
    );
}

#[test]
fn reembed_parses_and_needs_an_embedder_and_graph_list_counts_pending_vectors() {
    // ws004 s06 B: no mode composes an embedder until s05, so the CLI reaches the no-embedder refusal.
    let mut home = Home::new("reembed");
    home.graph();
    let (env, exit) = home.umx(&["embeddings", "reembed", "--graph", "demo", "--dry-run"]);
    assert_eq!(exit, 1, "{}", env.to_json());
    assert!(has(&env, Code::EmbedderNotConfigured), "{}", env.to_json());
    // Without --dry-run the daemon plans under the shared lock first (architect F1); no embedder stops it there.
    let (env, exit) = home.umx(&["embeddings", "reembed", "--graph", "demo"]);
    assert_eq!(exit, 1, "{}", env.to_json());
    assert!(has(&env, Code::EmbedderNotConfigured), "{}", env.to_json());
    let (env, exit) = home.umx(&["embeddings", "reembed", "--model", "Not A Key"]);
    assert_eq!(exit, 2, "{}", env.to_json());
    assert!(has(&env, Code::UsageInvalid), "{}", env.to_json());
    assert_eq!(
        listed(&home, "demo")["vectors_pending"],
        Value::Null,
        "no model, no embedder"
    );
    let path = home.path.join("m").display().to_string();
    let (env, _) = home.umx(&[
        "graph",
        "new",
        &path,
        "--alias",
        "modelled",
        "--embedding-model",
        "m@8",
    ]);
    assert_eq!(env.data["graph"]["vectors_pending"], 0, "{}", env.to_json());
    home.umx(&[
        "node",
        "add",
        "--graph",
        "modelled",
        "--type",
        "problem",
        "--title",
        "Slow builds",
    ]);
    assert_eq!(listed(&home, "modelled")["vectors_pending"], 1);
}

/// R91-1: the graph-scoped probe acts on the effective policy, not graph.json's word. A downgrade nobody confirmed
/// (graph.json says open, this machine's record says secured) and a missing record (fail closed) both keep the
/// graph secured, so the hosted instance is refused, with or without --embedder, and az is never asked.
#[test]
fn the_graph_probe_uses_the_effective_policy_not_graph_json() {
    let probe_refused = |home: &Home, log: &Path| {
        for args in [
            &["embeddings", "probe", "--graph", "demo"][..],
            &["embeddings", "probe", "--graph", "demo", "--embedder", "az"],
        ] {
            let (env, exit) = home.umx(args);
            assert_eq!(exit, 1, "{}", env.to_json());
            let found = issue(&env, Code::EmbedderUnavailable);
            assert!(found.message().contains("secured"), "{}", found.message());
        }
        assert!(
            !log.exists(),
            "az ran for a graph that is secured in effect"
        );
    };
    let marker_az = |bin: &Path| {
        let log = bin.join("ran");
        fs::write(
            bin.join("az"),
            format!("#!/bin/sh\n/usr/bin/touch '{}'\nexit 1\n", log.display()),
        )
        .unwrap();
        fs::set_permissions(bin.join("az"), fs::Permissions::from_mode(0o755)).unwrap();
        log
    };

    // graph.json lowered to open outside umx, with no umx call confirming it.
    let mut home = Home::new("probe-downgrade");
    let log = marker_az(&home.az_active(""));
    let path = home.secured_graph();
    home.umx(&["graph", "unload", "demo"]);
    edit_graph_json(&path, |v| v["embedding"]["policy"] = json!("open"));
    home.umx(&["graph", "load", &path]);
    let listed = home.ok(&["graph", "list"]);
    assert_eq!(listed.data["rows"][0]["effective_policy"], "secured");
    probe_refused(&home, &log);

    // This machine's record missing while its folder exists: every graph counts as secured.
    let mut home = Home::new("probe-no-state");
    let log = marker_az(&home.az_active(""));
    let path = home.graph();
    home.ok(&["graph", "unload", "demo"]);
    let state = home.state_file(b"{\"umx_state\": 1, \"graphs\": []}");
    fs::remove_file(&state).unwrap();
    home.umx(&["graph", "load", &path]);
    probe_refused(&home, &log);

    // The control: an open graph with a readable record is probed, so the marker works.
    let mut home = Home::new("probe-open");
    let log = marker_az(&home.az_active(""));
    home.graph();
    home.umx(&["embeddings", "probe", "--graph", "demo"]);
    assert!(log.exists(), "the control: az is asked for an open graph");
}

/// R91-2: when umx cannot list a secured graph's folder for open-period shards, it says so; it never reports that
/// none remain. Both levels: the embeddings/ folder and one model's folder.
#[test]
fn a_shard_folder_umx_cannot_read_is_reported_never_taken_as_none() {
    for (name, level) in [
        ("unchecked-top", "embeddings"),
        ("unchecked-space", "embeddings/text-embedding-3-small@256"),
    ] {
        let mut home = Home::new(name);
        let path = home.graph();
        open_period_shards(&path, &["ab"]);
        let folder = Path::new(&path).join(level);
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o000)).unwrap();
        let (env, exit) = home.umx(&["graph", "set", "demo", "--policy", "secured"]);
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(exit, 0, "{}", env.to_json());
        let codes: Vec<&str> = env.warnings.iter().map(Issue::code).collect();
        assert!(
            codes.contains(&"secured_shards_unchecked"),
            "{level}: {codes:?}"
        );
        assert!(
            !codes.contains(&"secured_shards_remain"),
            "{level}: {codes:?}"
        );
        let listed = home.ok(&["graph", "list"]);
        assert_eq!(
            listed.data["rows"][0]["effective_policy"], "secured",
            "the raise still happened"
        );
    }
}

/// s17 security, the one folder rule (repo_layer_ok): a repository config file umx refuses or cannot parse
/// allows no instance for its folder: the graphs in it embed with nothing and the probe refuses (az is never asked).
#[test]
fn a_broken_repo_config_keeps_its_graphs_away_from_every_embedder() {
    for (name, broken) in [("repo-unparsable", None), ("repo-refused", Some(0o666))] {
        let mut home = Home::new(name);
        let bin = home.az_active("");
        let log = bin.join("ran");
        fs::write(
            bin.join("az"),
            format!("#!/bin/sh\n/usr/bin/touch '{}'\nexit 1\n", log.display()),
        )
        .unwrap();
        fs::set_permissions(bin.join("az"), fs::Permissions::from_mode(0o755)).unwrap();
        let repo = home.repo();
        fs::create_dir_all(repo.join(".umx")).unwrap();
        let file = repo.join(".umx/config.toml");
        match broken {
            None => fs::write(&file, "embedder.active = [\n").unwrap(),
            Some(mode) => {
                fs::write(&file, "embedder.active = \"none\"\n").unwrap();
                fs::set_permissions(&file, fs::Permissions::from_mode(mode)).unwrap();
            }
        }
        home.start();
        let inside = repo.join("g").display().to_string();
        home.ok(&["graph", "new", &inside, "--alias", "inside"]);
        home.ok(&[
            "node",
            "add",
            "--graph",
            "inside",
            "--type",
            "problem",
            "--title",
            "client text",
        ]);
        std::thread::sleep(Duration::from_secs(2));
        assert!(
            !log.exists(),
            "{name}: az ran for a graph whose repo config is broken"
        );
        let (env, _) = home.umx(&["embeddings", "probe", "--graph", "inside"]);
        let found = issue(&env, Code::EmbedderUnavailable);
        assert!(
            found.message().contains("folder config"),
            "{}",
            found.message()
        );
        assert!(!log.exists(), "{name}: the probe asked az");
    }
}

/// Security: repo narrowing bounds the per-graph choice. A repo that narrows to its local instance keeps its
/// graphs off the hosted instance even when a graph records exactly the hosted instance's model: az is never asked.
#[test]
fn a_repo_narrowed_to_local_never_uses_a_hosted_instance_that_matches_the_graph() {
    let mut home = Home::new("narrow-local");
    let bin = home.az_active(
        "embedders.gemma.kind = \"local\"\n\
         embedders.gemma.model = \"embeddinggemma-2\"\n\
         embedders.gemma.dims = 256\n",
    );
    let log = bin.join("ran");
    fs::write(
        bin.join("az"),
        format!("#!/bin/sh\n/usr/bin/touch '{}'\nexit 1\n", log.display()),
    )
    .unwrap();
    fs::set_permissions(bin.join("az"), fs::Permissions::from_mode(0o755)).unwrap();
    home.env
        .push(("HOME".into(), home.path.display().to_string()));
    let repo = home.repo();
    fs::create_dir_all(repo.join(".umx")).unwrap();
    fs::write(
        repo.join(".umx/config.toml"),
        "embedder.active = \"gemma\"\n",
    )
    .unwrap();
    home.start();
    let inside = repo.join("g").display().to_string();
    home.ok(&[
        "graph",
        "new",
        &inside,
        "--alias",
        "inside",
        "--embedding-model",
        "text-embedding-3-small@256",
    ]);
    home.ok(&[
        "node",
        "add",
        "--graph",
        "inside",
        "--type",
        "problem",
        "--title",
        "client text",
    ]);
    std::thread::sleep(Duration::from_secs(2));
    assert!(
        !log.exists(),
        "az ran for a graph in a repo narrowed to a local instance"
    );
    let (env, _) = home.umx(&["embeddings", "probe", "--graph", "inside"]);
    assert!(
        env.errors
            .iter()
            .all(|e| !e.message().contains("Embedder az")),
        "{}",
        env.to_json()
    );
    assert!(!log.exists(), "the probe asked az");
}

/// Security B1: the folder rule is decided on the repo layer itself. A repo that sets embedder.active to the
/// SAME value as the machine (a local instance, or none) still allows only what it names, so a graph recording the
/// hosted instance's model is never sent to it: az never runs, from the worker or the probe. F1: a repo that
/// names an instance the machine does not define (not a valid narrowing, so resolve drops it) allows none, rather
/// than falling back to the machine's az.
#[test]
fn a_repo_naming_the_same_value_as_the_machine_still_bounds_the_choice() {
    let gemma = "embedders.gemma.kind = \"local\"\n\
                 embedders.gemma.model = \"embeddinggemma-2\"\n\
                 embedders.gemma.dims = 256\n";
    for (name, defined, machine, in_repo) in [
        (
            "same-local",
            gemma,
            "embedder.active = \"gemma\"\n",
            "embedder.active = \"gemma\"\n",
        ),
        ("same-none", gemma, "", "embedder.active = \"none\"\n"),
        (
            "undefined-local",
            "",
            "embedder.active = \"az\"\n",
            "embedder.active = \"gemma\"\n",
        ),
    ] {
        let mut home = Home::new(name);
        let bin = home.az_active(defined);
        // az_active made az the machine's choice; put this case's machine value in its place.
        let config = home.path.join("config.toml");
        let text = fs::read_to_string(&config)
            .unwrap()
            .replace("embedder.active = \"az\"\n", machine);
        fs::write(&config, text).unwrap();
        let log = bin.join("ran");
        fs::write(
            bin.join("az"),
            format!("#!/bin/sh\n/usr/bin/touch '{}'\nexit 1\n", log.display()),
        )
        .unwrap();
        fs::set_permissions(bin.join("az"), fs::Permissions::from_mode(0o755)).unwrap();
        home.env
            .push(("HOME".into(), home.path.display().to_string()));
        let repo = home.repo();
        fs::create_dir_all(repo.join(".umx")).unwrap();
        fs::write(repo.join(".umx/config.toml"), in_repo).unwrap();
        home.start();
        let inside = repo.join("g").display().to_string();
        home.ok(&[
            "graph",
            "new",
            &inside,
            "--alias",
            "inside",
            "--embedding-model",
            "text-embedding-3-small@256",
        ]);
        home.ok(&[
            "node",
            "add",
            "--graph",
            "inside",
            "--type",
            "problem",
            "--title",
            "client text",
        ]);
        std::thread::sleep(Duration::from_secs(2));
        assert!(!log.exists(), "{name}: the worker asked az");
        home.umx(&["embeddings", "probe", "--graph", "inside"]);
        assert!(!log.exists(), "{name}: the probe asked az");
    }
}

/// N1: `none` is reserved, never an instance name, so a repo's `embedder.active = "none"` always allows nothing.
/// A machine that defines a hosted instance named none has those keys refused (`config_key_unknown`), and a graph in
/// a repo set to none, recording that instance's model, is never sent to it: az never runs, from the worker or the
/// probe.
#[test]
fn an_instance_named_none_is_refused_so_a_repo_none_allows_nothing() {
    let mut home = Home::new("named-none");
    home.write_config(
        "embedder.active = \"none\"\n\
         embedders.none.kind = \"azure_openai\"\n\
         embedders.none.endpoint = \"https://127.0.0.1:9\"\n\
         embedders.none.deployment = \"emb\"\n\
         embedders.none.model = \"text-embedding-3-small\"\n\
         embedders.none.dims = 256\n",
    );
    let bin = home.path.join("bin");
    fs::create_dir_all(&bin).unwrap();
    home.env.push(("PATH".into(), bin.display().to_string()));
    let log = bin.join("ran");
    fs::write(
        bin.join("az"),
        format!("#!/bin/sh\n/usr/bin/touch '{}'\nexit 1\n", log.display()),
    )
    .unwrap();
    fs::set_permissions(bin.join("az"), fs::Permissions::from_mode(0o755)).unwrap();
    home.env
        .push(("HOME".into(), home.path.display().to_string()));
    let repo = home.repo();
    fs::create_dir_all(repo.join(".umx")).unwrap();
    fs::write(
        repo.join(".umx/config.toml"),
        "embedder.active = \"none\"\n",
    )
    .unwrap();
    home.start();
    let (checked, _) = home.umx(&["config", "check"]);
    assert!(
        checked.errors.iter().chain(&checked.warnings).any(
            |p| p.code() == "config_key_unknown" && p.message().contains("embedders.none.kind")
        ),
        "{}",
        checked.to_json()
    );
    let inside = repo.join("g").display().to_string();
    home.ok(&[
        "graph",
        "new",
        &inside,
        "--alias",
        "inside",
        "--embedding-model",
        "text-embedding-3-small@256",
    ]);
    home.ok(&[
        "node",
        "add",
        "--graph",
        "inside",
        "--type",
        "problem",
        "--title",
        "client text",
    ]);
    std::thread::sleep(Duration::from_secs(2));
    assert!(!log.exists(), "the worker asked az");
    home.umx(&["embeddings", "probe", "--graph", "inside"]);
    assert!(!log.exists(), "the probe asked az");
}

/// R106-3, end to end: two local instances of one model, the machine's default first. A repo narrowed to the
/// second uses the second for its graph (the probe names it), never nothing because the default was forbidden.
#[test]
fn a_repo_narrowed_to_one_of_two_matching_instances_uses_it() {
    let mut home = Home::new("narrow-tie");
    home.write_config(
        "embedder.active = \"gemma1\"\n\
         embedders.gemma1.kind = \"local\"\nembedders.gemma1.model = \"embeddinggemma-2\"\nembedders.gemma1.dims = 256\n\
         embedders.gemma2.kind = \"local\"\nembedders.gemma2.model = \"embeddinggemma-2\"\nembedders.gemma2.dims = 256\n",
    );
    home.env
        .push(("HOME".into(), home.path.display().to_string()));
    let repo = home.repo();
    fs::create_dir_all(repo.join(".umx")).unwrap();
    fs::write(
        repo.join(".umx/config.toml"),
        "embedder.active = \"gemma2\"\n",
    )
    .unwrap();
    home.start();
    let inside = repo.join("g").display().to_string();
    home.ok(&[
        "graph",
        "new",
        &inside,
        "--alias",
        "inside",
        "--embedding-model",
        "embeddinggemma-2@256+p1",
    ]);
    // The model file is absent (HOME is the test's), so the probe stops at ready(), naming the instance it chose.
    let (env, _) = home.umx(&["embeddings", "probe", "--graph", "inside"]);
    let found = issue(&env, Code::EmbedderUnavailable);
    assert!(
        found.message().starts_with("Embedder gemma2 "),
        "{}",
        found.message()
    );
}

/// req-49 (ws004 s08): `umx embeddings allow` records the allow in its own owner-only folder, never beside the policy
/// record: a state folder without its record is refused, which would count every graph as secured.
#[test]
fn an_allow_is_kept_in_its_own_folder_and_never_secures_the_graphs() {
    let mut home = Home::new("allow");
    home.graph();
    let (env, _) = home.umx(&[
        "node",
        "add",
        "--type",
        "problem",
        "--title",
        "Export fails",
        "--note",
        &fake_aws_key(),
    ]);
    assert!(has(&env, Code::SecretSuspected), "{}", env.to_json());
    let id = env.data["node"]["id"].as_str().unwrap().to_owned();
    let (env, exit) = home.umx(&["embeddings", "allow", "--node", &id, "--field", "note"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    assert_eq!(env.data["allowed"], true);
    assert_eq!(env.data["rule"], "aws_access_key");
    let file = home.path.join("allows/secret-allows.json");
    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(!fs::read_to_string(&file).unwrap().contains("AKIA"));
    assert!(
        !home.path.join("state").exists(),
        "the policy record's folder is untouched"
    );
    assert_eq!(listed(&home, "demo")["effective_policy"], "open");
    // A field with no secret: nothing to allow.
    let (env, exit) = home.umx(&["embeddings", "allow", "--node", &id, "--field", "title"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    assert_eq!(env.data["allowed"], false);
}

/// ws004 s26: a machine-config similar_floor replaces the model's floor and is reported as such (floor_source config,
/// floor_calibrated false); a floor outside -1 to 1 is refused. No model runs: the local instance is only built.
#[test]
fn a_machine_config_floor_is_used_and_reported_as_config() {
    let mut home = Home::new("floor-config");
    home.write_config(
        "embedders.loc.kind = \"local\"\nembedders.loc.model = \"embeddinggemma-2\"\nembedders.loc.dims = 256\n\
         embedders.loc.similar_floor = 0.42\n",
    );
    home.start();
    let path = home.path.join("g").display().to_string();
    home.ok(&[
        "graph",
        "new",
        &path,
        "--alias",
        "demo",
        "--embedding-model",
        "embeddinggemma-2@256+p1",
    ]);
    let env = home.ok(&["similarity", "rebuild", "--graph", "demo"]);
    assert_eq!(
        (
            &env.data["floor"],
            &env.data["floor_source"],
            &env.data["floor_calibrated"],
            &env.data["floor_run"]
        ),
        (&json!(0.42), &json!("config"), &json!(false), &json!(null)),
        "{}",
        env.to_json()
    );
    let (env, exit) = home.umx(&["config", "set", "embedders.loc.search_floor", "1.5"]);
    assert_eq!(exit, 1, "{}", env.to_json());
    issue(&env, Code::ConfigValueInvalid);
}

/// ws004 s27: an instance added to the config after the daemon started is probed without a restart: the probe builds
/// it from the config as it is now (here it reaches for its closed endpoint), instead of refusing it as "not defined
/// when the daemon started". The running daemon's default is unchanged.
#[test]
fn probe_builds_an_instance_added_after_the_daemon_started() {
    let mut home = Home::new("probe-added");
    let bin = home.az_active("");
    fake_az(&bin, "tok-DO-NOT-LEAK");
    home.start();
    for (k, v) in [
        ("kind", "azure_openai"),
        ("endpoint", "https://127.0.0.1:9"),
        ("deployment", "later-emb"),
        ("model", "text-embedding-3-large"),
        ("dims", "256"),
    ] {
        home.ok(&["config", "set", &format!("embedders.later.{k}"), v]);
    }
    let (env, exit) = home.umx(&["embeddings", "probe", "--embedder", "later"]);
    assert_eq!(exit, 1);
    assert!(
        env.errors.iter().all(|e| e.code() != "embedder_invalid"),
        "the added instance was refused, not built: {}",
        env.to_json()
    );
    let found = issue(&env, Code::EmbedderUnavailable);
    assert!(
        found
            .message()
            .contains("could not reach https://127.0.0.1:9"),
        "{}",
        found.message()
    );
    assert!(!printed(&home, &env).contains("DO-NOT-LEAK"));
    let status = home.ok(&["daemon", "status"]);
    assert_eq!(status.data["embedder"]["active"], "az");
}

/// R202-1: the route warning counts with the real selection and folder rules: a named graph that records no model
/// (it uses the default) counts; a named graph of the same model in a repo narrowed to none never routes to the
/// instance and does not. Counted, never named.
#[test]
fn the_route_warning_counts_keyless_graphs_and_not_graphs_their_folder_keeps_away() {
    let mut home = Home::new("route-count");
    home.az_active("");
    let repo = home.repo();
    fs::create_dir_all(repo.join(".umx")).unwrap();
    fs::write(
        repo.join(".umx/config.toml"),
        "embedder.active = \"none\"\n",
    )
    .unwrap();
    home.start();
    let keyless = home.path.join("k").display().to_string();
    home.ok(&["graph", "new", &keyless, "--alias", "keyless"]);
    home.ok(&["graph", "set", "keyless", "--visibility", "named"]);
    let narrowed = repo.join("n").display().to_string();
    home.ok(&[
        "graph",
        "new",
        &narrowed,
        "--alias",
        "narrowed",
        "--embedding-model",
        "text-embedding-3-small@256",
    ]);
    home.ok(&["graph", "set", "narrowed", "--visibility", "named"]);
    let env = home.ok(&[
        "config",
        "set",
        "embedders.az.endpoint",
        "https://127.0.0.2:9",
    ]);
    let found = issue(&env, Code::EmbedderRouteChanged);
    assert!(
        found.message().contains(" 1 loaded graphs"),
        "{}",
        found.message()
    );
    let text = env.to_json();
    assert!(
        !text.contains("keyless") && !text.contains("narrowed"),
        "{text}"
    );
}

/// R202-2: unsetting one key of the instance a named graph routes to (kind, model or dims) leaves it unbuildable, so
/// after a restart the graph falls back to another hosted instance of its model: that warns too.
#[test]
fn a_partial_unset_that_reroutes_a_named_graph_warns() {
    for leaf in ["kind", "model", "dims"] {
        let mut home = Home::new(&format!("route-unset-{leaf}"));
        home.az_active(
            "embedders.az2.kind = \"azure_openai\"\n\
             embedders.az2.endpoint = \"https://127.0.0.3:9\"\n\
             embedders.az2.deployment = \"emb\"\n\
             embedders.az2.model = \"text-embedding-3-small\"\n\
             embedders.az2.dims = 256\n",
        );
        home.start();
        let path = home.path.join("g").display().to_string();
        home.ok(&[
            "graph",
            "new",
            &path,
            "--alias",
            "acme",
            "--embedding-model",
            "text-embedding-3-small@256",
        ]);
        home.ok(&["graph", "set", "acme", "--visibility", "named"]);
        let env = home.ok(&["config", "unset", &format!("embedders.az.{leaf}")]);
        let found = issue(&env, Code::EmbedderRouteChanged);
        assert!(
            found.message().contains(" 1 loaded graphs"),
            "{leaf}: {}",
            found.message()
        );
        assert!(
            !env.to_json().contains("acme"),
            "{leaf}: counted, never named"
        );
    }
}

/// D62, security s20b (N2): config import that adds a hosted instance, or replaces one with --force at another
/// endpoint, warns embedder_route_changed when loaded graphs that are not open use its model; a dry run writes
/// nothing and does not warn.
#[test]
fn config_import_that_adds_or_repoints_a_hosted_instance_warns_the_route_changed() {
    let mut home = Home::new("fs3-route");
    let dir = fs3_folder(&home, FS3_CONFIG);
    home.env
        .push(("FS3_CONFIG_DIR".into(), dir.display().to_string()));
    home.start();
    let path = home.path.join("g").display().to_string();
    home.ok(&[
        "graph",
        "new",
        &path,
        "--alias",
        "acme",
        "--embedding-model",
        "text-embedding-3-small@1024",
    ]);
    home.ok(&["graph", "set", "acme", "--visibility", "named"]);
    let route = |env: &Envelope| {
        env.warnings
            .iter()
            .filter(|w| w.code() == "embedder_route_changed")
            .count()
    };
    let dry = home.ok(&["config", "import", "--from", "flowspace3", "--dry-run"]);
    assert_eq!(route(&dry), 0, "{}", dry.to_json());
    let added = home.ok(&["config", "import", "--from", "flowspace3"]);
    assert_eq!(route(&added), 1, "{}", added.to_json());
    assert!(!added.to_json().contains("acme"), "counted, never named");
    fs::write(
        dir.join("config.toml"),
        FS3_CONFIG.replace("example-resource", "elsewhere"),
    )
    .unwrap();
    let forced = home.ok(&["config", "import", "--from", "flowspace3", "--force"]);
    assert_eq!(route(&forced), 1, "{}", forced.to_json());
    // The same again changes nothing, so no warning.
    let same = home.ok(&["config", "import", "--from", "flowspace3", "--force"]);
    assert_eq!(route(&same), 0, "{}", same.to_json());
}

/// ws004 s27: `umx config import --from flowspace3` copies exactly the embedder keys into the machine layer (auth
/// cli), with a dry run that writes nothing; it never opens flowspace3's secrets.env or shows its key, its key's
/// name or value; it refuses to replace an instance unless --force, and it leaves embedder.active alone.
#[test]
fn config_import_copies_only_the_embedder_and_never_a_secret() {
    let mut home = Home::new("fs3-import");
    let dir = fs3_folder(&home, FS3_CONFIG);
    home.env
        .push(("FS3_CONFIG_DIR".into(), dir.display().to_string()));
    home.start();
    let machine = home.path.join("config.toml");
    let mut seen = String::new();

    let dry = home.ok(&["config", "import", "--from", "flowspace3", "--dry-run"]);
    seen += &dry.to_json();
    assert_eq!(dry.data["written"], false);
    assert!(
        !machine.exists()
            || !fs::read_to_string(&machine)
                .unwrap()
                .contains("azure_embed")
    );

    let done = home.ok(&["config", "import", "--from", "flowspace3"]);
    seen += &done.to_json();
    assert_eq!(done.data["embedder"], "azure_embed");
    assert_eq!(done.data["set"], dry.data["set"]);
    let keys: Vec<&str> = done.data["set"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["key"].as_str().unwrap())
        .collect();
    assert_eq!(
        keys,
        [
            "embedders.azure_embed.kind",
            "embedders.azure_embed.endpoint",
            "embedders.azure_embed.deployment",
            "embedders.azure_embed.model",
            "embedders.azure_embed.dims",
            "embedders.azure_embed.api_version",
            "embedders.azure_embed.auth",
        ]
    );
    let file = fs::read_to_string(&machine).unwrap();
    seen += &file;
    assert!(
        file.contains("embedders.azure_embed.auth = \"cli\""),
        "{file}"
    );
    assert!(file.contains("embedders.azure_embed.model = \"text-embedding-3-small\""));
    assert!(!file.contains("api_key_env"), "{file}");
    assert!(!file.contains("embedder.active"), "{file}");

    let (again, exit) = home.umx(&["config", "import", "--from", "flowspace3"]);
    assert_eq!(exit, 1);
    issue(&again, Code::ConfigImportExists);
    seen += &again.to_json();
    let forced = home.ok(&[
        "config",
        "import",
        "--from",
        "flowspace3",
        "--force",
        "--as",
        "fs3",
    ]);
    assert_eq!(forced.data["embedder"], "fs3");
    seen += &forced.to_json();
    seen += &home.ok(&["config", "show"]).to_json();

    assert!(
        !seen.contains("DO-NOT-LEAK"),
        "the fake key reached output or the config file"
    );
    assert!(
        !seen.contains("FS3_FAKE_KEY"),
        "the key's env var name reached output or the config file"
    );
    assert_eq!(mode(&dir.join("secrets.env")), 0o000);

    // The model cannot be told from a custom deployment name: refused with --model as the fix.
    let custom = Home::new("fs3-custom");
    let dir = fs3_folder(
        &custom,
        &FS3_CONFIG.replace("text-embedding-3-small-01", "mine"),
    );
    let mut custom = custom;
    custom
        .env
        .push(("FS3_CONFIG_DIR".into(), dir.display().to_string()));
    custom.start();
    let (env, _) = custom.umx(&["config", "import", "--from", "flowspace3"]);
    let found = issue(&env, Code::ConfigImportRefused);
    assert!(found.next().contains("--model"), "{}", found.next());
    custom.ok(&[
        "config",
        "import",
        "--from",
        "flowspace3",
        "--model",
        "text-embedding-3-large",
    ]);
}

/// R177-2: every command `config import` suggests repeats the target and the options it was given, so following it
/// changes only the instance that was asked for; a dry run's repair stays a dry run.
#[test]
fn config_import_repairs_keep_the_target_and_options() {
    let mut home = Home::new("fs3-repairs");
    let dir = fs3_folder(
        &home,
        &FS3_CONFIG.replace("text-embedding-3-small-01", "custom-deployment"),
    );
    home.env
        .push(("FS3_CONFIG_DIR".into(), dir.display().to_string()));
    let mut existing = String::new();
    for (name, deployment) in [("azure", "old-azure"), ("azure_embed", "old-embed")] {
        existing += &format!(
            "embedders.{name}.kind = \"azure_openai\"\n\
             embedders.{name}.endpoint = \"https://example.openai.azure.com\"\n\
             embedders.{name}.deployment = \"{deployment}\"\n\
             embedders.{name}.model = \"text-embedding-3-small\"\n\
             embedders.{name}.dims = 1536\n"
        );
    }
    let machine = home.write_config(&existing);
    home.start();
    let before = fs::read_to_string(&machine).unwrap();
    let deployment = |name: &str| {
        let show = home.ok(&["config", "show"]);
        setting(&show, &format!("embedders.{name}.deployment"))["value"].clone()
    };

    // No model can be told from "custom-deployment": the fix keeps --as and the dry run.
    let (env, _) = home.umx(&[
        "config",
        "import",
        "--from",
        "flowspace3",
        "--as",
        "azure",
        "--dry-run",
    ]);
    assert_eq!(
        issue(&env, Code::ConfigImportRefused).next(),
        "umx config import --from flowspace3 --as azure --model <model> --dry-run"
    );

    // A dry run that collides with azure: the fix names azure, keeps --model, --dims and --dry-run.
    let ask = [
        "config",
        "import",
        "--from",
        "flowspace3",
        "--as",
        "azure",
        "--model",
        "text-embedding-3-large",
        "--dims",
        "256",
        "--dry-run",
    ];
    let (env, exit) = home.umx(&ask);
    assert_eq!(exit, 1);
    let next = issue(&env, Code::ConfigImportExists).next().to_owned();
    assert_eq!(
        next,
        "umx config import --from flowspace3 --as azure --model text-embedding-3-large --dims 256 --force --dry-run"
    );
    let (preview, exit) = follow(&home, &next);
    assert_eq!(exit, 0, "{}", preview.to_json());
    assert_eq!(
        fs::read_to_string(&machine).unwrap(),
        before,
        "a dry run wrote"
    );
    assert_eq!(preview.data["key"], "text-embedding-3-large@256");

    // The preview's write command replaces azure only.
    let write = preview.data["next"][0].as_str().unwrap();
    let write = write.strip_suffix(" (writes these keys)").unwrap();
    assert_eq!(
        write,
        "umx config import --from flowspace3 --as azure --model text-embedding-3-large --dims 256 --force"
    );
    let (done, exit) = follow(&home, write);
    assert_eq!(exit, 0, "{}", done.to_json());
    assert_eq!(done.data["replaced"], true);
    assert_eq!(deployment("azure"), json!("custom-deployment"));
    assert_eq!(
        deployment("azure_embed"),
        json!("old-embed"),
        "an instance that was not asked for changed"
    );
    let file = fs::read_to_string(&machine).unwrap();
    assert!(file.contains("embedders.azure.dims = 256"), "{file}");
    assert!(
        file.contains("embedders.azure.model = \"text-embedding-3-large\""),
        "{file}"
    );

    // F4: no loaded graph records the key. text-embedding-3-large@256 is calibrated (ws004 s26), so no floor warning;
    // the trigger test covers an uncalibrated key (text-embedding-3-small@1024).
    issue(&done, Code::ImportKeyUnmatched);
    assert!(
        !done
            .warnings
            .iter()
            .any(|w| w.code() == "similarity_floor_uncalibrated"),
        "{}",
        done.to_json()
    );
}

//! The umx dispatch (blueprint 001, c-bin). `umx daemon start` runs the daemon in the foreground and `umx doctor`
//! checks the setup with the daemon up or down; every other command runs the CLI client.

use std::io::IsTerminal;
use std::process::ExitCode;

use umx_protocol::{Build, Envelope, Value};

mod ui;

/// Stub in a `stub` build (fakes, req-51), else real.
fn mode() -> umx_compose::Mode {
    #[cfg(feature = "stub")]
    return umx_compose::Mode::Stub;
    #[cfg(not(feature = "stub"))]
    umx_compose::Mode::Real
}

static UI_OPENER: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// The browser opener `umx ui` uses in place of the system's: set only by the test driver (never in a release
/// build), so a drive never opens the person's browser.
fn ui_opener() -> Option<std::path::PathBuf> {
    UI_OPENER.get().cloned()
}

/// Test driver only (ws009 M1b2 evidence): `umx ui` hands its links to `program` (on stdin, as osascript gets them)
/// and a detached view stops after `idle_ms` with no tab, in place of 30 min.
#[cfg(feature = "test-driver")]
pub fn ui_test_knobs(program: Option<std::path::PathBuf>, idle_ms: Option<u64>) {
    if let Some(p) = program {
        let _ = UI_OPENER.set(p);
    }
    if let Some(ms) = idle_ms {
        ui::set_idle(ms);
    }
}

fn actor_claim(actor: umx_core::provenance::Actor) -> umx_protocol::ActorClaim {
    use umx_core::provenance::{ActorInput, Tier, is_session_hash};
    let input: ActorInput = actor.into();
    let session = match input.harness_session {
        Some(value) if is_session_hash(&value) => Some(value),
        None => None,
        Some(_) => {
            eprintln!("umx: identity session was not hashed (ignored)");
            None
        }
    };
    umx_protocol::ActorClaim {
        tier: match input.tier {
            Tier::Pij => "pij",
            Tier::Harness => "harness",
            Tier::Cli => "cli",
            Tier::Unknown => "unknown",
        }
        .into(),
        harness: input.harness,
        session,
        seat: input.seat,
        role: input.role,
        model: input.model,
        effort: input.effort,
        user: input.user,
    }
}

pub fn run(
    args: impl IntoIterator<Item = String>,
    budgets: umx_compose::ClientBudgets,
) -> ExitCode {
    // Resolved once, now: on Linux a binary replaced later reads "<path> (deleted)", and `daemon restart` must run
    // the file now at this path (F46).
    let exe = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .unwrap_or_else(|_| std::path::PathBuf::from("umx"));
    let mut config = umx_daemon::Config::from_env();
    config.client_budgets = budgets;
    // req-63 (D22a): a terminal gets text and anything else JSON, unless UMX_OUTPUT pins it; flags win.
    let reader = config.json.unwrap_or(!std::io::stdout().is_terminal());
    let (json, args) = umx_cli::global_flags(args.into_iter().collect(), reader);
    let output = umx_cli::Output {
        json,
        color: config.color,
    };
    let build = Build {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        commit: env!("UMX_COMMIT").to_owned(),
        // `just dev` stamps each build; a plain cargo build says "dev".
        built: option_env!("UMX_BUILD_ID").unwrap_or("dev").to_owned(),
    };
    if let Some(help) = umx_cli::help(&args) {
        return ExitCode::from(umx_cli::emit(&help, output));
    }
    let code = match args.as_slice() {
        [daemon, start, rest @ ..] if daemon == "daemon" && start == "start" => {
            match umx_cli::daemon_start_flags(rest) {
                Ok(flags) => {
                    if let Some(ms) = flags.idle_unload_ms {
                        config = config.idle_unload_flag(ms);
                    }
                    let relaunch = umx_daemon::Relaunch {
                        exe,
                        args: ["daemon", "start"]
                            .into_iter()
                            .map(str::to_owned)
                            .chain(rest.iter().cloned())
                            .collect(),
                    };
                    let failed = umx_daemon::start(&config, build, relaunch, |ready| {
                        umx_cli::emit(ready, output);
                    });
                    let code = umx_cli::emit(&failed, output);
                    // req-86: a service start that finds a daemon running stands aside with exit 0, so launchd and
                    // systemd do not restart it in a loop.
                    let already = failed
                        .errors
                        .iter()
                        .any(|e| e.code() == umx_protocol::Code::DaemonAlreadyRunning.spec().code);
                    if flags.service && already { 0 } else { code }
                }
                Err(usage) => umx_cli::emit(&usage, output),
            }
        }
        [daemon, verb] if daemon == "daemon" && verb == "restart" => {
            umx_cli::emit(&umx_daemon::restart(&config, &build), output)
        }
        [daemon, verb] if daemon == "daemon" && verb == "install" => {
            umx_cli::emit(&umx_daemon::install(&config), output)
        }
        [daemon, verb] if daemon == "daemon" && verb == "uninstall" => {
            umx_cli::emit(&umx_daemon::uninstall(&config), output)
        }
        [daemon, verb] if daemon == "daemon" && verb == "service" => {
            umx_cli::emit(&umx_daemon::service(&config), output)
        }
        [daemon, verb] if daemon == "daemon" && verb == "status" => {
            let mut env = umx_cli::status(&config.socket, &build);
            if env.errors.is_empty() {
                env.data["service"] = umx_daemon::service_status(&config);
            }
            umx_cli::emit(&env, output)
        }
        [models, fetch, rest @ ..] if models == "models" && fetch == "fetch" => {
            match umx_cli::models_fetch_flags(rest) {
                Ok((name, dry_run)) => {
                    umx_cli::emit(&umx_daemon::models_fetch(&config, &name, dry_run), output)
                }
                Err(usage) => umx_cli::emit(&usage, output),
            }
        }
        [doctor, rest @ ..] if doctor == "doctor" => match umx_cli::doctor_flags(rest) {
            Ok(fix) => umx_cli::emit(
                &umx_daemon::doctor(
                    &config,
                    &build,
                    umx_daemon::Skill {
                        fingerprint: env!("UMX_SKILL"),
                        commit: env!("UMX_COMMIT_FULL"),
                    },
                    fix,
                ),
                output,
            ),
            Err(usage) => umx_cli::emit(&usage, output),
        },
        // `umx ui status|stop` (ws009 M1b2): ui.json and the process it names, here; never the daemon.
        [ui, verb, rest @ ..] if ui == "ui" && umx_cli::UI_LOCAL_VERBS.contains(&verb.as_str()) => {
            match umx_cli::ui_local_flags(verb, rest) {
                Ok(()) if verb == "stop" => umx_cli::emit(&ui::stop(&config), output),
                Ok(()) => umx_cli::emit(&ui::status(&config), output),
                Err(usage) => umx_cli::emit(&usage, output),
            }
        }
        // `umx ui` (plan 095): checked with the daemon by the CLI, then run here through the UiHost port.
        [ui, rest @ ..] if ui == "ui" && !rest.first().is_some_and(|w| umx_cli::is_ui_verb(w)) => {
            match umx_cli::ui_flags(rest, output.json).and_then(|flags| {
                match umx_cli::ui_preflight(&config.socket, &build, &flags.graphs) {
                    Ok(notes) => Ok((flags, notes)),
                    Err(refusal) => {
                        // A background view's refusal goes to the `--detach` waiting for it.
                        if let Some(to) = &flags.report_to {
                            ui::report(&config, to, &refusal);
                        }
                        Err(refusal)
                    }
                }
            }) {
                Ok((flags, notes)) if flags.detach => {
                    let run = ui::UiRun {
                        graphs: flags.graphs,
                        port: flags.port,
                        no_open: flags.no_open,
                        report_to: None,
                        notes: notes.clone(),
                    };
                    // The background view's own checks find the graphs loaded, so this launcher's notes (a load on
                    // demand above all) join its reply, each once (review R6).
                    let mut started = ui::detach(&config, &run);
                    if started.errors.is_empty() {
                        for note in notes {
                            if !started.warnings.contains(&note) && !started.hints.contains(&note) {
                                started = started.note(note);
                            }
                        }
                        started = started.settle_hints();
                    }
                    if !output.json && started.errors.is_empty() {
                        println!(
                            "umx ui: the graph view runs in the background at {} (pid {}). umx ui status shows it, umx ui stop ends it.",
                            started.data["url"].as_str().unwrap_or_default(),
                            started.data["pid"]
                        );
                        if !started.warnings.is_empty() || !started.hints.is_empty() {
                            let said = started
                                .warnings
                                .iter()
                                .chain(&started.hints)
                                .cloned()
                                .fold(Envelope::ok("ui", Value::Null), Envelope::note);
                            umx_cli::emit(&said, output);
                        }
                        0
                    } else {
                        umx_cli::emit(&started, output)
                    }
                }
                Ok((flags, notes)) => {
                    let detached = flags.report_to.is_some();
                    let run = ui::UiRun {
                        graphs: flags.graphs,
                        port: flags.port,
                        no_open: flags.no_open,
                        report_to: flags.report_to,
                        notes: notes.clone(),
                    };
                    let stopped = ui::ui(&config, &build, &run, |ready| {
                        // A background view reported to the `--detach` that started it; its stdout is /dev/null.
                        if detached {
                            return;
                        }
                        if output.json {
                            umx_cli::emit(&ready.envelope(&notes), output);
                            return;
                        } else if run.no_open {
                            // The link holds the token: stdout only, once, as text (never an envelope or a log).
                            println!("umx ui: open {}\n  Ctrl-C stops the view.", ready.link);
                        } else {
                            println!(
                                "umx ui: the graph view is open at {} (Ctrl-C stops it).",
                                ready.url
                            );
                        }
                        // What the checks said (stale_files, F144; graph_loaded_on_demand, a hint), each with its
                        // next, after the ready line.
                        if !notes.is_empty() {
                            let said = notes
                                .iter()
                                .cloned()
                                .fold(Envelope::ok("ui", Value::Null), Envelope::note)
                                .settle_hints();
                            umx_cli::emit(&said, output);
                        }
                    });
                    umx_cli::emit(&stopped, output)
                }
                Err(refusal) => umx_cli::emit(&refusal, output),
            }
        }
        _ => {
            let actor = if umx_cli::needs_actor(&args) {
                Some(actor_claim(umx_compose::client_actor(
                    mode(),
                    &config.search_path,
                    budgets,
                )))
            } else {
                None
            };
            umx_cli::run(
                &args,
                &umx_cli::Context {
                    socket: config.socket,
                    build,
                    output,
                    by: config.author,
                    graph: config.graph,
                    actor,
                },
            )
        }
    };
    if !umx_compose::finish_client_tools(
        std::time::Instant::now() + std::time::Duration::from_secs(2),
    ) {
        // A kernel-blocked exec or reap can outlive the reply; it remains owned until process exit. The one report of
        // a helper umx could not clean up: tools print nothing themselves (plan 188 item 6). Names no argv, env or id.
        eprintln!(
            "umx: identity helper cleanup is still waiting on the operating system; the command's result above stands"
        );
    }
    ExitCode::from(code)
}

#[cfg(feature = "test-driver")]
pub fn generous_budgets() -> umx_compose::ClientBudgets {
    use std::time::Duration;
    umx_compose::ClientBudgets {
        identity_work: Duration::from_secs(120),
        identity_hard: Duration::from_secs(150),
        service_tool: Duration::from_secs(120),
        token_tool: Duration::from_secs(120),
    }
}

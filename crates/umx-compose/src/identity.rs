use crate::{ClientBudgets, Mode};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use umx_adapter_system::agent_identity::{
    ENV_KEYS, EnvAgentIdentity, IdentityEnv, PijAgentIdentity, env_key_enabled,
};
use umx_core::ports::{AgentIdentity, IdentityError};
use umx_core::provenance::{
    Actor, ActorInput, InvalidActor, Tier, hash_session_id, identity_value,
};

fn tool(search_path: &[PathBuf], by: Instant) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    search_path
        .iter()
        .filter(|dir| dir.is_absolute())
        .find_map(|dir| {
            if Instant::now() >= by {
                return None;
            }
            let path = dir.join("pij-rs");
            let metadata = path.metadata().ok()?;
            if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
                return None;
            }
            let path = path.canonicalize().ok()?;
            (Instant::now() < by).then_some(path)
        })
}

#[expect(
    clippy::disallowed_methods,
    reason = "exact-name client identity allow-list (R2)"
)]
fn environment() -> IdentityEnv {
    let values: BTreeMap<String, String> = ENV_KEYS
        .iter()
        .filter(|key| env_key_enabled(key))
        .filter_map(|key| Some(((*key).into(), std::env::var(key).ok()?)))
        .collect();
    IdentityEnv::new(values)
}

fn ready_actor(actor: Actor) -> Result<Actor, InvalidActor> {
    let mut input: ActorInput = actor.into();
    for (field, value) in [
        ("seat", &mut input.seat),
        ("role", &mut input.role),
        ("model", &mut input.model),
        ("effort", &mut input.effort),
        ("harness", &mut input.harness),
        ("user", &mut input.user),
    ] {
        if let Some(raw) = value {
            *raw = identity_value(field, raw)?;
        }
    }
    input.harness_session = input
        .harness_session
        .as_deref()
        .map(hash_session_id)
        .transpose()
        .map_err(|_| InvalidActor("harness_session"))?;
    Actor::try_from(input)
}

/// One call per source, no cache across processes, and raw sessions are hashed before returning to the binary.
pub fn actor_from_sources(
    pij: &dyn AgentIdentity,
    env: &dyn AgentIdentity,
    user: Option<&str>,
) -> Actor {
    if let Some(actor) = ready_source(pij) {
        return actor;
    }
    fallback_actor(env, user)
}

fn ready_source(source: &dyn AgentIdentity) -> Option<Actor> {
    match AgentIdentity::current(source) {
        Ok(Some(actor)) => ready_actor(actor).ok(),
        Ok(None) | Err(IdentityError::Unavailable(_)) | Err(IdentityError::Invalid(_)) => None,
    }
}

fn fallback_actor(env: &dyn AgentIdentity, user: Option<&str>) -> Actor {
    if let Some(actor) = ready_source(env) {
        return actor;
    }
    let Some(user) = user.and_then(|raw| identity_value("user", raw).ok()) else {
        return Actor::unknown();
    };
    Actor::try_from(ActorInput {
        tier: Tier::Cli,
        seat: None,
        role: None,
        model: None,
        effort: None,
        harness: None,
        harness_session: None,
        user: Some(user),
    })
    .expect("validated cli actor")
}

fn from_snapshot(
    snapshot: IdentityEnv,
    search_path: &[PathBuf],
    start: Instant,
    budgets: ClientBudgets,
) -> Actor {
    let work = start + budgets.identity_work;
    let hard = start + budgets.identity_hard;
    let source_env = snapshot.clone();
    let paths = search_path.to_vec();
    let (sent, received) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("umx-client-identity".into())
        .spawn(move || {
            let path = tool(&paths, work);
            let ready = if Instant::now() < work {
                let pij = PijAgentIdentity::new(path, source_env).with_deadlines(work, hard);
                ready_source(&pij)
            } else {
                None
            };
            let _ = sent.send(ready);
        });
    if let Ok(handle) = spawned {
        umx_adapter_system::register_tool_helper(handle);
        if let Ok(Some(actor)) =
            received.recv_timeout(work.saturating_duration_since(Instant::now()))
            && Instant::now() < work
        {
            return actor;
        }
    }
    let user = snapshot.login();
    fallback_actor(&EnvAgentIdentity::new(Ok(snapshot)), user.as_deref())
}

/// Called for writes and UiCommand, as listed by umx_cli::needs_actor; never daemon composition or content reads.
pub fn client_actor(mode: Mode, search_path: &[PathBuf], budgets: ClientBudgets) -> Actor {
    let start = Instant::now();
    match mode {
        Mode::Real => from_snapshot(environment(), search_path, start, budgets),
        #[cfg(feature = "stub")]
        Mode::Stub => actor_from_sources(
            &umx_fakes::FakeAgentIdentity::new(Ok(Some(Actor::unknown()))),
            &umx_fakes::FakeAgentIdentity::default(),
            None,
        ),
    }
}

/// Configured primary client adapter. No identity discovery runs for the wiring inventory.
pub fn client_adapters(mode: Mode, search_path: &[PathBuf]) -> Vec<(&'static str, &'static str)> {
    let id = match mode {
        Mode::Real => match tool(search_path, Instant::now() + Duration::from_millis(1750)) {
            Some(path) => PijAgentIdentity::new(Some(path), IdentityEnv::default()).adapter_id(),
            None => EnvAgentIdentity::new(Ok(IdentityEnv::default())).adapter_id(),
        },
        #[cfg(feature = "stub")]
        Mode::Stub => umx_fakes::FakeAgentIdentity::default().adapter_id(),
    };
    vec![("agent-identity", id)]
}

#[cfg(test)]
mod budget_tests {
    use super::*;

    #[test]
    fn client_defaults_preserve_the_production_caps() {
        let budgets = ClientBudgets::default();
        assert_eq!(budgets.identity_work, Duration::from_millis(1750));
        assert_eq!(budgets.identity_hard, Duration::from_secs(2));
        assert_eq!(budgets.service_tool, Duration::from_secs(10));
        assert_eq!(budgets.token_tool, Duration::from_secs(10));
    }

    #[test]
    fn expired_identity_work_returns_without_waiting_for_the_hard_deadline() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        struct Temp(PathBuf);
        impl Drop for Temp {
            fn drop(&mut self) {
                fs::remove_dir_all(&self.0).unwrap();
            }
        }
        let path = std::env::temp_dir().join(format!("umx-expired-capture-{}", std::process::id()));
        fs::create_dir(&path).unwrap();
        let _cleanup = Temp(path.clone());
        let tool = path.join("pij-rs");
        fs::write(&tool, "#!/bin/sh\nexec /bin/sleep 120\n").unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o700)).unwrap();
        let snapshot = IdentityEnv::new(BTreeMap::from([(
            "COPILOT_AGENT_SESSION_ID".into(),
            "ExpiredFixtureSession".into(),
        )]));
        let start = Instant::now();
        let actor = from_snapshot(
            snapshot,
            &[path],
            start,
            ClientBudgets {
                identity_work: Duration::ZERO,
                identity_hard: Duration::from_secs(35),
                ..ClientBudgets::default()
            },
        );
        let took = start.elapsed();
        assert!(crate::finish_client_tools(
            Instant::now() + Duration::from_secs(5)
        ));
        assert!(
            took < Duration::from_secs(10),
            "expired work waited for an ordinary or hard deadline: {took:?}"
        );
        assert_eq!(actor.fields().tier, Tier::Harness);
    }
}

#[cfg(all(test, feature = "stub"))]
mod tests {
    use super::*;
    use umx_fakes::FakeAgentIdentity;

    #[test]
    fn a_slow_tool_falls_to_the_harness_within_the_whole_budget_and_the_write_succeeds() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use std::process::{Command, Stdio};
        struct Temp(PathBuf);
        impl Drop for Temp {
            fn drop(&mut self) {
                fs::remove_dir_all(&self.0).unwrap();
            }
        }
        let path = std::env::temp_dir().join(format!("umx-capture-budget-{}", std::process::id()));
        fs::create_dir(&path).unwrap();
        let _cleanup = Temp(path.clone());
        let executable = path.join("pij-rs");
        let pidfile = path.join("leader");
        let quoted = pidfile.display().to_string().replace('\'', "'\\''");
        fs::write(
            &executable,
            format!("#!/bin/sh\necho $$ > '{quoted}'\nsleep 30\n"),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let snapshot = IdentityEnv::new(BTreeMap::from([
            (
                "COPILOT_AGENT_SESSION_ID".into(),
                "FallbackSession_01".into(),
            ),
            ("USER".into(), "writer".into()),
        ]));
        let start = Instant::now();
        let (actor, took) = std::thread::scope(|scope| {
            let paths = vec![path.clone()];
            let capture = scope.spawn(move || {
                let actor = from_snapshot(snapshot, &paths, start, ClientBudgets::default());
                (actor, start.elapsed())
            });
            let pid = loop {
                match fs::read_to_string(&pidfile) {
                    Ok(text) => {
                        if let Ok(pid) = text.trim().parse::<u32>() {
                            break pid;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => panic!("read slow-tool readiness: {error}"),
                }
                assert!(
                    start.elapsed() < Duration::from_millis(1750),
                    "the slow-tool fixture must start within capture's work budget"
                );
                std::thread::sleep(Duration::from_millis(2));
            };
            let group = |pid: u32| {
                Command::new("/usr/bin/pgrep")
                    .args(["-g", &pid.to_string()])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .unwrap()
                    .code()
            };
            assert_eq!(
                group(pid),
                Some(0),
                "the group-presence probe has a positive control"
            );
            let captured = capture.join().unwrap();
            assert!(crate::finish_client_tools(
                Instant::now() + Duration::from_secs(2)
            ));
            let until = Instant::now() + Duration::from_secs(1);
            while group(pid) == Some(0) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(group(pid), Some(1), "the whole tool group is gone");
            captured
        });
        assert!(took <= Duration::from_millis(2100), "capture took {took:?}");
        assert_eq!(actor.fields().tier, Tier::Harness);
        assert!(umx_core::provenance::is_session_hash(
            actor.fields().harness_session.as_deref().unwrap()
        ));
        let config = crate::Config::at(path.join("home"), None).roots([path.to_str().unwrap()]);
        let mut svc = crate::compose(Mode::Real, &config);
        let ctx = umx_services::Ctx::new("writer").actor(actor);
        svc.graph_new(
            &ctx,
            path.join("g").to_str().unwrap(),
            "budget",
            None,
            false,
            None,
        )
        .unwrap();
        let written = svc
            .node_add(
                &ctx,
                umx_services::NodeAddRequest {
                    kind: "problem".into(),
                    title: "Write after timed-out capture".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(written.value.node.authored_by_session.is_some());
    }

    fn fixture_actor() -> Actor {
        Actor::try_from(ActorInput {
            tier: Tier::Harness,
            seat: None,
            role: None,
            model: None,
            effort: None,
            harness: Some("copilot".into()),
            harness_session: Some("ClientSession_01".into()),
            user: Some("client-writer".into()),
        })
        .expect("valid client fixture")
    }

    #[test]
    fn client_resolution_uses_each_source_once_and_hashes_before_returning() {
        let actor = fixture_actor();
        let expected = hash_session_id(actor.fields().harness_session.as_deref().unwrap()).unwrap();
        let pij = FakeAgentIdentity::new(Ok(Some(actor)));
        let env = FakeAgentIdentity::default();
        let ready = actor_from_sources(&pij, &env, Some("writer"));
        assert_eq!((pij.calls(), env.calls()), (1, 0));
        assert_eq!(
            ready.fields().harness_session.as_deref(),
            Some(expected.as_str())
        );
    }

    #[test]
    fn identity_failure_falls_through_to_env_cli_and_unknown() {
        let pij =
            FakeAgentIdentity::new(Err(IdentityError::Unavailable("whoami unavailable".into())));
        let env = FakeAgentIdentity::new(Ok(Some(fixture_actor())));
        assert_eq!(
            actor_from_sources(&pij, &env, None).fields().tier,
            Tier::Harness
        );
        assert_eq!((pij.calls(), env.calls()), (1, 1));
        let missing = FakeAgentIdentity::default();
        assert_eq!(
            actor_from_sources(&missing, &missing, Some("writer"))
                .fields()
                .tier,
            Tier::Cli
        );
        assert_eq!(
            actor_from_sources(&missing, &missing, None).fields().tier,
            Tier::Unknown
        );
    }
}

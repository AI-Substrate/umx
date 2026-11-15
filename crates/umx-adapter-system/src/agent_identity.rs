use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};
use umx_core::Value;
use umx_core::ports::{AgentIdentity, IdentityError};
use umx_core::provenance::{Actor, ActorInput, Tier, identity_value, is_pij_seat, session_id};

pub const WORK_TIMEOUT: Duration = Duration::from_millis(1750);
pub const HARD_TIMEOUT: Duration = Duration::from_secs(2);

pub const ENV_KEYS: &[&str] = &[
    "COPILOT_AGENT_SESSION_ID",
    "PIJ_SESSION_ID",
    "CLAUDE_CODE_SESSION_ID",
    "CODEX_THREAD_ID",
    "USER",
    "LOGNAME",
];
pub const HARNESS_VARIABLES: &[(&str, &str, bool)] = &[
    ("copilot", "COPILOT_AGENT_SESSION_ID", true),
    ("claude", "CLAUDE_CODE_SESSION_ID", true),
    // Not measured on a live Codex seat yet; security requires absent until confirmed.
    ("codex", "CODEX_THREAD_ID", false),
];

pub fn env_key_enabled(key: &str) -> bool {
    ENV_KEYS.contains(&key)
        && HARNESS_VARIABLES
            .iter()
            .find(|(_, variable, _)| *variable == key)
            .is_none_or(|(_, _, confirmed)| *confirmed)
}

#[derive(Clone, Default)]
pub struct IdentityEnv(BTreeMap<String, String>);

impl fmt::Debug for IdentityEnv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "IdentityEnv(<{} redacted>)", self.0.len())
    }
}

impl IdentityEnv {
    pub fn new(values: BTreeMap<String, String>) -> Self {
        Self(
            values
                .into_iter()
                .filter(|(key, _)| env_key_enabled(key))
                .collect(),
        )
    }

    pub fn login(&self) -> Option<String> {
        umx_core::provenance::login_user(
            self.0.get("USER").map(String::as_str),
            self.0.get("LOGNAME").map(String::as_str),
        )
    }

    fn seat_hint(&self) -> Option<&str> {
        self.0
            .get("PIJ_SESSION_ID")
            .map(String::as_str)
            .filter(|s| is_pij_seat(s))
    }
}

pub struct EnvAgentIdentity(Result<IdentityEnv, IdentityError>);

impl EnvAgentIdentity {
    pub fn new(snapshot: Result<IdentityEnv, IdentityError>) -> Self {
        Self(snapshot)
    }
}

impl AgentIdentity for EnvAgentIdentity {
    fn adapter_id(&self) -> &'static str {
        "env.agent-identity"
    }

    fn current(&self) -> Result<Option<Actor>, IdentityError> {
        let env = self.0.as_ref().map_err(Clone::clone)?;
        for &(harness, key, confirmed) in HARNESS_VARIABLES {
            if confirmed && let Some(raw) = env.0.get(key) {
                return Actor::try_from(ActorInput {
                    tier: Tier::Harness,
                    seat: None,
                    role: None,
                    model: None,
                    effort: None,
                    harness: Some(harness.into()),
                    harness_session: session_id(raw).then(|| raw.clone()),
                    user: Some(env.login().unwrap_or_else(|| "unknown".into())),
                })
                .map(Some)
                .map_err(|error| IdentityError::Invalid(error.0));
            }
        }
        Ok(None)
    }
}

pub struct PijAgentIdentity {
    tool: Option<PathBuf>,
    env: IdentityEnv,
    deadlines: Option<(Instant, Instant)>,
}

impl PijAgentIdentity {
    pub fn new(tool: Option<PathBuf>, env: IdentityEnv) -> Self {
        Self {
            tool,
            env,
            deadlines: None,
        }
    }

    pub fn with_deadlines(mut self, work: Instant, hard: Instant) -> Self {
        self.deadlines = Some((work, hard));
        self
    }

    fn json(
        &self,
        args: &[&str],
        step: &'static str,
        deadline: Instant,
        hard_deadline: Instant,
    ) -> Result<Value, IdentityError> {
        let tool = self
            .tool
            .as_ref()
            .ok_or_else(|| IdentityError::Unavailable("whoami tool unavailable".into()))?;
        let mut command = Command::new(tool);
        command.args(args);
        for &(_, variable, _) in HARNESS_VARIABLES {
            command.env_remove(variable);
        }
        match self.env.seat_hint() {
            Some(seat) => {
                command.env("PIJ_SESSION_ID", seat);
            }
            None => {
                command.env_remove("PIJ_SESSION_ID");
            }
        }
        let output = crate::tool::run_until(command, deadline, hard_deadline, Some(64 << 10))
            .map_err(|_| IdentityError::Unavailable(format!("{step} unavailable")))?;
        if !output.status.success() {
            return Err(IdentityError::Unavailable(format!("{step} failed")));
        }
        let text =
            std::str::from_utf8(&output.stdout).map_err(|_| IdentityError::Invalid("whoami"))?;
        let value: Value = text.parse().map_err(|_| IdentityError::Invalid("whoami"))?;
        if Instant::now() >= deadline {
            return Err(IdentityError::Unavailable("capture deadline".into()));
        }
        if value.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(IdentityError::Unavailable(format!("{step} refused")));
        }
        Ok(value)
    }
}

fn optional(field: &'static str, value: Option<&Value>) -> Option<String> {
    let value = value?;
    if value.is_null() {
        return None;
    }
    Some(
        value
            .as_str()
            .and_then(|v| identity_value(field, v).ok())
            .unwrap_or_else(|| "unknown".into()),
    )
}

impl AgentIdentity for PijAgentIdentity {
    fn adapter_id(&self) -> &'static str {
        "pij.agent-identity"
    }

    fn current(&self) -> Result<Option<Actor>, IdentityError> {
        let start = Instant::now();
        let (deadline, hard_deadline) = self
            .deadlines
            .unwrap_or((start + WORK_TIMEOUT, start + HARD_TIMEOUT));
        let who = self.json(&["--json", "whoami"], "whoami", deadline, hard_deadline)?;
        let data = who.get("data").unwrap_or(&Value::Null);
        if data.is_null() || data.get("id").is_none() {
            return Ok(None);
        }
        let seat = data
            .get("id")
            .and_then(Value::as_str)
            .ok_or(IdentityError::Invalid("seat"))?;
        let seat = identity_value("seat", seat).map_err(|_| IdentityError::Invalid("seat"))?;
        let harness = data
            .get("harness")
            .and_then(Value::as_str)
            .ok_or(IdentityError::Invalid("harness"))?;
        let harness =
            identity_value("harness", harness).map_err(|_| IdentityError::Invalid("harness"))?;
        let state = self
            .json(
                &["--json", "state", &seat],
                "state",
                deadline,
                hard_deadline,
            )
            .ok();
        let mut model = optional(
            "model",
            state
                .as_ref()
                .and_then(|v| v.pointer("/data/sessionStatus/status/model/value")),
        );
        let mut effort = None;
        if model.is_none()
            && let Ok(list) = self.json(&["--json", "list"], "list", deadline, hard_deadline)
            && let Some(row) = list
                .pointer("/data/seats")
                .and_then(Value::as_array)
                .and_then(|rows| {
                    rows.iter()
                        .find(|r| r.get("id").and_then(Value::as_str) == Some(seat.as_str()))
                })
        {
            model = optional("model", row.get("model"));
            effort = optional("effort", row.get("effort"));
        }
        if Instant::now() >= deadline {
            return Err(IdentityError::Unavailable("capture deadline".into()));
        }
        let raw_session = data
            .get("session")
            .and_then(Value::as_str)
            .filter(|s| session_id(s))
            .map(str::to_owned);
        Actor::try_from(ActorInput {
            tier: Tier::Pij,
            seat: Some(seat),
            harness: Some(harness),
            harness_session: raw_session,
            role: optional("role", data.get("role")),
            model,
            effort,
            user: self.env.login(),
        })
        .map(Some)
        .map_err(|error| IdentityError::Invalid(error.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use umx_contracts::agent_identity::{OUTPUT_MARKER, Scenario, secret};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "umx-identity-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn tool(&self, script: &str) -> PathBuf {
            let path = self.0.join("pij-rs");
            fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            path
        }

        fn quoted(&self, name: &str) -> String {
            format!(
                "'{}'",
                self.0
                    .join(name)
                    .display()
                    .to_string()
                    .replace('\'', "'\\''")
            )
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn env(values: &[(&str, &str)]) -> IdentityEnv {
        IdentityEnv::new(
            values
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
        )
    }

    fn test_port(tool: PathBuf, env: IdentityEnv) -> PijAgentIdentity {
        let start = Instant::now();
        PijAgentIdentity::new(Some(tool), env).with_deadlines(
            start + Duration::from_secs(120),
            start + Duration::from_secs(150),
        )
    }

    fn env_fixture(scenario: Scenario) -> EnvAgentIdentity {
        let value = match scenario {
            Scenario::Present => Ok(env(&[
                ("COPILOT_AGENT_SESSION_ID", "Session_01"),
                ("USER", "writer"),
            ])),
            Scenario::Absent => Ok(IdentityEnv::default()),
            Scenario::Invalid => Err(IdentityError::Invalid("user")),
            Scenario::Overlong => Ok(env(&[
                ("COPILOT_AGENT_SESSION_ID", "Session_01"),
                ("USER", &"x".repeat(129)),
            ])),
            Scenario::Secret => Ok(env(&[
                ("COPILOT_AGENT_SESSION_ID", "Session_01"),
                ("USER", &secret()),
            ])),
            Scenario::Unavailable => Err(IdentityError::Unavailable("env unavailable".into())),
        };
        EnvAgentIdentity::new(value)
    }

    mod env_contract {
        use super::*;
        umx_contracts::agent_identity_contract!(
            env_fixture;
            ai_c1_present_is_a_valid_actor(),
            ai_c2_absence_is_not_an_error(),
            ai_c3_invalid_names_a_field_only(),
            ai_c4_overlong_values_are_not_returned(),
            ai_c5_secret_values_are_not_returned(),
            ai_c6_unavailable_never_echoes_output(),
        );
    }

    struct PijFixture {
        port: PijAgentIdentity,
        _temp: Temp,
    }
    impl AgentIdentity for PijFixture {
        fn adapter_id(&self) -> &'static str {
            self.port.adapter_id()
        }
        fn current(&self) -> Result<Option<Actor>, IdentityError> {
            self.port.current()
        }
    }

    fn pij_fixture(scenario: Scenario) -> PijFixture {
        let temp = Temp::new();
        let who = match scenario {
            Scenario::Absent => r#"{"ok":true,"data":null}"#,
            Scenario::Invalid => r#"{"ok":true,"data":{"id":"-bad","harness":"copilot"}}"#,
            _ => {
                r#"{"ok":true,"data":{"id":"pij-calm-otter","harness":"copilot","session":"Session_01","role":"coder"}}"#
            }
        };
        let script = if matches!(scenario, Scenario::Unavailable) {
            format!("printf '%s' '{OUTPUT_MARKER}' >&2\nexit 7")
        } else {
            format!(
                "case \"$2\" in\nwhoami) printf '%s' '{who}';;\nstate) printf '%s' '{{\"ok\":true,\"data\":{{\"sessionStatus\":{{\"status\":{{\"model\":{{\"value\":\"openai/GPT-5.6\"}}}}}}}}}}';;\n*) exit 7;;\nesac"
            )
        };
        let login = match scenario {
            Scenario::Overlong => "x".repeat(129),
            Scenario::Secret => secret(),
            _ => "writer".into(),
        };
        let tool = temp.tool(&script);
        PijFixture {
            port: test_port(tool, env(&[("USER", &login)])),
            _temp: temp,
        }
    }

    mod pij_contract {
        use super::*;
        umx_contracts::agent_identity_contract!(
            pij_fixture;
            ai_c1_present_is_a_valid_actor(),
            ai_c2_absence_is_not_an_error(),
            ai_c3_invalid_names_a_field_only(),
            ai_c4_overlong_values_are_not_returned(),
            ai_c5_secret_values_are_not_returned(),
            ai_c6_unavailable_never_echoes_output(),
        );
    }

    #[test]
    fn unknown_and_unconfirmed_variables_are_ignored_and_bad_session_is_unknown() {
        let values = env(&[
            ("FOO_SESSION_ID", &secret()),
            ("CLAUDE_CODE_MESSAGING_TOKEN", &secret()),
            ("CODEX_THREAD_ID", "Session_01"),
            ("PIJ_SESSION_ID", "-bad"),
            ("USER", "writer"),
        ]);
        assert!(values.seat_hint().is_none());
        assert!(!format!("{values:?}").contains(&secret()));
        assert!(
            EnvAgentIdentity::new(Ok(values))
                .current()
                .unwrap()
                .is_none()
        );
        let actor = EnvAgentIdentity::new(Ok(env(&[
            ("COPILOT_AGENT_SESSION_ID", "../bad"),
            ("USER", "writer"),
        ])))
        .current()
        .unwrap()
        .unwrap();
        assert_eq!(actor.session_key(), "session/copilot/unknown");
    }

    #[test]
    fn whoami_runs_once_and_state_falls_back_to_list_with_nullable_effort() {
        let temp = Temp::new();
        let calls = temp.quoted("calls");
        let script = format!(
            "printf '%s\\n' \"$2\" >> {calls}\ncase \"$2\" in\nwhoami) printf '%s' '{{\"ok\":true,\"data\":{{\"id\":\"pij-calm-otter\",\"harness\":\"copilot\",\"session\":\"Session_01\"}}}}';;\nstate) exit 7;;\nlist) printf '%s' '{{\"ok\":true,\"data\":{{\"seats\":[{{\"id\":\"pij-calm-otter\",\"model\":\"claude-opus-5.5\",\"effort\":\"high\"}}]}}}}';;\nesac"
        );
        let port = test_port(temp.tool(&script), env(&[("USER", "writer")]));
        let actor = port.current().unwrap().unwrap();
        assert_eq!(actor.fields().model.as_deref(), Some("claude-opus-5.5"));
        assert_eq!(actor.fields().effort.as_deref(), Some("high"));
        assert_eq!(
            fs::read_to_string(temp.0.join("calls")).unwrap(),
            "whoami\nstate\nlist\n"
        );
    }

    #[test]
    fn oversized_output_is_unavailable_without_echoing_it() {
        let temp = Temp::new();
        let port = test_port(temp.tool("head -c 70000 /dev/zero"), IdentityEnv::default());
        let error = port.current().unwrap_err().to_string();
        assert_eq!(error, "identity unavailable: whoami unavailable");
    }

    #[test]
    fn the_shared_deadline_kills_descendants_and_returns_no_output() {
        fn running(pid: &str) -> bool {
            let state = Command::new("/bin/ps")
                .args(["-p", pid, "-o", "stat="])
                .output()
                .unwrap();
            let state_text = String::from_utf8_lossy(&state.stdout);
            let state_text = state_text.trim();
            state.status.success() && !state_text.is_empty() && !state_text.starts_with('Z')
        }

        let temp = Temp::new();
        let child_path = temp.0.join("child");
        let ready_path = temp.0.join("ready");
        let script = format!(
            "sleep 300 &\nprintf '%s' \"$!\" > {}\nprintf '%s' '{OUTPUT_MARKER}'\n: > {}\nwait",
            temp.quoted("child"),
            temp.quoted("ready")
        );
        let port = test_port(temp.tool(&script), IdentityEnv::default());
        let ordinary_deadline = port.deadlines.unwrap().0;
        let return_budget = Duration::from_secs(10);
        let (sent, received) = std::sync::mpsc::channel();
        crate::tool::WORK_DEADLINE_AFTER_SPAWN.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                let ready_by = Instant::now() + Duration::from_secs(90);
                while !ready_path.exists() {
                    assert!(Instant::now() < ready_by, "child did not signal readiness");
                    std::thread::sleep(Duration::from_millis(10));
                }
                let pid = fs::read_to_string(child_path).expect("ready child wrote its pid");
                assert!(running(&pid), "positive child control before deadline");
                let expired_at = Instant::now();
                assert!(
                    ordinary_deadline.saturating_duration_since(expired_at) > return_budget,
                    "readiness must leave ordinary and hard fallbacks beyond the return bound"
                );
                sent.send((pid, expired_at)).unwrap();
                expired_at
            }));
        });
        let result = port.current();
        let returned_at = Instant::now();
        let (pid, expired_at) = received
            .try_recv()
            .expect("deadline injected only after child readiness");
        let elapsed = returned_at.duration_since(expired_at);
        assert!(
            elapsed < return_budget,
            "capture returned {elapsed:?} after injected work expiry; expected under {return_budget:?}"
        );
        assert_eq!(
            result.unwrap_err().to_string(),
            "identity unavailable: whoami unavailable"
        );
        let stopped_by = Instant::now() + Duration::from_secs(30);
        while running(&pid) {
            assert!(
                Instant::now() < stopped_by,
                "descendant survived the deadline"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

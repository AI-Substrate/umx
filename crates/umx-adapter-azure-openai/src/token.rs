// Copyright (c) 2026 AI-Substrate. MIT. The token cache is adapted from flowspace3
// crates/providers/src/azure_openai.rs; the az/azd runner replaces azure_identity, whose
// AzureCliCredential runs `az` through `/bin/sh -c`.

//! Entra bearer tokens: where they come from, and how long one is reused.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The Entra scope every public-cloud Azure OpenAI resource is guarded by.
pub const COGNITIVE_SERVICES_SCOPE: &str = "https://cognitiveservices.azure.com/.default";

/// Fetch a new token this long before the old one expires, so a request issued just inside
/// the window cannot arrive just outside it.
pub const TOKEN_REFRESH_MARGIN: Duration = Duration::from_secs(300);

/// `az` can block on an interactive prompt; give up on it after this long.
pub const CLI_TIMEOUT: Duration = Duration::from_secs(10);

#[cfg(test)]
std::thread_local! {
    static DEADLINE_AFTER_SPAWN: std::cell::RefCell<Option<Box<dyn FnOnce() -> Instant>>> = const { std::cell::RefCell::new(None) };
}

/// When a CLI omits the expiry, reuse its token for this long (it lives about an hour).
const UNKNOWN_EXPIRY: Duration = Duration::from_secs(600);

/// A bearer token held in memory only. `Debug` never prints it.
#[derive(Clone)]
pub struct Token {
    secret: String,
    expires_at: SystemTime,
    /// Which tool printed it (`az` or `azd`) and the program as run (an absolute path when umx-compose resolved
    /// it from PATH).
    from: Option<(&'static str, String)>,
}

impl Token {
    pub fn new(secret: impl Into<String>, expires_at: SystemTime) -> Self {
        Self {
            secret: secret.into(),
            expires_at,
            from: None,
        }
    }

    /// Records which tool printed the token and the program as run, for `credential()`.
    pub fn from_program(mut self, kind: &'static str, program: impl Into<String>) -> Self {
        self.from = Some((kind, program.into()));
        self
    }

    pub(crate) fn secret(&self) -> &str {
        &self.secret
    }

    fn fresh(&self, now: SystemTime) -> bool {
        self.expires_at
            .duration_since(now)
            .is_ok_and(|left| left > TOKEN_REFRESH_MARGIN)
    }
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Token")
            .field("secret", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .field("from", &self.from)
            .finish()
    }
}

/// Why no token: a short reason naming the tool and its exit status, never its output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenError(pub String);

/// Something that hands out Entra tokens for [`COGNITIVE_SERVICES_SCOPE`].
pub trait TokenSource: Send + Sync {
    fn token(&self) -> Result<Token, TokenError>;
}

/// `az login`'s token, then `azd auth login`'s, each run directly (fixed argv, no shell) and
/// found on PATH. Only stdout's JSON is read; stderr is discarded, because it can hold a token.
#[derive(Debug, Clone)]
pub struct CliTokenSource {
    tools: Vec<Tool>,
    timeout: Duration,
}

#[derive(Debug, Clone)]
struct Tool {
    kind: &'static str,
    program: String,
    args: Vec<&'static str>,
}

impl Default for CliTokenSource {
    fn default() -> Self {
        Self::with_programs("az", "azd")
    }
}

impl CliTokenSource {
    /// The same argv as [`Default`], with the two programs named; tests point these at stubs.
    pub fn with_programs(az: impl Into<String>, azd: impl Into<String>) -> Self {
        Self {
            tools: vec![
                Tool {
                    kind: "az",
                    program: az.into(),
                    args: vec![
                        "account",
                        "get-access-token",
                        "--scope",
                        COGNITIVE_SERVICES_SCOPE,
                        "--output",
                        "json",
                    ],
                },
                Tool {
                    kind: "azd",
                    program: azd.into(),
                    args: vec![
                        "auth",
                        "token",
                        "--scope",
                        COGNITIVE_SERVICES_SCOPE,
                        "--output",
                        "json",
                    ],
                },
            ],
            timeout: CLI_TIMEOUT,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

impl TokenSource for CliTokenSource {
    fn token(&self) -> Result<Token, TokenError> {
        let mut reasons = Vec::new();
        for tool in &self.tools {
            match run(tool, self.timeout)
                .and_then(|out| parse_cli_token(&out, &tool.program))
                .map(|t| t.from_program(tool.kind, &tool.program))
            {
                Ok(token) => return Ok(token),
                Err(TokenError(reason)) => reasons.push(reason),
            }
        }
        Err(TokenError(reasons.join("; ")))
    }
}

/// Run one tool and return its stdout. The deadline covers everything: the tool's exit AND the
/// end of its stdout, which a background helper it started can hold open after it exits.
fn run(tool: &Tool, timeout: Duration) -> Result<Vec<u8>, TokenError> {
    let deadline = Instant::now() + timeout;
    let name = &tool.program;
    let mut command = Command::new(name);
    command
        .args(&tool.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // Its own process group, so a timeout can stop the tool and anything it started.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = spawn_retrying(&mut command).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => TokenError(format!("{name} not found on PATH")),
        _ => TokenError(format!("{name} could not start ({:?})", e.kind())),
    })?;
    #[cfg(test)]
    let deadline = DEADLINE_AFTER_SPAWN
        .with(|hook| hook.borrow_mut().take())
        .map_or(deadline, |hook| hook());

    // Read stdout on a thread; the result comes back on a channel so waiting for it is bounded.
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let (sent, received) = mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = sent.send(stdout.read_to_end(&mut out).map(|_| out));
    });
    let timed_out = || {
        TokenError(format!(
            "{name} gave no token within {}s (it may be waiting for a prompt)",
            timeout.as_secs()
        ))
    };

    let out = match received.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            stop(&mut child);
            return Err(TokenError(format!(
                "{name} output could not be read ({:?})",
                e.kind()
            )));
        }
        Err(_) => {
            stop(&mut child);
            return Err(timed_out());
        }
    };
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                stop(&mut child);
                return Err(timed_out());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => {
                stop(&mut child);
                return Err(TokenError(format!("{name} wait failed ({:?})", e.kind())));
            }
        }
    };
    if !status.success() {
        return Err(TokenError(format!("{name} exited with {status}")));
    }
    Ok(out)
}

/// Starts `command`, retrying for up to about a second while Linux reports the program's file busy (ETXTBSY).
/// A file that was just written stays busy while any process forked in that moment still holds the write handle,
/// until that child execs; any program that runs a freshly installed tool can meet it.
fn spawn_retrying(command: &mut Command) -> std::io::Result<std::process::Child> {
    let mut tries = 0;
    loop {
        match command.spawn() {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && tries < 100 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            other => return other,
        }
    }
}

/// Kill the tool's process group (the tool and whatever it started), then reap the tool.
/// The group is killed before the reap, while the tool's pid, which names the group, is still held.
fn stop(child: &mut Child) {
    #[cfg(unix)]
    {
        // By absolute path, so PATH cannot substitute another program.
        let _ = Command::new("/bin/kill")
            .args(["-KILL", "--", &format!("-{}", child.id())])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// az prints `accessToken` and `expires_on` (epoch seconds); `expiresOn` is local time and is
/// not read. azd prints `token` and `expiresOn` (RFC 3339, UTC).
fn parse_cli_token(stdout: &[u8], name: &str) -> Result<Token, TokenError> {
    #[derive(serde::Deserialize)]
    struct CliToken {
        #[serde(alias = "token", rename = "accessToken")]
        access_token: Option<String>,
        expires_on: Option<serde_json::Value>,
        #[serde(rename = "expiresOn")]
        expires_on_text: Option<String>,
    }
    let parsed: CliToken = serde_json::from_slice(stdout)
        .map_err(|_| TokenError(format!("{name} printed no token JSON")))?;
    let secret = parsed
        .access_token
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| TokenError(format!("{name} printed no token")))?;
    let epoch = match parsed.expires_on {
        Some(serde_json::Value::Number(n)) => n.as_u64(),
        Some(serde_json::Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    }
    .or_else(|| parsed.expires_on_text.as_deref().and_then(rfc3339_utc));
    let expires_at = match epoch {
        Some(secs) => UNIX_EPOCH + Duration::from_secs(secs),
        None => SystemTime::now() + UNKNOWN_EXPIRY,
    };
    Ok(Token::new(secret, expires_at))
}

/// Epoch seconds for `YYYY-MM-DDTHH:MM:SS[.frac]Z`; anything else (az's local time) is `None`.
fn rfc3339_utc(text: &str) -> Option<u64> {
    let text = text.strip_suffix('Z')?;
    let (date, time) = text.split_once('T')?;
    let mut d = date.splitn(3, '-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let time = time.split('.').next()?;
    let mut t = time.splitn(3, ':').map(|p| p.parse::<i64>().ok());
    let (hh, mm, ss) = (t.next()??, t.next()??, t.next()??);
    crate::retry::utc_epoch(y, m, day, hh, mm, ss)
}

/// The last token, reused until it is within [`TOKEN_REFRESH_MARGIN`] of expiring.
/// `az` does not cache, so without this every embed call would spawn a process.
pub(crate) struct TokenCache {
    source: Box<dyn TokenSource>,
    last: Mutex<Option<Token>>,
}

impl TokenCache {
    pub(crate) fn new(source: Box<dyn TokenSource>) -> Self {
        Self {
            source,
            last: Mutex::new(None),
        }
    }

    pub(crate) fn bearer(&self) -> Result<String, TokenError> {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(token) = last.as_ref().filter(|t| t.fresh(SystemTime::now())) {
            return Ok(token.secret().to_string());
        }
        let token = self.source.token()?;
        let secret = token.secret().to_string();
        *last = Some(token);
        Ok(secret)
    }

    /// The program that printed the cached token, while one is cached. Never waits: while a fetch holds the lock
    /// (az can take 10 s) it answers `None`, so a status read is never held up by a sign-in.
    pub(crate) fn from(&self) -> Option<(&'static str, String)> {
        let last = match self.last.try_lock() {
            Ok(last) => last,
            Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return None,
        };
        last.as_ref().and_then(|t| t.from.clone())
    }

    /// Drop the cached token, so the next call asks the source again (after a 401).
    pub(crate) fn forget(&self) {
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

impl std::fmt::Debug for TokenCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TokenCache")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn az_output_is_read_by_epoch_expiry() {
        let out = br#"{"accessToken":"t1","expiresOn":"2026-11-10 13:00:00.000000","expires_on":1794282000,"tokenType":"Bearer"}"#;
        let token = parse_cli_token(out, "az").unwrap();
        assert_eq!(token.secret(), "t1");
        assert_eq!(
            token.expires_at,
            UNIX_EPOCH + Duration::from_secs(1_794_282_000)
        );
    }

    #[test]
    fn azd_output_is_read_by_utc_expiry() {
        let out = br#"{"token":"t2","expiresOn":"2026-11-10T03:00:00Z"}"#;
        let token = parse_cli_token(out, "azd").unwrap();
        assert_eq!(token.secret(), "t2");
        assert_eq!(rfc3339_utc("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(rfc3339_utc("2000-03-01T00:00:01.5Z"), Some(951_868_801));
        assert_eq!(
            token.expires_at,
            UNIX_EPOCH + Duration::from_secs(1_794_279_600)
        );
    }

    #[test]
    fn local_time_expiry_is_never_trusted() {
        assert_eq!(rfc3339_utc("2026-11-10 13:00:00.000000"), None);
        let out = br#"{"accessToken":"t3","expiresOn":"2026-11-10 13:00:00.000000"}"#;
        let token = parse_cli_token(out, "az").unwrap();
        let left = token.expires_at.duration_since(SystemTime::now()).unwrap();
        assert!(left <= UNKNOWN_EXPIRY, "{left:?}");
    }

    #[test]
    fn debug_never_prints_the_token() {
        let token = Token::new("tok-DO-NOT-LEAK", SystemTime::now());
        assert!(!format!("{token:?}{token:#?}").contains("DO-NOT-LEAK"));
    }

    #[cfg(unix)]
    mod cli {
        use super::super::*;
        use std::os::unix::fs::PermissionsExt;

        /// A stub tool: a script that records its argv and prints `stdout` (or sleeps, or fails).
        fn stub(dir: &std::path::Path, name: &str, body: &str) -> String {
            let path = dir.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path.to_string_lossy().into_owned()
        }

        fn temp(name: &str) -> std::path::PathBuf {
            let dir = std::env::temp_dir().join(format!("umx-azure-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            dir
        }

        fn source(az: impl Into<String>, azd: impl Into<String>) -> CliTokenSource {
            CliTokenSource::with_programs(az, azd).with_timeout(Duration::from_secs(120))
        }

        fn alive(pid: &str) -> bool {
            let output = Command::new("/bin/ps")
                .args(["-p", pid.trim(), "-o", "stat="])
                .output()
                .unwrap();
            let state = String::from_utf8_lossy(&output.stdout);
            let state = state.trim();
            output.status.success() && !state.is_empty() && !state.starts_with('Z')
        }

        fn expire_when_ready(
            pidfile: &std::path::Path,
            ready: &std::path::Path,
        ) -> mpsc::Receiver<(String, Instant)> {
            let pidfile = pidfile.to_owned();
            let ready = ready.to_owned();
            let (sent, received) = mpsc::channel();
            let ordinary_deadline = Instant::now() + Duration::from_secs(120);
            DEADLINE_AFTER_SPAWN.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    let by = Instant::now() + Duration::from_secs(90);
                    while !ready.exists() {
                        assert!(
                            Instant::now() < by,
                            "token fixture did not signal readiness"
                        );
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    let pid = std::fs::read_to_string(pidfile).expect("ready tool wrote its pid");
                    assert!(alive(&pid), "positive tool control before injected expiry");
                    let now = Instant::now();
                    assert!(
                        ordinary_deadline.saturating_duration_since(now) > Duration::from_secs(10),
                        "readiness must leave the ordinary deadline beyond the return bound"
                    );
                    sent.send((pid, now)).unwrap();
                    now
                }));
            });
            received
        }

        fn stopped(pid: &str) {
            let by = Instant::now() + Duration::from_secs(30);
            while alive(pid) {
                assert!(Instant::now() < by, "token helper outlived the timeout");
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        #[test]
        fn az_runs_with_fixed_argv_and_its_token_is_used() {
            let dir = temp("argv");
            let log = dir.join("argv");
            let az = stub(
                &dir,
                "az",
                &format!(
                    r#"printf '%s\n' "$@" > '{}'; echo '{{"accessToken":"from-az","expires_on":4102444800}}'; echo 'secret-on-stderr' >&2"#,
                    log.display()
                ),
            );
            let source = source(az.clone(), "/nonexistent/azd");
            let token = source.token().unwrap();
            assert_eq!(token.secret(), "from-az");
            assert_eq!(token.from, Some(("az", az)));
            assert_eq!(
                std::fs::read_to_string(&log).unwrap(),
                "account\nget-access-token\n--scope\nhttps://cognitiveservices.azure.com/.default\n--output\njson\n"
            );
        }

        #[test]
        fn azd_is_the_fallback_and_failures_name_tools_not_output() {
            let dir = temp("fallback");
            let az = stub(
                &dir,
                "az",
                "echo 'stdout-secret'; echo 'stderr-secret' >&2; exit 1",
            );
            let azd = stub(
                &dir,
                "azd",
                r#"echo '{"token":"from-azd","expiresOn":"2100-01-01T00:00:00Z"}'"#,
            );
            let tokens = source(az.clone(), azd.clone());
            let token = tokens.token().unwrap();
            assert_eq!(token.secret(), "from-azd");
            // credential() reports the tool that gave the token, as run.
            assert_eq!(token.from, Some(("azd", azd)));

            let both_fail = source(az, "/nonexistent/azd");
            let TokenError(reason) = both_fail.token().unwrap_err();
            assert!(reason.contains("exited with"), "{reason}");
            assert!(reason.contains("not found on PATH"), "{reason}");
            assert!(!reason.contains("secret"), "{reason}");
        }

        /// The tool exits 0 with a token, but a helper it started keeps stdout open: the
        /// deadline still holds, and the helper is killed with the tool's process group.
        #[test]
        fn a_helper_holding_stdout_cannot_outlast_the_deadline() {
            let dir = temp("helper");
            let pidfile = dir.join("pid");
            let ready = dir.join("ready");
            let az = stub(
                &dir,
                "az",
                &format!(
                    r#"sleep 300 & echo $! > '{}'; echo '{{"accessToken":"t","expires_on":4102444800}}'; : > '{}'; exit 0"#,
                    pidfile.display(),
                    ready.display(),
                ),
            );
            let source = source(az, "/nonexistent/azd");
            let expiry = expire_when_ready(&pidfile, &ready);
            let TokenError(reason) = source.token().unwrap_err();
            let returned_at = Instant::now();
            let (pid, expired_at) = expiry.try_recv().expect("token deadline was injected");
            assert!(
                returned_at.duration_since(expired_at) < Duration::from_secs(10),
                "token capture ignored injected work expiry"
            );
            assert!(reason.contains("no token within"), "{reason}");
            stopped(&pid);
        }

        #[test]
        fn a_hanging_az_is_killed_at_the_timeout() {
            let dir = temp("hang");
            let pidfile = dir.join("pid");
            let ready = dir.join("ready");
            let az = stub(
                &dir,
                "az",
                &format!(
                    "printf '%s' $$ > '{}'; : > '{}'; exec /bin/sleep 300",
                    pidfile.display(),
                    ready.display(),
                ),
            );
            let source = source(az, "/nonexistent/azd");
            let expiry = expire_when_ready(&pidfile, &ready);
            let TokenError(reason) = source.token().unwrap_err();
            let returned_at = Instant::now();
            let (pid, expired_at) = expiry.try_recv().expect("token deadline was injected");
            assert!(
                returned_at.duration_since(expired_at) < Duration::from_secs(10),
                "hanging tool ignored injected work expiry"
            );
            assert!(reason.contains("no token within"), "{reason}");
            stopped(&pid);
        }
    }
}

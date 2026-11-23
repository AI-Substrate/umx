//! UiHost port (req-104, ws009 s2): runs the local graph view's server for `umx ui` and opens the browser at it.
//! Adapters: system.ui-host (node in its own process group, the browser by absolute path), fake.ui-host.
//!
//! The view has no per-run token: the server keeps it to this machine's browsers by Host, Origin and Sec-Fetch-Site
//! (apps/ui lib/guard.ts). The server's stdin stays open while umx runs, so when umx ends in any way (Ctrl-C, kill)
//! the server sees end of file and stops its whole process group.
//!
//! ws009 M1b2: the host also starts `umx ui` detached ([`UiHost::detach`], [`UiHost::report`]), reads a process's
//! start time and command line ([`UiHost::process`]) so `umx ui stop` signals only the process ui.json names
//! ([`UiHost::signal`]), and arms single-use launch codes on a running server ([`UiHost::launch_code`]): a code goes
//! only to the server's stdin and the browser, never argv, env or a log. A code lets a launch link name graphs.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// The bytes of randomness in a launch code; as unpadded base64url it is [`CODE_LEN`] characters.
pub const CODE_BYTES: usize = 32;
pub const CODE_LEN: usize = 43;

/// What to run: `<node> <app_dir>/bin/serve.mjs --stdin [--port=<n>]`, in `app_dir`.
#[derive(Clone, PartialEq, Eq)]
pub struct UiLaunch {
    /// Absolute path of node.
    pub node: String,
    /// The app folder, canonical and checked by the caller.
    pub app_dir: String,
    /// The port to bind on 127.0.0.1; `None` lets the server pick a free one.
    pub port: Option<u16>,
    /// The server's whole environment: nothing else is inherited.
    pub env: Vec<(String, String)>,
    /// How long the server may take to say it is ready.
    pub ready_ms: u64,
}

/// Debug never prints env values.
impl fmt::Debug for UiLaunch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiLaunch")
            .field("node", &self.node)
            .field("app_dir", &self.app_dir)
            .field("port", &self.port)
            .field("env", &self.env.iter().map(|(k, _)| k).collect::<Vec<_>>())
            .field("ready_ms", &self.ready_ms)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiError {
    /// The server could not start (node missing, not executable, ...): the step and the OS error kind.
    Start(String),
    /// The server did not say it was ready in time; it was stopped.
    TimedOut,
    /// The server ended before it was ready, with this exit code (`None`: a signal).
    Exited(Option<i32>),
    /// The browser could not be opened.
    Open(String),
    /// No launch code could be made, or one is malformed.
    Code(String),
}

impl fmt::Display for UiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UiError::Start(m) => write!(f, "the view's server could not start ({m})"),
            UiError::TimedOut => f.write_str("the view's server did not say it was ready in time"),
            UiError::Exited(Some(c)) => write!(f, "the view's server stopped with exit {c}"),
            UiError::Exited(None) => f.write_str("the view's server was stopped by a signal"),
            UiError::Open(m) => write!(f, "the browser could not be opened ({m})"),
            UiError::Code(m) => write!(f, "no launch code could be made ({m})"),
        }
    }
}

/// The flag [`UiHost::detach`] adds to the child's arguments, before the path of the socket it reports to.
pub const REPORT_FLAG: &str = "--report-to";

/// How long an armed launch code stays good, in ms (security D5).
pub const CODE_LIFE_MS: u64 = 60_000;

/// A process as the system reports it: when it started and its command line, as text to compare (never parsed for
/// meaning). `umx ui stop` signals a pid only when both match what ui.json recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiProcess {
    pub started: String,
    pub argv: String,
}

/// A detached child that reported: its pid and the one line it sent (an envelope as JSON, never a launch code).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiDetached {
    pub pid: u32,
    pub said: Vec<u8>,
}

/// Why the supervisor is about to stop the view's server ([`UiHost::ending`]): only these words reach its stdin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiEnd {
    /// The daemon restarted (another start id).
    Restarted,
    /// The daemon stopped answering.
    Unreachable,
}

impl UiEnd {
    pub fn as_str(self) -> &'static str {
        match self {
            UiEnd::Restarted => "restarted",
            UiEnd::Unreachable => "unreachable",
        }
    }
}

/// What a running server did with a launch code ([`UiHost::launch_code`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiCodeUse {
    /// A tab asked for events within the last 30 s: the code was dropped, and that tab shows the raise itself.
    Tab,
    /// Kept for [`CODE_LIFE_MS`], for one use: open the browser at a link carrying it.
    Armed,
}

/// A server [`UiHost::serve`] started. The host owns its processes: pass this back to ask about it or stop it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiServed {
    /// The host's own number for it.
    pub id: u64,
    /// The port it listens on, on 127.0.0.1.
    pub port: u16,
}

pub trait UiHost: Send + Sync {
    /// `<adapter>.ui-host`, as in the adapter matrix (d-6).
    fn adapter_id(&self) -> &'static str;

    /// A fresh launch code: [`CODE_BYTES`] random bytes as unpadded base64url ([`code_ok`]).
    fn code(&self) -> Result<String, UiError>;

    /// Starts the server and waits until it says it is ready, at most `launch.ready_ms`. A server that never says
    /// so is stopped (with its group) before this returns.
    fn serve(&self, launch: &UiLaunch) -> Result<UiServed, UiError>;

    /// Whether the server still runs. Never blocks; a server that ended is cleaned up (its group stopped).
    fn running(&self, server: &UiServed) -> bool;

    /// Stops the server's whole process group and waits for it. Repeating is harmless. Dropping the host stops
    /// every server it started.
    fn stop(&self, server: &UiServed);

    /// Opens the default browser at `url` without putting it in any process's argv. Only [`url_ok`] URLs.
    fn open(&self, url: &str) -> Result<(), UiError>;

    /// Starts `exe args.. --report-to <socket>` detached: a session of its own, stdin, stdout and stderr on /dev/null,
    /// never waited for. Waits at most `ready_ms` for the one line the child sends with [`UiHost::report`], then
    /// removes the socket. A child that exits first is [`UiError::Exited`]; one that never reports is stopped and
    /// [`UiError::TimedOut`].
    fn detach(&self, exe: &str, args: &[String], ready_ms: u64) -> Result<UiDetached, UiError>;

    /// The detached child's side: sends `line` to the socket its parent named after [`REPORT_FLAG`].
    fn report(&self, to: &str, line: &[u8]) -> Result<(), UiError>;

    /// The process `pid` as the system reports it, or None when no such process runs.
    fn process(&self, pid: u32) -> Option<UiProcess>;

    /// Asks `pid` to stop (TERM, that one process). The caller checks [`UiHost::process`] against its record first.
    fn signal(&self, pid: u32) -> Result<(), UiError>;

    /// Tells the running server why it is about to stop (one stdin line, `end <why>`), so its tabs can say so (ui-expert
    /// N1). The caller stops it after a short grace either way.
    fn ending(&self, server: &UiServed, why: UiEnd) -> Result<(), UiError>;

    /// Takes this UMX_HOME's one graph view (review B1): an exclusive hold for this host's life, let go when
    /// the host is dropped or its process ends in any way, never inherited by a process it starts. False: another
    /// process holds it. Taking it again in the same host is true.
    fn own_view(&self) -> Result<bool, UiError>;

    /// Hands `code` ([`code_ok`]) to the running server on its stdin. [`UiCodeUse::Armed`]: it is good once,
    /// for [`CODE_LIFE_MS`]; [`UiCodeUse::Tab`]: a tab is connected, and the code was dropped.
    fn launch_code(&self, server: &UiServed, code: &str) -> Result<UiCodeUse, UiError>;
}

/// A launch code as [`UiHost::code`] makes it.
pub fn code_ok(code: &str) -> bool {
    code.len() == CODE_LEN
        && code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A URL the host may open: `http://127.0.0.1:<port>/` and then only characters that need no quoting anywhere.
pub fn url_ok(url: &str) -> bool {
    url.len() <= 512
        && url.starts_with("http://127.0.0.1:")
        && url
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~:/#=&%".contains(&b))
}

/// Unpadded base64url, for launch codes.
pub fn base64url(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..=chunk.len() {
            out.push(char::from(ABC[((n >> (18 - 6 * i)) & 63) as usize]));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_matches_known_values() {
        assert_eq!(base64url(b""), "");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
        assert!(code_ok(&base64url(&[7u8; CODE_BYTES])));
    }

    #[test]
    fn only_loopback_urls_with_plain_characters_may_be_opened() {
        assert!(url_ok("http://127.0.0.1:4747/#t=abc_-&g=acme%20x"));
        for bad in [
            "http://localhost:4747/",
            "https://127.0.0.1:4747/",
            "http://127.0.0.1:4747/\"x",
            "http://127.0.0.1:4747/ x",
            "http://127.0.0.1:4747/\\",
            "http://127.0.0.1.evil:1/",
        ] {
            assert!(!url_ok(bad), "{bad}");
        }
    }
}

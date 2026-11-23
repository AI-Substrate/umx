//! UiHost contract. A factory gives a host and its stage: the stage builds launches whose server behaves as a clause
//! asks (for the real adapter, a stub script run by /bin/sh in a temp folder) and reports what that server saw, so no
//! clause runs node or opens a real browser.

use umx_core::ports::ui_host::{CODE_LEN, REPORT_FLAG, code_ok};
use umx_core::ports::{UiCodeUse, UiEnd, UiError, UiHost, UiLaunch};

pub const CLAUSES: &[&str] = &[
    "uh_c1_codes_are_base64url_and_never_repeat",
    "uh_c2_the_server_gets_no_secret_and_the_port_is_reported",
    "uh_c3_stop_or_dropping_the_host_leaves_no_process",
    "uh_c4_a_server_never_ready_times_out_and_leaves_no_process",
    "uh_c5_a_server_that_dies_before_ready_is_an_exit_and_leaves_no_process",
    "uh_c6_a_server_that_ends_is_seen_as_not_running",
    "uh_c7_open_hands_over_only_a_loopback_url",
    "uh_c8_a_launch_code_reaches_stdin_only_and_is_dropped_while_a_tab_is_connected",
    "uh_c9_a_process_is_reported_until_it_is_signalled",
    "uh_c10_a_detached_child_reports_one_line_or_its_end_is_an_error",
    "uh_c11_the_view_is_owned_by_one_host_at_a_time",
    "uh_c12_why_the_view_ends_reaches_the_server",
];

/// How a stage's detached child behaves (ws009 M1b2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Child {
    /// Sends `line` to the socket after the report flag, then exits.
    Reports(&'static str),
    /// Exits at once without reporting.
    Exits,
    /// Never reports and keeps running until signalled.
    Silent,
}

/// How a stage's server behaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Server {
    /// Says it is ready, reads its stdin until it closes, then stops its group.
    Ready,
    /// Never says it is ready; stops its group when its stdin closes.
    Silent,
    /// Exits with 3 before it is ready, leaving a child behind in its group.
    Dies,
    /// Says it is ready, then exits by itself shortly after.
    Ends,
}

pub trait UiStage {
    /// A fresh launch whose server behaves as `server`, binding `port` (or any port).
    fn launch(&self, server: Server, port: Option<u16>) -> UiLaunch;
    /// Every line the last server read from its stdin, in order.
    fn stdin_seen(&self) -> Vec<String>;
    /// The last server's argv and env, as text.
    fn argv_and_env_seen(&self) -> String;
    /// Whether any process of the last server (its leader or a child) is still alive.
    fn alive(&self) -> bool;
    /// The URLs the host handed to the browser, in order.
    fn opened(&self) -> Vec<String>;
    /// Whether the last server sees a connected tab.
    fn tab(&self, live: bool);
    /// The launch codes the last server read from its stdin, in order.
    fn codes_seen(&self) -> Vec<String>;
    /// An exe and its arguments that, started detached, behave as `child`.
    fn child(&self, child: Child) -> (String, Vec<String>);
    /// A process the stage started that runs until signalled: its pid.
    fn live_process(&self) -> u32;
    /// Whether the stage's process `pid` has ended (for a detached child that never reported, or a signalled one).
    fn ended(&self, pid: u32) -> bool;
    /// Makes another owner hold this UMX_HOME's view, or let it go.
    fn other_owner(&self, holds: bool);
    /// The word the last server read in an `end` line, if any.
    fn ending_seen(&self) -> Option<String>;
}

const CODE: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

fn settle<S: UiStage>(stage: &S) -> bool {
    // A group that was signalled may take a moment to go, longer on a loaded machine; give it up to 30 s.
    for _ in 0..600 {
        if !stage.alive() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    false
}

pub fn uh_c1_codes_are_base64url_and_never_repeat<P: UiHost, S: UiStage>(
    factory: impl Fn() -> (P, S),
) {
    let (host, _) = factory();
    assert!(host.adapter_id().ends_with(".ui-host"));
    let a = host.code().expect("code");
    let b = host.code().expect("code");
    assert_eq!(a.len(), CODE_LEN);
    assert!(code_ok(&a) && code_ok(&b), "{a} {b}");
    assert_ne!(a, b);
}

/// The view has no per-run token: starting the server writes nothing to its stdin, and its argv says `--stdin`.
pub fn uh_c2_the_server_gets_no_secret_and_the_port_is_reported<P: UiHost, S: UiStage>(
    factory: impl Fn() -> (P, S),
) {
    let (host, stage) = factory();
    let server = host
        .serve(&stage.launch(Server::Ready, Some(47_471)))
        .expect("serve");
    assert_eq!(server.port, 47_471);
    assert!(host.running(&server));
    let seen = stage.argv_and_env_seen();
    assert!(
        seen.contains("--stdin") && !seen.contains("--token-stdin"),
        "{seen}"
    );
    // Anything written before ready would be read by now, after the ready line; a code proves the pipe is read.
    let code = host.code().expect("code");
    assert_eq!(host.launch_code(&server, &code), Ok(UiCodeUse::Armed));
    assert_eq!(
        stage.stdin_seen(),
        vec![format!("code {code}")],
        "the server got only the code"
    );
    host.stop(&server);
    let any = host
        .serve(&stage.launch(Server::Ready, None))
        .expect("serve on any port");
    assert!(any.port > 0);
    assert_ne!(any.id, server.id);
    host.stop(&any);
}

pub fn uh_c3_stop_or_dropping_the_host_leaves_no_process<P: UiHost, S: UiStage>(
    factory: impl Fn() -> (P, S),
) {
    let (host, stage) = factory();
    let server = host
        .serve(&stage.launch(Server::Ready, None))
        .expect("serve");
    assert!(stage.alive());
    host.stop(&server);
    assert!(settle(&stage), "a process of the server outlived stop");
    assert!(!host.running(&server));
    host.stop(&server);
    // Dropping the host stops what it started.
    host.serve(&stage.launch(Server::Ready, None))
        .expect("serve");
    assert!(stage.alive());
    drop(host);
    assert!(settle(&stage), "a process of the server outlived its host");
}

pub fn uh_c4_a_server_never_ready_times_out_and_leaves_no_process<P: UiHost, S: UiStage>(
    factory: impl Fn() -> (P, S),
) {
    let (host, stage) = factory();
    let mut launch = stage.launch(Server::Silent, None);
    launch.ready_ms = 300;
    match host.serve(&launch) {
        Err(UiError::TimedOut) => {}
        Err(e) => panic!("expected TimedOut, got {e:?}"),
        Ok(_) => panic!("expected TimedOut, got a server"),
    }
    assert!(settle(&stage), "a timed-out server was left running");
}

pub fn uh_c5_a_server_that_dies_before_ready_is_an_exit_and_leaves_no_process<
    P: UiHost,
    S: UiStage,
>(
    factory: impl Fn() -> (P, S),
) {
    let (host, stage) = factory();
    match host.serve(&stage.launch(Server::Dies, None)) {
        Err(UiError::Exited(Some(3))) => {}
        Err(e) => panic!("expected Exited(3), got {e:?}"),
        Ok(_) => panic!("expected Exited(3), got a server"),
    }
    assert!(settle(&stage), "the dead server's child was left running");
}

pub fn uh_c6_a_server_that_ends_is_seen_as_not_running<P: UiHost, S: UiStage>(
    factory: impl Fn() -> (P, S),
) {
    let (host, stage) = factory();
    let server = host
        .serve(&stage.launch(Server::Ends, None))
        .expect("serve");
    let mut ended = false;
    for _ in 0..100 {
        if !host.running(&server) {
            ended = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(ended, "a server that ended still reads as running");
    assert!(settle(&stage));
}

pub fn uh_c7_open_hands_over_only_a_loopback_url<P: UiHost, S: UiStage>(
    factory: impl Fn() -> (P, S),
) {
    let (host, stage) = factory();
    let url = format!("http://127.0.0.1:4747/#t={CODE}&g=acme");
    host.open(&url).expect("open");
    for bad in [
        "http://example.com/",
        "http://127.0.0.1:4747/\" & do shell script \"x",
        "file:///etc/passwd",
    ] {
        assert!(
            matches!(host.open(bad), Err(UiError::Open(_))),
            "{bad} was opened"
        );
    }
    assert_eq!(stage.opened(), vec![url]);
}

pub fn uh_c8_a_launch_code_reaches_stdin_only_and_is_dropped_while_a_tab_is_connected<
    P: UiHost,
    S: UiStage,
>(
    factory: impl Fn() -> (P, S),
) {
    let (host, stage) = factory();
    let server = host
        .serve(&stage.launch(Server::Ready, None))
        .expect("serve");
    let code = host.code().expect("a code");
    assert_eq!(host.launch_code(&server, &code), Ok(UiCodeUse::Armed));
    assert_eq!(stage.codes_seen(), vec![code.clone()]);
    assert!(!stage.argv_and_env_seen().contains(&code));
    stage.tab(true);
    let other = host.code().expect("a code");
    assert_eq!(host.launch_code(&server, &other), Ok(UiCodeUse::Tab));
    stage.tab(false);
    host.stop(&server);
    assert!(settle(&stage));
    assert!(
        host.launch_code(&server, &other).is_err(),
        "a stopped server takes no code"
    );
}

pub fn uh_c9_a_process_is_reported_until_it_is_signalled<P: UiHost, S: UiStage>(
    factory: impl Fn() -> (P, S),
) {
    let (host, stage) = factory();
    let pid = stage.live_process();
    let seen = host.process(pid).expect("a live process is reported");
    assert!(
        !seen.started.is_empty() && !seen.argv.is_empty(),
        "{seen:?}"
    );
    assert_eq!(host.process(pid), Some(seen), "the same each time");
    host.signal(pid).expect("signal");
    let mut gone = false;
    for _ in 0..600 {
        if stage.ended(pid) {
            gone = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(gone, "a signalled process still runs");
}

pub fn uh_c10_a_detached_child_reports_one_line_or_its_end_is_an_error<P: UiHost, S: UiStage>(
    factory: impl Fn() -> (P, S),
) {
    let (host, stage) = factory();
    let (exe, args) = stage.child(Child::Reports("{\"ready\":1}"));
    let done = host.detach(&exe, &args, 30_000).expect("detach");
    assert_eq!(done.said, b"{\"ready\":1}".to_vec());
    assert!(done.pid > 0);
    let (exe, args) = stage.child(Child::Exits);
    assert!(
        matches!(host.detach(&exe, &args, 30_000), Err(UiError::Exited(_))),
        "a child that ends without a word"
    );
    let (exe, args) = stage.child(Child::Silent);
    assert!(matches!(
        host.detach(&exe, &args, 500),
        Err(UiError::TimedOut)
    ));
    assert!(
        !args.iter().any(|a| a == REPORT_FLAG),
        "the host adds the flag"
    );
}

pub fn uh_c11_the_view_is_owned_by_one_host_at_a_time<P: UiHost, S: UiStage>(
    factory: impl Fn() -> (P, S),
) {
    let (host, stage) = factory();
    assert_eq!(host.own_view(), Ok(true), "nobody holds it");
    assert_eq!(
        host.own_view(),
        Ok(true),
        "taking it again in the same host"
    );
    drop(host);
    let (host, stage2) = factory();
    stage2.other_owner(true);
    assert_eq!(host.own_view(), Ok(false), "another holds it");
    stage2.other_owner(false);
    // A process forked elsewhere in this one may hold the descriptor until its exec closes it: allow a moment.
    let mut free = Ok(false);
    for _ in 0..100 {
        free = host.own_view();
        if free == Ok(true) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(free, Ok(true), "free once the other lets go");
    drop(stage);
}

pub fn uh_c12_why_the_view_ends_reaches_the_server<P: UiHost, S: UiStage>(
    factory: impl Fn() -> (P, S),
) {
    let (host, stage) = factory();
    let server = host
        .serve(&stage.launch(Server::Ready, None))
        .expect("serve");
    host.ending(&server, UiEnd::Restarted).expect("ending");
    let mut seen = None;
    for _ in 0..200 {
        seen = stage.ending_seen();
        if seen.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert_eq!(seen.as_deref(), Some("restarted"));
    assert!(host.running(&server), "telling it is not stopping it");
    host.stop(&server);
    assert!(settle(&stage));
    assert!(
        host.ending(&server, UiEnd::Unreachable).is_err(),
        "a stopped server is told nothing"
    );
}

/// One `#[test]` per UiHost clause plus a test that the list is exactly `CLAUSES`. Usage:
/// `ui_host_contract!(factory; uh_c1_<what>(), ...)` with `factory: Fn() -> (impl UiHost, impl UiStage)`, each clause
/// written as a call so umx-arch's r-fake-per-port token scan sees it.
#[macro_export]
macro_rules! ui_host_contract {
    ($factory:expr; $($clause:ident()),+ $(,)?) => {
        mod ui_host_contract {
            #[allow(unused_imports)]
            use super::*;
            $(
                #[test]
                fn $clause() {
                    $crate::ui_host::$clause($factory)
                }
            )+
            #[test]
            fn every_clause_is_listed() {
                $crate::assert_listed($crate::ui_host::CLAUSES, &[$(stringify!($clause)),+]);
            }
        }
    };
}

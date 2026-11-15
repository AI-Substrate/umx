//! The req-39 agent walkthrough, run against the stub build with the network denied by the OS (req-51).
//! Run it with `cargo test -p umx --features stub --test walkthrough` (`just walkthrough`).
#![cfg(feature = "stub")]

use std::io::{BufRead, BufReader};
use std::net::UdpSocket;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use umx_protocol::{Envelope, Status};

const UMX: &str = env!("CARGO_BIN_EXE_umx");

/// macOS: everything but Unix-domain sockets is denied.
#[cfg(target_os = "macos")]
const SANDBOX: &str =
    "(version 1)(allow default)(deny network*)(allow network* (local unix) (remote unix))";

/// `program args` with no network: sandbox-exec on macOS, a fresh network namespace on Linux.
fn offline(program: &str, args: &[&str]) -> Command {
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = Command::new("sandbox-exec");
        c.args(["-p", SANDBOX, program]);
        c
    };
    #[cfg(target_os = "linux")]
    let mut cmd = {
        let mut c = Command::new("unshare");
        c.args(["--net", "--map-root-user", "--", program]);
        c
    };
    cmd.args(args);
    cmd
}

/// Run only as a child by `the_wrapper_denies_network`: panics unless the process can reach a network. A UDP
/// connect to TEST-NET-1 sends nothing but needs a route; the sandbox refuses it and an empty network
/// namespace has no route. (A loopback bind is not a test: an empty Linux namespace still allows it.)
#[test]
#[ignore = "probe for the network wrapper's positive control"]
fn network_probe() {
    let socket = UdpSocket::bind("0.0.0.0:0").expect("udp socket");
    socket
        .connect("192.0.2.1:9")
        .expect("a route to the network");
}

fn probe(cmd: &mut Command) -> bool {
    cmd.args(["--exact", "network_probe", "--ignored", "--test-threads=1"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
fn the_wrapper_denies_network() {
    let me = std::env::current_exe().unwrap();
    let me = me.to_str().unwrap();
    assert!(
        probe(&mut Command::new(me)),
        "positive control: without the wrapper this machine must have a network route"
    );
    let wrapped = offline("true", &[]).status();
    assert!(
        wrapped.is_ok_and(|s| s.success()),
        "the no-network wrapper is unavailable; on Linux allow unprivileged user namespaces \
         (sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0)"
    );
    assert!(
        !probe(&mut offline(me, &[])),
        "inside the wrapper there must be no network route"
    );
}

struct Agent {
    home: PathBuf,
    daemon: Option<Child>,
}

impl Agent {
    fn new() -> Agent {
        let home = PathBuf::from(format!("/tmp/umx-w-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        Agent { home, daemon: None }
    }

    /// One printed line, run as a shell would, with the network denied. `umx` is the stub build.
    fn shell(&self, line: &str) -> Command {
        let script = format!("umx() {{ \"$UMX_STUB_BIN\" \"$@\"; }}\n{line}");
        let mut cmd = offline("sh", &["-c", &script]);
        cmd.env("UMX_HOME", &self.home)
            .env("UMX_STUB_BIN", UMX)
            .env_remove("UMX_OUTPUT")
            .env_remove("UMX_GRAPH");
        cmd
    }

    fn run(&mut self, line: &str) -> Envelope {
        if line.starts_with("umx daemon start") {
            let mut child = self.shell(line).stdout(Stdio::piped()).spawn().unwrap();
            let mut ready = String::new();
            BufReader::new(child.stdout.take().unwrap())
                .read_line(&mut ready)
                .unwrap();
            self.daemon = Some(child);
            return Envelope::from_json(ready.as_bytes())
                .unwrap_or_else(|e| panic!("{line}: {e}: {ready:?}"));
        }
        let out = self.shell(line).output().unwrap();
        let env = Envelope::from_json(&out.stdout)
            .unwrap_or_else(|e| panic!("{line}: {e}: {}", String::from_utf8_lossy(&out.stdout)));
        assert_eq!(out.status.code(), Some(0), "{line}: {}", env.to_json());
        env
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.shell("umx daemon stop").output();
        if let Some(mut child) = self.daemon.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

/// The indented `umx ...` lines under `## First commands`, comments and all, as printed.
fn first_commands(markdown: &str) -> Vec<String> {
    markdown
        .split("## First commands")
        .nth(1)
        .expect("agents-start-here has a First commands section")
        .split("\n## ")
        .next()
        .unwrap()
        .lines()
        .filter_map(|l| l.strip_prefix("    "))
        .map(str::trim)
        .filter(|l| l.starts_with("umx "))
        .map(str::to_owned)
        .collect()
}

#[test]
fn an_agent_seeds_a_graph_using_only_what_umx_printed() {
    let mut agent = Agent::new();
    let start = agent.run("umx agents-start-here");
    let lines = first_commands(start.data["markdown"].as_str().unwrap());
    assert!(lines.len() >= 5, "{lines:?}");

    let mut ids: Vec<String> = Vec::new();
    let mut graph_path = None;
    let mut edges = None;
    for printed in &lines {
        let line = printed
            .replace("<from id>", ids.first().map_or("<from id>", String::as_str))
            .replace("<to id>", ids.get(1).map_or("<to id>", String::as_str));
        assert!(
            !line.contains('<'),
            "{printed}: needs an id no earlier command printed"
        );
        let env = agent.run(&line);
        assert!(
            matches!(env.status, Status::Ok | Status::Warning),
            "{line}: {}",
            env.to_json()
        );
        match env.command.as_str() {
            "graph new" => graph_path = env.data["graph"]["path"].as_str().map(PathBuf::from),
            "node add" => ids.push(env.data["node"]["id"].as_str().unwrap().to_owned()),
            "edge list" => edges = Some(env.data["rows"].clone()),
            _ => {}
        }
    }

    assert_eq!(ids.len(), 2, "two nodes added: {lines:?}");
    let edges = edges.expect("the printed commands list edges");
    assert_eq!(edges.as_array().map(Vec::len), Some(1), "{edges}");
    assert_eq!(
        (&edges[0]["from"], &edges[0]["to"]),
        (&ids[0].clone().into(), &ids[1].clone().into())
    );

    let status = agent.run("umx daemon status");
    assert_eq!(status.data["mode"], "stub");
    assert_eq!(
        status.data["adapters"],
        umx_protocol::json!({
            "clock": "fake.clock",
            "config_files": "fake.config-files",
            "embedder": "fake.embedder",
            "graph_files": "fake.graph-files",
            "id_generator": "fake.id-generator",
            "local_state": "fake.local-state",
            "model_fetcher": "fake.model-fetcher",
            "secret_allows": "fake.local-state",
            "lint_terms": "fake.local-state",
            "scopes": "fake.local-state",
            "registry": "fake.local-state",
            "secrets": "fake.secrets",
            "service_manager": "fake.service-manager",
            "source_reader": "fake.source-reader",
            "vector_cache": "fake.graph-files",
        })
    );
    // The stub embeds with the fake, with the network denied: the probe never leaves the machine.
    let probe = agent.run("umx embeddings probe");
    assert_eq!(
        (
            &probe.data["adapter"],
            &probe.data["dims"],
            &probe.data["locality"]
        ),
        (&"fake.embedder".into(), &64.into(), &"local".into())
    );
    let path = graph_path.expect("graph new reports its path");
    assert!(
        !path.exists(),
        "{} exists on disk: stub mode must not write graph files",
        path.display()
    );
}

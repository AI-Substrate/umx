//! F207 (ws004 s35): `umx daemon stop` with a local llama.cpp model loaded exits 0. Exit runs ggml's static Metal
//! teardown, which aborted (GGML_ASSERT in ggml_metal_rsets_free, exit 134) while a model was still loaded. The daemon
//! now applies every queued write and stops its writer, then releases each local model (sessions joined, contexts then
//! model freed), then exits. Runs where the embeddinggemma-2 file is in the models cache (fetch it with
//! `umx models fetch embeddinggemma-2`); elsewhere, CI included, it is skipped with a message (the file is 558 MB).

use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const KEY: &str = "embeddinggemma-2@256+p1";

/// The shared models cache, ~/.cache. Tests read no env (d-9): home_dir is only how this test finds the cache, and the
/// daemon is pointed at the same one with XDG_CACHE_HOME.
fn cache() -> PathBuf {
    std::env::home_dir().unwrap_or_default().join(".cache")
}

/// Whether the cache holds the model's BF16 file.
fn model_present() -> bool {
    let dir = cache().join("umx/models/ggml-org--embeddinggemma-2-GGUF");
    let found = fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .any(|commit| commit.path().join("embeddinggemma-2-BF16.gguf").is_file());
    if !found {
        eprintln!(
            "skipped: no embeddinggemma-2 file under {} (fetch it with umx models fetch embeddinggemma-2)",
            dir.display()
        );
    }
    found
}

/// A daemon in the foreground, so its exit status is the test's to read; killed if the test fails first.
struct Daemon(Option<Child>);

impl Daemon {
    fn wait(mut self) -> ExitStatus {
        let mut child = self.0.take().unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                return status;
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                panic!("the daemon did not exit within 60 s of its stop");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn umx(home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_umx"));
    cmd.env("UMX_HOME", home)
        .env("UMX_ROOTS", home)
        .env("XDG_CACHE_HOME", cache())
        .env_remove("UMX_OUTPUT")
        .env_remove("UMX_GRAPH");
    cmd
}

fn run(home: &Path, args: &[&str]) -> String {
    let out = umx(home).args(args).arg("--json").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "umx {args:?}: {stdout}");
    stdout
}

/// Starts a daemon whose stderr goes to `log`, once it has said it is ready.
fn start(home: &Path, log: &Path) -> Daemon {
    let mut child = umx(home)
        .args(["daemon", "start", "--json"])
        .stdout(Stdio::piped())
        .stderr(File::create(log).unwrap())
        .spawn()
        .unwrap();
    let mut ready = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert!(ready.contains("\"ok\""), "daemon start: {ready}");
    Daemon(Some(child))
}

/// Stops `daemon` through the CLI and asserts it exited 0 with no ggml assert in its log.
fn stop_cleanly(home: &Path, daemon: Daemon, log: &Path, when: &str) {
    run(home, &["daemon", "stop"]);
    let status = daemon.wait();
    let stderr = fs::read_to_string(log).unwrap_or_default();
    assert!(
        status.code() == Some(0) && !stderr.contains("GGML_ASSERT"),
        "{when}: the daemon exited {status}; its stderr:\n{stderr}"
    );
}

fn add(home: &Path, from: usize, to: usize) {
    for i in from..to {
        let title = format!("synthetic note {i} about pears and ladders");
        run(
            home,
            &[
                "node", "add", "--graph", "g", "--type", "note", "--title", &title,
            ],
        );
    }
}

fn content_nodes(graph: &Path) -> usize {
    fs::read_to_string(graph.join("nodes.jsonl"))
        .unwrap()
        .lines()
        .filter(|l| l.contains("pears and ladders"))
        .count()
}

#[test]
fn a_stop_with_a_local_model_loaded_applies_its_writes_then_exits_cleanly() {
    if !model_present() {
        return;
    }
    let home = PathBuf::from(format!("/tmp/umx-ls-{}", std::process::id()));
    let _ = fs::remove_dir_all(&home);
    fs::create_dir_all(&home).unwrap();
    let log = home.join("daemon.err");
    let graph = home.join("g");
    let graph_arg = graph.to_str().unwrap();

    // `config set` goes through a daemon; the instance is built when the next one starts.
    let daemon = start(&home, &log);
    for kv in [
        ["embedders.gemma.kind", "local"],
        ["embedders.gemma.model", "embeddinggemma-2"],
        ["embedders.gemma.dims", "256"],
        ["embedder.active", "gemma"],
    ] {
        run(&home, &["config", "set", kv[0], kv[1]]);
    }
    stop_cleanly(&home, daemon, &log, "a stop with no model loaded");

    let daemon = start(&home, &log);
    run(
        &home,
        &[
            "graph",
            "new",
            graph_arg,
            "--alias",
            "g",
            "--embedding-model",
            KEY,
        ],
    );
    add(&home, 0, 8);
    let deadline = Instant::now() + Duration::from_secs(120);
    while !run(&home, &["graph", "list"]).contains("\"vectors_pending\":0") {
        assert!(
            Instant::now() < deadline,
            "8 nodes not embedded within 120 s"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    // More writes, and the stop at once: their embeds may still be running when it arrives.
    add(&home, 8, 12);
    stop_cleanly(&home, daemon, &log, "a stop with the model loaded");
    // Every write was applied before the model was released and the process exited.
    assert_eq!(content_nodes(&graph), 12);
    assert!(
        fs::read_dir(graph.join("embeddings"))
            .map(|d| d.count() > 0)
            .unwrap_or(false),
        "no vector shards were written"
    );

    // The files load in a fresh daemon (which warms the model again), and that one stops cleanly too.
    let daemon = start(&home, &log);
    run(&home, &["graph", "load", graph_arg]);
    let listed = run(&home, &["graph", "list"]);
    assert!(listed.contains("\"nodes\":12"), "{listed}");
    stop_cleanly(&home, daemon, &log, "a stop after a reload");
    let _ = fs::remove_dir_all(&home);
}

//! req-52: four CLI processes add nodes to one graph at once; nothing is lost and a reload rewrites the same
//! content bytes. It checks exit codes and the graph's files, not the envelope. The default run is 4 x 25; the
//! full 4 x 250 is ignored locally (fsync-heavy) and runs in CI: `cargo test -p umx --test writers -- --ignored`.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

const WRITERS: usize = 4;

fn content_lines(bytes: &[u8]) -> Vec<&[u8]> {
    bytes
        .split_inclusive(|b| *b == b'\n')
        .filter(|line| {
            let decoded = umx_core::codec::decode_nodes(line).unwrap();
            !umx_core::provenance::is_provenance(&decoded.records[0])
        })
        .collect()
}

struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A short UMX_HOME (the socket path limit is 104 bytes) that is also the only graph root.
fn home() -> PathBuf {
    let path = PathBuf::from(format!("/tmp/umx-w-{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

fn umx(home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_umx"));
    cmd.env("UMX_HOME", home)
        .env("UMX_ROOTS", home)
        .env_remove("UMX_OUTPUT");
    cmd
}

fn run(home: &Path, args: &[&str]) {
    let out = umx(home).args(args).arg("--json").output().unwrap();
    assert!(
        out.status.success(),
        "umx {args:?}: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

fn start(home: &Path) -> Daemon {
    let mut child = umx(home)
        .args(["daemon", "start", "--json"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert!(ready.contains("\"ok\""), "daemon start: {ready}");
    Daemon(child)
}

#[test]
fn four_cli_writers_lose_nothing_and_a_reload_rewrites_the_same_bytes() {
    writers_lose_nothing(25);
}

#[test]
#[ignore = "4 x 250 writes, run in CI (req-52); locally: cargo test -p umx --test writers -- --ignored"]
fn four_cli_writers_lose_nothing_at_full_size() {
    writers_lose_nothing(250);
}

fn writers_lose_nothing(each: usize) {
    let home = home();
    let graph = home.join("g");
    let graph_arg = graph.to_str().unwrap();
    let daemon = start(&home);
    run(&home, &["graph", "new", graph_arg, "--alias", "demo"]);

    let writers: Vec<_> = (0..WRITERS)
        .map(|w| {
            let home = home.clone();
            std::thread::spawn(move || {
                for i in 0..each {
                    let title = format!("writer {w} node {i}");
                    run(
                        &home,
                        &[
                            "node",
                            "add",
                            "--type",
                            "problem",
                            "--title",
                            &title,
                            "--by",
                            &format!("w{w}"),
                        ],
                    );
                }
            })
        })
        .collect();
    for w in writers {
        w.join().unwrap();
    }
    let nodes = fs::read(graph.join("nodes.jsonl")).unwrap();
    let content = content_lines(&nodes);
    assert_eq!(content.len(), WRITERS * each);

    // A fresh daemon rebuilds the graph from the files; its next write rewrites every existing line unchanged.
    drop(daemon);
    let _daemon = start(&home);
    run(&home, &["graph", "load", graph_arg]);
    run(
        &home,
        &[
            "node",
            "add",
            "--type",
            "problem",
            "--title",
            "after reload",
        ],
    );
    let after = fs::read(graph.join("nodes.jsonl")).unwrap();
    let after_content = content_lines(&after);
    assert!(
        after_content.starts_with(&content),
        "existing records were rewritten differently"
    );
    assert_eq!(after_content.len(), WRITERS * each + 1);
    let _ = fs::remove_dir_all(&home);
}

//! ws002 F72 (architect): a starting daemon reloads every graph this machine's registry remembers before it answers.
//! Measured through the release binary: `umx daemon start` to its ready line, and an in-place `umx daemon restart`,
//! first with nothing remembered, then with `graphs` copies of the fixture loaded. The reload's cost is the
//! difference; the CLI's restart waits at most 10 s for the new daemon to answer.

use super::{Args, Daemon, cli, ms, umx};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::Stdio;
use std::time::Instant;

/// Starts a daemon; returns it and the wall time to its ready line.
fn start(a: &Args, home: &Path, work: &Path) -> Result<(Daemon, f64), String> {
    let t = Instant::now();
    let mut child = umx(a, home, work)
        .args(["daemon", "start", "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("start daemon: {e}"))?;
    let mut ready = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut ready)
        .map_err(|e| e.to_string())?;
    let took = ms(t.elapsed());
    if !ready.contains("\"pid\"") {
        return Err(format!("daemon start: {ready}"));
    }
    Ok((Daemon(child), took))
}

fn stop(a: &Args, home: &Path, work: &Path, daemon: Daemon) -> Result<(), String> {
    cli(a, home, work, &["daemon", "stop"])?;
    let mut daemon = daemon;
    let _ = daemon.0.wait();
    Ok(())
}

/// Start and restart times with nothing remembered, then with `folders` loaded.
pub fn measure(a: &Args, work: &Path, folders: &[String]) -> Result<Value, String> {
    let home = work.join("h");
    let (daemon, start_empty) = start(a, &home, work)?;
    let (restart_empty, _) = cli(a, &home, work, &["daemon", "restart"])?;
    for (i, folder) in folders.iter().enumerate() {
        cli(
            a,
            &home,
            work,
            &["graph", "load", folder, "--as", &format!("g{i}")],
        )?;
    }
    let (restart, _) = cli(a, &home, work, &["daemon", "restart"])?;
    let (_, status) = cli(a, &home, work, &["daemon", "status"])?;
    let reloaded = status["data"]["registry"]["reloaded"]
        .as_array()
        .map_or(0, Vec::len);
    stop(a, &home, work, daemon)?;
    let (daemon, start) = start(a, &home, work)?;
    let (_, list) = cli(a, &home, work, &["graph", "list"])?;
    let listed = list["data"]["total"].as_u64().unwrap_or(0);
    stop(a, &home, work, daemon)?;
    if reloaded != folders.len() || listed != folders.len() as u64 {
        return Err(format!(
            "reloaded {reloaded} and listed {listed} of {} graphs",
            folders.len()
        ));
    }
    Ok(json!({
        "graphs": folders.len(),
        "start_ms": { "empty": start_empty, "remembered": start },
        "restart_ms": { "empty": restart_empty, "remembered": restart },
    }))
}

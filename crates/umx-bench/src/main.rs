//! umx-bench: the benchmark root (blueprint 001, c-bench; PRD req-43). Generates a seeded synthetic graph,
//! measures it in process through umx-compose's real adapters and through the release `umx` binary with a
//! private daemon, and writes a dated result file. Reports, never gates (D8). Run it with `just bench`.
//! The storage keys (walk, path, within, memory per node) are measured on a graph built straight through the
//! codec and umx-graph (workstream 003 s01), and so are the Cypher rows (workstream 005 s02); `--rich` switches
//! to the rich fixture. `--ruler` runs only the ruler (workstream 003 s14): the product's own rows on a realistic
//! fixture, its keyed variant and the rich fixture (see ruler.rs).

mod alloc;
mod batch;
mod cross_reads;
mod cypher;
mod external;
mod fixture;
mod provenance;
mod real;
mod reload;
mod report;
mod ruler;
mod storage;
mod vectors;
mod writes;

use serde_json::{Value, json};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::time::{Duration, Instant};
use umx_compose::{Config, Mode, compose};
use umx_services::{Ctx, NodeListRequest};

#[global_allocator]
static ALLOC: alloc::Counting = alloc::Counting;

/// The largest graph anyone runs on the shared machine.
const MAX_NODES: usize = 100_000;
/// The fewest nodes `--ruler` runs on (plan 196). Below it the write rows' picked node pairs crowd together and are
/// often already linked, so a row could not add its edge; from it up the ruler runs (a row whose pick list is empty,
/// such as the typo queries below about 250 nodes, is skipped and named).
const MIN_RULER_NODES: usize = 100;

struct Args {
    nodes: usize,
    edges_per_node: usize,
    seed: u64,
    runs: usize,
    adds: usize,
    umx: PathBuf,
    out: PathBuf,
    commit: String,
    conditions: String,
    rich: bool,
    cli: bool,
    vectors: usize,
    external_only: bool,
    batch_ids_only: bool,
    external_writes_only: bool,
    provenance_only: bool,
    provenance_sessions: usize,
    provenance_duplicate_lines: bool,
    cross_reads_only: bool,
    /// `--reload-only <graphs>`: only the start and restart times with that many fixture copies remembered (F72).
    reload_only: usize,
    /// `--ruler`: only the ruler rows (ws003 s14), written to `--out`.
    ruler: bool,
    /// `--redact-only`: only the ruler's evidence redact row (F197), on the realistic fixture; printed, not written.
    redact_only: bool,
    /// `--along-only`: only the ruler's `--along` rows (D71 step 12), on the realistic fixture; printed, not written.
    along_only: bool,
    /// `--strength-only`: only the ruler's search strength rows (PRD 002 search-10), on the realistic fixture; printed,
    /// not written.
    strength_only: bool,
}

const USAGE: &str = "umx-bench --umx <release umx binary> --out <dir> --commit <sha> [--conditions <text>] \
[--nodes 10000] [--edges-per-node 3] [--seed 1] [--runs 5] [--adds 50] [--vectors 4] [--rich] [--no-cli] [--external-ends-only] [--batch-ids-only] [--external-writes-only] [--cross-reads-only] [--reload-only <graphs>] [--provenance-only] [--provenance-sessions 10000] [--provenance-duplicate-lines] [--ruler] [--redact-only] [--along-only] [--strength-only]";

fn args() -> Result<Args, String> {
    parse(std::env::args().skip(1))
}

fn parse(mut it: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut a = Args {
        nodes: 10_000,
        edges_per_node: 3,
        seed: 1,
        runs: 5,
        adds: 50,
        umx: PathBuf::new(),
        out: PathBuf::new(),
        commit: "unknown".into(),
        conditions: String::new(),
        rich: false,
        cli: true,
        vectors: 4,
        external_only: false,
        batch_ids_only: false,
        external_writes_only: false,
        provenance_only: false,
        provenance_sessions: 0,
        provenance_duplicate_lines: false,
        cross_reads_only: false,
        reload_only: 0,
        ruler: false,
        redact_only: false,
        along_only: false,
        strength_only: false,
    };
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--cross-reads-only" => {
                a.cross_reads_only = true;
                continue;
            }
            "--ruler" => {
                a.ruler = true;
                continue;
            }
            "--redact-only" => {
                a.ruler = true;
                a.redact_only = true;
                continue;
            }
            "--along-only" => {
                a.ruler = true;
                a.along_only = true;
                continue;
            }
            "--strength-only" => {
                a.ruler = true;
                a.strength_only = true;
                continue;
            }
            "--rich" => {
                a.rich = true;
                continue;
            }
            "--no-cli" => {
                a.cli = false;
                continue;
            }
            "--external-ends-only" => {
                a.external_only = true;
                continue;
            }
            "--batch-ids-only" => {
                a.batch_ids_only = true;
                continue;
            }
            "--external-writes-only" => {
                a.external_writes_only = true;
                continue;
            }
            "--provenance-only" => {
                a.provenance_only = true;
                continue;
            }
            "--provenance-duplicate-lines" => {
                a.provenance_duplicate_lines = true;
                continue;
            }
            _ => {}
        }
        let value = it
            .next()
            .ok_or(format!("{flag} needs a value. Usage: {USAGE}"))?;
        let number = || {
            value
                .parse::<usize>()
                .map_err(|_| format!("{flag} takes a number"))
        };
        match flag.as_str() {
            "--nodes" => a.nodes = number()?,
            "--edges-per-node" => a.edges_per_node = number()?,
            "--seed" => a.seed = number()? as u64,
            "--runs" => a.runs = number()?.max(1),
            "--adds" => a.adds = number()?.max(1),
            "--vectors" => a.vectors = number()?,
            "--umx" => a.umx = PathBuf::from(&value),
            "--out" => a.out = PathBuf::from(&value),
            "--commit" => a.commit = value,
            "--conditions" => a.conditions = value,
            "--reload-only" => a.reload_only = number()?,
            "--provenance-sessions" => a.provenance_sessions = number()?,
            _ => return Err(format!("unknown flag {flag}. Usage: {USAGE}")),
        }
    }
    if (!a.external_only
        && !a.batch_ids_only
        && !a.external_writes_only
        && !a.cross_reads_only
        && !a.ruler
        && a.umx.as_os_str().is_empty())
        || a.out.as_os_str().is_empty()
    {
        return Err(format!("--umx and --out are required. Usage: {USAGE}"));
    }
    if a.nodes > MAX_NODES {
        return Err(format!("--nodes is at most {MAX_NODES} on this machine"));
    }
    if a.ruler && a.nodes < MIN_RULER_NODES {
        return Err(format!(
            "--ruler needs --nodes {MIN_RULER_NODES} or more (got {}): on fewer, its picked node pairs crowd together",
            a.nodes
        ));
    }
    if a.provenance_sessions > 10_000 || (a.provenance_only && (!a.rich || a.nodes == 0)) {
        return Err(
            "provenance measurements require --rich, nonzero nodes and at most 10000 sessions"
                .into(),
        );
    }
    if a.provenance_sessions > 0 && !a.provenance_only {
        return Err("--provenance-sessions requires --provenance-only".into());
    }
    if a.provenance_duplicate_lines && (!a.provenance_only || a.provenance_sessions == 0) {
        return Err(
            "duplicate stamp lines require --provenance-only and nonzero --provenance-sessions"
                .into(),
        );
    }
    Ok(a)
}

fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 1e6).round() / 1e3
}

/// Median and minimum of `runs` timings of `f`, in milliseconds.
fn timed(runs: usize, mut f: impl FnMut()) -> Value {
    let mut all: Vec<f64> = (0..runs)
        .map(|_| {
            let t = Instant::now();
            f();
            ms(t.elapsed())
        })
        .collect();
    let raw = all.clone();
    all.sort_by(f64::total_cmp);
    json!({ "median": all[all.len() / 2], "min": all[0], "runs": runs, "samples": raw })
}

/// Load, list and find through the composed services (real adapters).
fn composed(a: &Args, work: &Path, graph: &str) -> Value {
    let config = Config::at(work.join("in-process-home"), None).roots([work.to_str().unwrap()]);
    let mut svc = compose(Mode::Real, &config);
    let ctx = Ctx::new("umx-bench");
    let load = timed(a.runs, || {
        svc.graph_load(&ctx, graph, None).expect("fixture loads");
    });
    let all = NodeListRequest {
        limit: usize::MAX,
        ..Default::default()
    };
    let list_all = timed(a.runs, || {
        svc.node_list(&ctx, all.clone()).expect("list");
    });
    let filtered = NodeListRequest {
        kind: Some("problem".into()),
        wheres: vec!["likelihood>=3".into()],
        ..Default::default()
    };
    let list_filtered = timed(a.runs, || {
        svc.node_list(&ctx, filtered.clone()).expect("list");
    });
    let find = timed(a.runs, || {
        svc.node_find(&ctx, "approval delays", None, None, 10)
            .expect("find");
    });
    // umx search with no embedder composed: the exact and word legs, the scan every search pays (ws004 s11). The
    // vector leg's scan is s13's `vector_scan` keys.
    let search = timed(a.runs, || {
        svc.search(&ctx, "approval delays", None, 10)
            .expect("search");
    });
    // umx query through services (ws005 s04, architect): scope, permits and the clock hook on top of `run`. q5 is
    // the slowest accepted query of the Cypher rows (cypher::measure times it at the API).
    let q5 = umx_services::CypherRequest {
        text: "MATCH (n) WHERE size(n.evidence_kinds) > 0 AND NOT 'client' IN n.evidence_kinds AND NOT \
               'transcript' IN n.evidence_kinds RETURN n.id AS id ORDER BY id"
            .into(),
        limit: None,
        offset: None,
        params: Vec::new(),
        along: None,
    };
    let query = timed(a.runs, || {
        svc.query(&ctx, &q5).expect("q5 runs");
    });
    json!({
        "load_ms": load, "list_all_ms": list_all, "list_filtered_ms": list_filtered, "find_ms": find,
        "search_ms": search, "query_q5_ms": query,
    })
}

fn in_process(a: &Args, work: &Path, graph: &str, fx: &fixture::Fixture) -> Value {
    // The composed graph is dropped before the storage keys run, so it is not in their memory figures.
    let mut v = composed(a, work, graph);
    let keys = storage::measure(a.runs, a.seed, fx);
    // Its own graph, built after the storage keys' is dropped, so neither counts the other.
    v["cypher"] = cypher::measure(a.runs, fx);
    // After the storage keys, so their memory figures are the graph without vectors.
    if a.vectors > 0 {
        v["vectors"] = vectors::measure(a.runs, a.seed, fx, a.vectors);
        eprintln!("umx-bench: vectors done");
    }
    let (Value::Object(into), Value::Object(from)) = (&mut v, keys) else {
        unreachable!("both are objects")
    };
    into.extend(from);
    v
}

struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn umx(a: &Args, home: &Path, work: &Path) -> Command {
    let mut cmd = Command::new(&a.umx);
    cmd.env("UMX_HOME", home)
        .env("UMX_ROOTS", work)
        .env_remove("UMX_GRAPH")
        .env_remove("UMX_OUTPUT");
    cmd
}

/// Runs one CLI command; returns its wall time and parsed envelope.
fn cli(a: &Args, home: &Path, work: &Path, args: &[&str]) -> Result<(f64, Value), String> {
    let t = Instant::now();
    let out = umx(a, home, work)
        .args(args)
        .arg("--json")
        .output()
        .map_err(|e| format!("run umx: {e}"))?;
    let took = ms(t.elapsed());
    let env: Value = serde_json::from_slice(&out.stdout)
        .map_err(|_| format!("umx {args:?}: {}", String::from_utf8_lossy(&out.stdout)))?;
    if env["status"] == "error" {
        return Err(format!("umx {args:?}: {env}"));
    }
    Ok((took, env))
}

fn rss_kb(pid: u64) -> Option<u64> {
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn through_cli(a: &Args, work: &Path, graph: &str, parent: &str) -> Result<Value, String> {
    let home = work.join("h");
    let mut child = umx(a, &home, work)
        .args(["daemon", "start", "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("start daemon: {e}"))?;
    let mut ready = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut ready)
        .map_err(|e| e.to_string())?;
    let daemon = Daemon(child);
    let ready: Value =
        serde_json::from_str(&ready).map_err(|_| format!("daemon start: {ready}"))?;
    let pid = ready["data"]["pid"]
        .as_u64()
        .ok_or(format!("daemon start: {ready}"))?;
    let idle = rss_kb(pid);
    let (load, _) = cli(a, &home, work, &["graph", "load", graph])?;
    let loaded = rss_kb(pid);
    let actor_warmup_ms = if a.provenance_only {
        Some(
            cli(
                a,
                &home,
                work,
                &[
                    "node",
                    "add",
                    "--type",
                    "problem",
                    "--title",
                    "Steady CLI warmup",
                ],
            )?
            .0,
        )
    } else {
        None
    };
    let mut list = Vec::new();
    let mut find = Vec::new();
    for _ in 0..a.runs {
        list.push(cli(a, &home, work, &["node", "list", "--limit", "100"])?.0);
        find.push(cli(a, &home, work, &["node", "find", "approval", "delays"])?.0);
    }
    let mut adds = Vec::new();
    for i in 0..a.adds {
        let title = format!("Bench addition {i}");
        adds.push(
            cli(
                a,
                &home,
                work,
                &["node", "add", "--type", "problem", "--title", &title],
            )?
            .0,
        );
    }
    let after = rss_kb(pid);
    // Before the parent adds, so this figure stays comparable with results made before they existed.
    let nodes_after = std::fs::metadata(Path::new(graph).join("nodes.jsonl"))
        .map(|m| m.len())
        .ok();
    let parent_adds = writes::cli_parent_adds(a.adds, parent, |args| {
        cli(a, &home, work, args).map(|r| r.0)
    })?;
    let _ = cli(a, &home, work, &["daemon", "stop"]);
    drop(daemon);
    let raw_adds = adds.clone();
    for v in [&mut list, &mut find, &mut adds] {
        v.sort_by(f64::total_cmp);
    }
    Ok(json!({
        "load_ms": load,
        "list_100_ms": { "median": percentile(&list, 0.5), "min": list[0], "runs": a.runs },
        "find_ms": { "median": percentile(&find, 0.5), "min": find[0], "runs": a.runs },
        "add_ms": { "p50": percentile(&adds, 0.5), "p95": percentile(&adds, 0.95), "max": adds[adds.len() - 1], "n": a.adds, "samples": raw_adds },
        "add_parent_ms": parent_adds,
        "rss_kb": { "idle": idle, "loaded": loaded, "after_adds": after },
        "nodes_bytes_after_adds": nodes_after,
        "unmeasured_actor_warmup_writes": usize::from(a.provenance_only),
        "unmeasured_actor_warmup_ms": actor_warmup_ms,
    }))
}

/// A new private folder under the per-user temp folder ($TMPDIR), made with `create_dir` so a folder already
/// there (or a symlink planted at the name) fails instead of being used. Short: the daemon socket path inside it
/// must stay under 104 bytes.
fn work_dir() -> Result<PathBuf, String> {
    use std::os::unix::fs::DirBuilderExt;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let work = std::env::temp_dir().join(format!(
        "umx-bench-{}-{:05x}",
        std::process::id(),
        nanos & 0xf_ffff
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&work)
        .map_err(|e| format!("make {}: {e}", work.display()))?;
    let socket = work.join("h/run/umx.sock");
    if socket.as_os_str().len() >= 104 {
        let _ = std::fs::remove_dir(&work);
        return Err(format!(
            "{} is too long for the daemon socket; set TMPDIR to a shorter folder",
            socket.display()
        ));
    }
    Ok(work)
}

/// Removes the work folder however `run` ends, errors included.
struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(a: &Args) -> Result<PathBuf, String> {
    if a.provenance_only {
        return provenance::run(a);
    }
    if a.ruler {
        return ruler::run(a);
    }
    if a.external_only || a.external_writes_only || a.cross_reads_only {
        let fx = fixture::generate(a.seed, a.nodes, a.edges_per_node, a.rich);
        let measurements = if a.cross_reads_only {
            json!({"cross_graph_reads": cross_reads::measure(a.runs, &fx)?})
        } else if a.external_writes_only {
            json!({"external_writes": external::measure_services(a.runs, &fx)?})
        } else {
            json!({"external_ends": external::measure(a.runs, &fx)})
        };
        return report::write(
            &a.out,
            &json!({
                "umx_bench": 2,
                "date": umx_core::time::iso_from_ms(
                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                        .map_err(|e| format!("system clock: {e}"))?.as_millis() as u64),
                "commit": a.commit,
                "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
                "conditions": a.conditions,
                "machine": report::machine(),
                "fixture": {"kind": if a.rich { "rich" } else { "plain" },
                    "seed": a.seed, "nodes": fx.node_count, "edges": fx.edge_count},
                "in_process": measurements,
            }),
        );
    }
    let work = work_dir()?;
    let _cleanup = RemoveOnDrop(work.clone());
    if a.reload_only > 0 {
        let fx = fixture::generate(a.seed, a.nodes, a.edges_per_node, a.rich);
        let mut folders = Vec::new();
        for i in 0..a.reload_only {
            let dir = work.join(format!("r{i}"));
            std::fs::create_dir(&dir).map_err(|e| format!("make {}: {e}", dir.display()))?;
            for (name, bytes) in [
                ("graph.json", &fx.graph),
                ("nodes.jsonl", &fx.nodes),
                ("edges.jsonl", &fx.edges),
            ] {
                std::fs::write(dir.join(name), bytes).map_err(|e| format!("write {name}: {e}"))?;
            }
            folders.push(dir.to_str().ok_or("work path is not UTF-8")?.to_owned());
        }
        let reload = reload::measure(a, &work, &folders)?;
        return report::write(
            &a.out,
            &json!({
                "umx_bench": 2,
                "date": umx_core::time::iso_from_ms(
                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                        .map_err(|e| format!("system clock: {e}"))?.as_millis() as u64),
                "commit": a.commit,
                "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
                "conditions": a.conditions,
                "machine": report::machine(),
                "fixture": {"kind": if a.rich { "rich" } else { "plain" },
                    "seed": a.seed, "nodes": fx.node_count, "edges": fx.edge_count},
                "cli": {"reload": reload},
            }),
        );
    }
    let dir = work.join("g");
    std::fs::create_dir(&dir).map_err(|e| format!("make {}: {e}", dir.display()))?;
    let generated = Instant::now();
    let fx = fixture::generate(a.seed, a.nodes, a.edges_per_node, a.rich);
    for (name, bytes) in [
        ("graph.json", &fx.graph),
        ("nodes.jsonl", &fx.nodes),
        ("edges.jsonl", &fx.edges),
    ] {
        std::fs::write(dir.join(name), bytes).map_err(|e| format!("write {name}: {e}"))?;
    }
    let generate_ms = ms(generated.elapsed());
    if a.batch_ids_only {
        return report::write(
            &a.out,
            &json!({
                "umx_bench": 2,
                "date": umx_core::time::iso_from_ms(
                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                        .map_err(|e| format!("system clock: {e}"))?.as_millis() as u64),
                "commit": a.commit,
                "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
                "conditions": a.conditions,
                "machine": report::machine(),
                "fixture": {"kind": if a.rich { "rich" } else { "plain" },
                    "seed": a.seed, "nodes": fx.node_count, "edges": fx.edge_count},
                "in_process": {"batch_ids": batch::explicit_ids(&work, &dir, &fx, a.runs)?},
            }),
        );
    }
    let graph = dir.to_str().ok_or("work path is not UTF-8")?.to_owned();
    eprintln!(
        "umx-bench: {} nodes, {} edges in {}",
        fx.node_count, fx.edge_count, graph
    );
    let parent = writes::first_node_id(&fx.nodes)?;
    let mut in_process = in_process(a, &work, &graph, &fx);
    in_process["writes"] = writes::in_process(a.adds, &work, &dir, &parent)?;
    // ws003 s07 (req-88): batch rows, through the services and on the graph alone.
    in_process["batch"] = batch::services(&work, &dir, &parent)?;
    in_process["batch"]["graph"] = batch::graph(a.runs.min(3), &fx);
    eprintln!("umx-bench: batch done");
    eprintln!("umx-bench: in-process done");
    let cli = if a.cli {
        through_cli(a, &work, &graph, &parent)
    } else {
        Ok(Value::Null)
    };
    let _ = std::fs::remove_dir_all(&work);
    let cli = cli?;
    let result = json!({
        "umx_bench": 2,
        "date": umx_core::time::iso_from_ms(
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
        ),
        "commit": a.commit,
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "conditions": a.conditions,
        "machine": report::machine(),
        "fixture": {
            "kind": if a.rich { "rich" } else { "plain" },
            "seed": a.seed, "nodes": fx.node_count, "edges": fx.edge_count, "edges_per_node": a.edges_per_node,
            "vectors_per_node": a.vectors,
            "generate_ms": generate_ms,
            "bytes": { "graph": fx.graph.len(), "nodes": fx.nodes.len(), "edges": fx.edges.len() },
        },
        "in_process": in_process,
        "cli": cli,
    });
    report::write(&a.out, &result)
}

fn main() -> ExitCode {
    let code = match args().and_then(|a| run(&a)) {
        Ok(path) => {
            println!("{}", path.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("umx-bench: {e}");
            ExitCode::FAILURE
        }
    };
    // The one report of a helper left pending, as in the umx binary: tools print nothing themselves (plan 188 item 6).
    if !umx_compose::finish_client_tools(Instant::now() + Duration::from_secs(2)) {
        eprintln!(
            "umx-bench: identity helper cleanup is still waiting on the operating system; the results above stand"
        );
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ruler_args(nodes: usize, out: &Path) -> Result<Args, String> {
        let out = out.to_str().expect("UTF-8").to_owned();
        parse(
            ["--ruler", "--nodes", &nodes.to_string(), "--out", &out]
                .into_iter()
                .map(String::from),
        )
    }

    /// Plan 196: `--ruler` refuses fewer nodes than its picks need with an argument error, and from that size up it
    /// runs: every row it writes has samples, and a row left without any (the typo picks on a small fixture) is named
    /// in `skipped_rows` instead.
    #[test]
    fn the_ruler_refuses_too_few_nodes_and_runs_from_the_minimum() {
        let out = work_dir().unwrap();
        let _cleanup = RemoveOnDrop(out.clone());
        assert!(ruler_args(MIN_RULER_NODES - 1, &out).is_err());
        for nodes in [MIN_RULER_NODES, MIN_RULER_NODES + 1, 300] {
            let path = ruler::run(&ruler_args(nodes, &out).unwrap()).unwrap();
            let r: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            let rows = r["rows"].as_array().unwrap();
            let skipped = r["skipped_rows"].as_array().unwrap();
            for row in rows {
                assert!(
                    row["n"].as_u64() > Some(0),
                    "{nodes} nodes: {}",
                    row["name"]
                );
                assert!(
                    !skipped.contains(&row["name"]),
                    "{nodes} nodes: {}",
                    row["name"]
                );
            }
            assert!(
                rows.iter()
                    .any(|row| row["name"] == "real/edge_list_from_inbound_hub_ms"),
                "{nodes} nodes"
            );
        }
    }
}

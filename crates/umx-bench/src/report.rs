//! Result files: `<date>-<HHMMSS>-<commit>-<nodes>n.json`, `-rich` before `.json` for the rich fixture (plus
//! `-2`, `-3`, ... if that name is taken), and a short `.md` that compares with the latest earlier result for the
//! same fixture (kind, seed, nodes, edges per node). Format 1 results have no kind; they are plain.

use serde_json::{self, Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

pub fn machine() -> Value {
    let (cpu, memory) = if cfg!(target_os = "macos") {
        (
            run("sysctl", &["-n", "machdep.cpu.brand_string"]),
            run("sysctl", &["-n", "hw.memsize"]).and_then(|m| m.parse::<u64>().ok()),
        )
    } else {
        let read = |p: &str| std::fs::read_to_string(p).unwrap_or_default();
        let field = |text: &str, key: &str| {
            text.lines()
                .find(|l| l.starts_with(key))
                .and_then(|l| l.split(':').nth(1))
                .map(|v| v.trim().to_owned())
        };
        let mem = field(&read("/proc/meminfo"), "MemTotal")
            .and_then(|v| {
                v.split_whitespace()
                    .next()
                    .and_then(|k| k.parse::<u64>().ok())
            })
            .map(|kb| kb * 1024);
        (field(&read("/proc/cpuinfo"), "model name"), mem)
    };
    json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "cpu": cpu,
        "cores": std::thread::available_parallelism().map(|n| n.get()).ok(),
        "memory_bytes": memory,
    })
}

/// The metrics shown in the summary: label, JSON pointer, unit.
const METRICS: &[(&str, &str, &str)] = &[
    (
        "in-process graph load (median)",
        "/in_process/load_ms/median",
        "ms",
    ),
    (
        "in-process node list, all (median)",
        "/in_process/list_all_ms/median",
        "ms",
    ),
    (
        "in-process node list, filtered (median)",
        "/in_process/list_filtered_ms/median",
        "ms",
    ),
    (
        "in-process node find (median)",
        "/in_process/find_ms/median",
        "ms",
    ),
    ("CLI graph load", "/cli/load_ms", "ms"),
    (
        "CLI node list --limit 100 (median)",
        "/cli/list_100_ms/median",
        "ms",
    ),
    ("CLI node find (median)", "/cli/find_ms/median", "ms"),
    ("CLI node add p50", "/cli/add_ms/p50", "ms"),
    ("CLI node add p95", "/cli/add_ms/p95", "ms"),
    (
        "CLI node add --parent p50 (two-file set)",
        "/cli/add_parent_ms/p50",
        "ms",
    ),
    (
        "CLI node add --parent p95 (two-file set)",
        "/cli/add_parent_ms/p95",
        "ms",
    ),
    (
        "in-process node add p50 (one file)",
        "/in_process/writes/add_ms/p50",
        "ms",
    ),
    (
        "in-process node add --parent p50 (two-file set)",
        "/in_process/writes/add_parent_ms/p50",
        "ms",
    ),
    (
        "batch of 1,000 node adds, in process (one write)",
        "/in_process/batch/adds_ms",
        "ms",
    ),
    (
        "batch of 1,000 adds with parent edges, in process",
        "/in_process/batch/adds_parent_edges_ms",
        "ms",
    ),
    (
        "batch apply only: 1,000 adds + 1,000 edges (median)",
        "/in_process/batch/graph/apply_adds_edges_ms/median",
        "ms",
    ),
    (
        "batch apply only: 1,000 edits (median)",
        "/in_process/batch/graph/apply_edits_ms/median",
        "ms",
    ),
    (
        "batch rollback of 1,000 mixed ops (median)",
        "/in_process/batch/graph/rollback_ms/median",
        "ms",
    ),
    (
        "walk to depth 3, both directions (median)",
        "/in_process/walk_depth3_us/median",
        "us",
    ),
    (
        "nodes reached per walk (check)",
        "/in_process/walk_depth3_reached",
        "nodes",
    ),
    (
        "shortest path, per pair (median)",
        "/in_process/path_us/median",
        "us",
    ),
    ("path hop sum (check)", "/in_process/path_hops_sum", "hops"),
    (
        "within the root (median)",
        "/in_process/within_root_ms/median",
        "ms",
    ),
    (
        "within the root, count (check)",
        "/in_process/within_root_count",
        "nodes",
    ),
    (
        "within a node 1% in (median)",
        "/in_process/within_1pct_ms/median",
        "ms",
    ),
    (
        "within a node 1% in, count (check)",
        "/in_process/within_1pct_count",
        "nodes",
    ),
    (
        "get title by id (median)",
        "/in_process/get_title_ns/median",
        "ns",
    ),
    (
        "get whole node by id (median)",
        "/in_process/get_node_ns/median",
        "ns",
    ),
    ("graph memory", "/in_process/mem_graph_bytes", "bytes"),
    (
        "graph memory per node, edges included",
        "/in_process/mem_per_node_bytes",
        "bytes",
    ),
    ("load peak memory", "/in_process/load_peak_bytes", "bytes"),
    (
        "encode nodes (median)",
        "/in_process/encode_nodes_ms/median",
        "ms",
    ),
    (
        "encode edges (median)",
        "/in_process/encode_edges_ms/median",
        "ms",
    ),
    (
        "list by type and status (median)",
        "/in_process/list_type_status_ms/median",
        "ms",
    ),
    (
        "list by type and status, count (check)",
        "/in_process/list_type_status_count",
        "nodes",
    ),
    (
        "add a node and 3 edges in memory (median)",
        "/in_process/add_node_3_edges_ms/median",
        "ms",
    ),
    (
        "rebuild both adjacency arrays (median)",
        "/in_process/rebuild_adjacency_ms/median",
        "ms",
    ),
    (
        "hot part per node (columns, edges, adjacency)",
        "/in_process/mem_hot_per_node_bytes",
        "bytes",
    ),
    (
        "Graph::memory_bytes over the allocator count",
        "/in_process/memory_bytes_over_alloc",
        "ratio",
    ),
    (
        "vector memory per node (s13 band 1,090 at k=4)",
        "/in_process/vectors/mem_per_node_bytes",
        "bytes",
    ),
    (
        "vector memory per node, applied one by one (R81-3)",
        "/in_process/vectors/apply_mem_per_node_bytes",
        "bytes",
    ),
    (
        "vector memory_bytes over the allocator count",
        "/in_process/vectors/memory_bytes_over_alloc",
        "ratio",
    ),
    (
        "decode committed shards into blocks (median)",
        "/in_process/vectors/decode_ms/median",
        "ms",
    ),
    (
        "peak during shard decode over the blocks",
        "/in_process/vectors/decode_peak_over_blocks",
        "ratio",
    ),
    (
        "graph load, then its vectors (median)",
        "/in_process/vectors/graph_and_vectors_load_ms/median",
        "ms",
    ),
    (
        "bytes written for one node's re-embed over the mean shard (band 2)",
        "/in_process/vectors/reembed_one_node/over_mean_shard",
        "ratio",
    ),
    (
        "similar_to rebuild, every list (k 10)",
        "/in_process/vectors/similarity/rebuild_ms",
        "ms",
    ),
    (
        "similar_to longest shared-lock hold (snapshot copy of the whole-node block)",
        "/in_process/vectors/similarity/lock_snapshot_ms",
        "ms",
    ),
    (
        "similar_to longest lock hold (apply, writer)",
        "/in_process/vectors/similarity/lock_apply_ms",
        "ms",
    ),
    (
        "similar_to one-node update (median)",
        "/in_process/vectors/similarity/one_node_update_ms/median",
        "ms",
    ),
    (
        "similar_to lists per node (slots and length)",
        "/in_process/vectors/similarity/slots_per_node_bytes",
        "bytes",
    ),
    (
        "exact scan, whole-node block, top 10 (median)",
        "/in_process/vectors/scan_whole_ms/median",
        "ms",
    ),
    (
        "exact scan, whole-node block in a tenth (median)",
        "/in_process/vectors/scan_whole_tenth_ms/median",
        "ms",
    ),
    (
        "exact scan, every block (median)",
        "/in_process/vectors/scan_every_block_ms/median",
        "ms",
    ),
    (
        "Cypher: slowest supported query (median)",
        "/in_process/cypher/slowest_ms",
        "ms",
    ),
    (
        "Cypher: q5 through services, umx query's path (median)",
        "/in_process/query_q5_ms/median",
        "ms",
    ),
    (
        "Cypher: sum of query medians",
        "/in_process/cypher/sum_of_medians_ms",
        "ms",
    ),
    (
        "Cypher: largest peak memory of one query",
        "/in_process/cypher/max_peak_bytes",
        "bytes",
    ),
    (
        "Cypher: most work units of one accepted query",
        "/in_process/cypher/max_work",
        "units",
    ),
    (
        "Cypher: unbound similar_to scan, score > 0.8, count (median)",
        "/in_process/cypher/similar/queries/0/ms/median",
        "ms",
    ),
    (
        "Cypher: unbound similar_to scan, score > 0.8, RETURN b (median)",
        "/in_process/cypher/similar/queries/1/ms/median",
        "ms",
    ),
    ("daemon RSS idle", "/cli/rss_kb/idle", "KB"),
    ("daemon RSS with graph loaded", "/cli/rss_kb/loaded", "KB"),
    ("daemon RSS after adds", "/cli/rss_kb/after_adds", "KB"),
    ("graph.json", "/fixture/bytes/graph", "bytes"),
    ("nodes.jsonl", "/fixture/bytes/nodes", "bytes"),
    ("edges.jsonl", "/fixture/bytes/edges", "bytes"),
];

/// The fixture parameters a baseline must share for its numbers to be comparable.
const FIXTURE_KEYS: &[&str] = &["kind", "seed", "nodes", "edges_per_node"];

/// A fixture parameter; format 1 results predate the rich fixture, so a missing kind is plain.
fn param(fixture: &Value, key: &str) -> Value {
    match &fixture[key] {
        Value::Null if key == "kind" => json!("plain"),
        v => v.clone(),
    }
}

/// The latest earlier result in `out` for the same fixture, if any. Called before this run's file exists.
fn previous(out: &Path, fixture: &Value) -> Option<(String, Value)> {
    let mut found: Vec<(String, usize, String, Value)> = std::fs::read_dir(out)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| {
            let v: Value = serde_json::from_slice(&std::fs::read(&p).ok()?).ok()?;
            let name = p.file_name()?.to_string_lossy().into_owned();
            FIXTURE_KEYS
                .iter()
                .all(|k| param(&v["fixture"], k) == param(fixture, k))
                .then(|| {
                    (
                        v["date"].as_str().unwrap_or("").to_owned(),
                        name.len(),
                        name,
                        v,
                    )
                })
        })
        .collect();
    // Same second: `x-2` follows `x` and `x-10` follows `x-9`, so order by name length, then name.
    found.sort_by(|a, b| (&a.0, a.1, &a.2).cmp(&(&b.0, b.1, &b.2)));
    found.pop().map(|(_, _, name, v)| (name, v))
}

/// What differs between this run and its baseline beyond the feature under test.
fn differences(r: &Value, p: &Value) -> Vec<&'static str> {
    [
        ("build profile", "/profile"),
        ("run conditions", "/conditions"),
        ("machine", "/machine"),
    ]
    .into_iter()
    .filter(|(_, ptr)| r.pointer(ptr) != p.pointer(ptr))
    .map(|(what, _)| what)
    .collect()
}

fn fmt(v: &Value) -> String {
    match v {
        Value::Null => "n/a".into(),
        other => other.to_string(),
    }
}

fn markdown(r: &Value, prev: Option<&(String, Value)>) -> String {
    let text = |v: &Value| v.as_str().map(str::to_owned).unwrap_or_else(|| fmt(v));
    let gib = r["machine"]["memory_bytes"]
        .as_f64()
        .map_or("n/a".to_owned(), |b| {
            format!("{:.0} GiB", b / (1u64 << 30) as f64)
        });
    let mut md = format!(
        "# umx-bench {}\n\nCommit `{}`, {} build. Conditions: {}.\n\nMachine: {}, {} {}, {} cores, {} memory.\n\n\
Fixture: {}, seed {}, {} nodes, {} edges (synthetic, no client data).\n\n",
        text(&r["date"]),
        text(&r["commit"]),
        text(&r["profile"]),
        r["conditions"]
            .as_str()
            .filter(|c| !c.is_empty())
            .unwrap_or("not recorded"),
        text(&r["machine"]["cpu"]),
        text(&r["machine"]["os"]),
        text(&r["machine"]["arch"]),
        fmt(&r["machine"]["cores"]),
        gib,
        text(&param(&r["fixture"], "kind")),
        r["fixture"]["seed"],
        r["fixture"]["nodes"],
        r["fixture"]["edges"],
    );
    if let Some(batch) = r.pointer("/in_process/batch_ids") {
        md.push_str("| batch of 1,000 adds | writer hold median (ms) | graph apply median (ms) | peak dead bytes (raw runs) |\n|---|---:|---:|---:|\n");
        for mode in [
            "appended",
            "random_explicit",
            "appended_with_edges",
            "random_explicit_with_edges",
            "mixed_future",
        ] {
            let row = &batch["measurements"][mode];
            md.push_str(&format!(
                "| {mode} | {} | {} | {} |\n",
                row["writer_hold"]["median"], row["graph_apply"]["median"], row["peak_dead_bytes"],
            ));
        }
        md.push_str(&format!(
            "\nRandom explicit / appended writer hold: {}x. Raw runs are in the JSON file.\n",
            batch["random_to_append"],
        ));
        return md;
    }
    match prev {
        Some((name, p)) => {
            md.push_str(&format!("Compared with `{name}`."));
            let diff = differences(r, p);
            if !diff.is_empty() {
                md.push_str(&format!(
                    " Not like for like, it differs in: {}. Not every change is the feature's.",
                    diff.join(", ")
                ));
            }
            md.push_str("\n\n| metric | now | before | change |\n|---|---|---|---|\n");
        }
        None => md.push_str(
            "No earlier result for this fixture (kind, seed, nodes, edges per node).\n\n| metric | now |\n|---|---|\n",
        ),
    }
    for (label, pointer, unit) in METRICS {
        let now = r.pointer(pointer).cloned().unwrap_or(Value::Null);
        match prev {
            Some((_, p)) => {
                let before = p.pointer(pointer).cloned().unwrap_or(Value::Null);
                let change = match (now.as_f64(), before.as_f64()) {
                    (Some(n), Some(b)) if b > 0.0 => format!("{:+.1}%", (n - b) / b * 100.0),
                    _ => "n/a".into(),
                };
                md.push_str(&format!(
                    "| {label} | {} {unit} | {} {unit} | {change} |\n",
                    fmt(&now),
                    fmt(&before)
                ));
            }
            None => md.push_str(&format!("| {label} | {} {unit} |\n", fmt(&now))),
        }
    }
    md.push_str("\nNumbers report, they do not gate (D8). Full figures are in the .json beside this file.\n");
    md
}

/// Writes the result; returns the JSON file's path.
pub fn write(out: &Path, r: &Value) -> Result<PathBuf, String> {
    std::fs::create_dir_all(out).map_err(|e| format!("make {}: {e}", out.display()))?;
    let date = r["date"].as_str().unwrap_or("");
    let day = date.get(..10).unwrap_or("unknown");
    let time: String = date
        .get(11..19)
        .unwrap_or("000000")
        .chars()
        .filter(char::is_ascii_digit)
        .collect();
    let base = format!(
        "{day}-{time}-{}-{}n{}",
        r["commit"].as_str().unwrap_or("unknown"),
        r["fixture"]["nodes"],
        if param(&r["fixture"], "kind") == "rich" {
            "-rich"
        } else {
            ""
        }
    );
    let prev = previous(out, &r["fixture"]);
    let mut bytes = serde_json::to_vec_pretty(r).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    // Claim a fresh name so no run ever overwrites another, even two in the same second.
    let (stem, json_path) = (1..)
        .map(|n| match n {
            1 => base.clone(),
            n => format!("{base}-{n}"),
        })
        .find_map(|stem| {
            let path = out.join(format!("{stem}.json"));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut f) => Some(
                    f.write_all(&bytes)
                        .map(|()| (stem, path.clone()))
                        .map_err(|e| format!("write {}: {e}", path.display())),
                ),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(e) => Some(Err(format!("write {}: {e}", path.display()))),
            }
        })
        .expect("an unused name exists")?;
    std::fs::write(out.join(format!("{stem}.md")), markdown(r, prev.as_ref()))
        .map_err(|e| format!("write {stem}.md: {e}"))?;
    Ok(json_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(date: &str, commit: &str, seed: u64, nodes: u64, load: f64) -> Value {
        json!({ "date": date, "commit": commit, "profile": "release", "conditions": "test",
                "machine": {}, "fixture": { "seed": seed, "nodes": nodes, "edges": 3, "edges_per_node": 3,
                "bytes": {} }, "in_process": { "load_ms": { "median": load } }, "cli": {} })
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("umx-bench-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_summary_compares_with_the_previous_result_of_the_same_fixture() {
        let dir = scratch("report");
        write(&dir, &result("2026-11-09T01:00:00Z", "aaa", 1, 10, 100.0)).unwrap();
        write(&dir, &result("2026-11-09T01:30:00Z", "zzz", 1, 99, 1.0)).unwrap();
        write(&dir, &result("2026-11-09T01:40:00Z", "sss", 2, 10, 1.0)).unwrap();
        let mut other = result("2026-11-09T01:50:00Z", "eee", 1, 10, 1.0);
        other["fixture"]["edges_per_node"] = json!(5);
        write(&dir, &other).unwrap();
        let mut now = result("2026-11-09T02:00:00Z", "bbb", 1, 10, 150.0);
        now["profile"] = json!("debug");
        let path = write(&dir, &now).unwrap();
        assert!(path.ends_with("2026-11-09-020000-bbb-10n.json"));
        let md = std::fs::read_to_string(dir.join("2026-11-09-020000-bbb-10n.md")).unwrap();
        assert!(
            md.contains("Compared with `2026-11-09-010000-aaa-10n.json`. Not like for like, it differs in: build profile."),
            "{md}"
        );
        assert!(
            md.contains("| in-process graph load (median) | 150.0 ms | 100.0 ms | +50.0% |"),
            "{md}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rich_results_are_named_apart_and_compare_only_with_rich_and_old_results_read_as_plain() {
        let dir = scratch("rich");
        // Format 1: no kind.
        write(&dir, &result("2026-11-09T01:00:00Z", "old", 1, 10, 100.0)).unwrap();
        let mut rich = result("2026-11-10T01:00:00Z", "new", 1, 10, 300.0);
        rich["fixture"]["kind"] = json!("rich");
        let first_rich = write(&dir, &rich).unwrap();
        assert!(first_rich.ends_with("2026-11-10-010000-new-10n-rich.json"));
        let md = std::fs::read_to_string(first_rich.with_extension("md")).unwrap();
        assert!(
            md.contains("No earlier result") && md.contains("Fixture: rich,"),
            "{md}"
        );
        let mut plain = result("2026-11-10T02:00:00Z", "new", 1, 10, 150.0);
        plain["fixture"]["kind"] = json!("plain");
        let plain = write(&dir, &plain).unwrap();
        let md = std::fs::read_to_string(plain.with_extension("md")).unwrap();
        assert!(
            md.contains("Compared with `2026-11-09-010000-old-10n.json`.")
                && md.contains("Fixture: plain,"),
            "{md}"
        );
        rich["date"] = json!("2026-11-10T03:00:00Z");
        let second_rich = write(&dir, &rich).unwrap();
        let md = std::fs::read_to_string(second_rich.with_extension("md")).unwrap();
        assert!(
            md.contains("Compared with `2026-11-10-010000-new-10n-rich.json`."),
            "{md}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn repeated_runs_on_one_commit_keep_every_result_and_compare_with_the_last() {
        let dir = scratch("repeat");
        let first = write(&dir, &result("2026-11-09T08:29:51Z", "ccc", 1, 10, 100.0)).unwrap();
        let second = write(&dir, &result("2026-11-09T08:30:12Z", "ccc", 1, 10, 110.0)).unwrap();
        let third = write(&dir, &result("2026-11-09T08:30:12Z", "ccc", 1, 10, 121.0)).unwrap();
        let fourth = write(&dir, &result("2026-11-09T08:30:12Z", "ccc", 1, 10, 133.1)).unwrap();
        let names: Vec<_> = [&first, &second, &third, &fourth]
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            [
                "2026-11-09-082951-ccc-10n.json",
                "2026-11-09-083012-ccc-10n.json",
                "2026-11-09-083012-ccc-10n-2.json",
                "2026-11-09-083012-ccc-10n-3.json",
            ]
        );
        let load = |p: &PathBuf| {
            let v: Value = serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap();
            v["in_process"]["load_ms"]["median"].as_f64().unwrap()
        };
        assert_eq!(
            [load(&first), load(&second), load(&third), load(&fourth)],
            [100.0, 110.0, 121.0, 133.1]
        );
        let md = |p: &PathBuf| std::fs::read_to_string(p.with_extension("md")).unwrap();
        assert!(md(&first).contains("No earlier result"), "{}", md(&first));
        assert!(
            md(&second).contains("Compared with `2026-11-09-082951-ccc-10n.json`."),
            "{}",
            md(&second)
        );
        assert!(
            md(&third).contains("Compared with `2026-11-09-083012-ccc-10n.json`."),
            "{}",
            md(&third)
        );
        assert!(
            md(&fourth).contains("Compared with `2026-11-09-083012-ccc-10n-2.json`."),
            "{}",
            md(&fourth)
        );
        assert!(
            md(&second).contains("| 110.0 ms | 100.0 ms | +10.0% |"),
            "{}",
            md(&second)
        );
        assert!(
            !md(&second).contains("Not like for like"),
            "{}",
            md(&second)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

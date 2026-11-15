//! Security's B1: memory is bounded when rows carry lists. A bound list is shared between rows (cloning a
//! row is O(1)), and making or reading a list costs its elements in work units, so the budget bounds how many
//! elements a query can materialise. Its own test binary: the counting allocator measures this process only.

use std::alloc::{GlobalAlloc, Layout, System};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use umx_core::codec::{decode_edges, decode_nodes};
use umx_graph::Graph;
use umx_query::{GraphView, RunOptions, run};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let live = LIVE.fetch_add(layout.size(), Relaxed) + layout.size();
        PEAK.fetch_max(live, Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// Peak bytes allocated while `f` runs, over what was live before it.
fn peak_of<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let base = LIVE.load(Relaxed);
    PEAK.store(base, Relaxed);
    let out = f();
    (out, PEAK.load(Relaxed).saturating_sub(base))
}

fn design() -> Graph {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../umx-cypher/fixtures/design");
    let nodes = decode_nodes(&fs::read(dir.join("nodes.jsonl")).unwrap()).unwrap();
    let edges = decode_edges(&fs::read(dir.join("edges.jsonl")).unwrap()).unwrap();
    Graph::from_records(nodes.records, edges.records)
}

const MB: usize = 1 << 20;

/// `WITH [0, ..., n-1] AS a UNWIND a AS x RETURN count(*)` peaked at 7.25 GB at n = 10,000 (quadratic) before
/// the fix. Now it answers n, and its peak grows with n, not n^2; and a list too large for the budget stops with
/// query_too_large.
#[test]
fn a_list_bound_across_rows_is_shared_and_charged() {
    let g = design();
    let view = GraphView::one("design", &g, &[]);
    let options = RunOptions::default();
    let query = |n: usize| {
        let list: Vec<String> = (0..n).map(|i| i.to_string()).collect();
        format!(
            "WITH [{}] AS a UNWIND a AS x RETURN count(*) AS c",
            list.join(", ")
        )
    };
    let mut peaks = Vec::new();
    for n in [10_000, 100_000] {
        let text = query(n);
        let (result, peak) = peak_of(|| run(&view, &text, &options));
        let rows = result.unwrap_or_else(|p| panic!("n = {n}: {p:?}"));
        assert_eq!(rows.rows[0][0], umx_query::Cell::Int(n as i64), "n = {n}");
        eprintln!("n = {n}: peak {} KB", peak / 1024);
        peaks.push(peak);
        // About the parsed query, the list once and the rows: tens of MB at 100k, never GB.
        assert!(peak < 200 * MB, "n = {n}: peak {} MB", peak / MB);
    }
    // Ten times the elements, well under a hundred times the memory.
    assert!(peaks[1] < peaks[0] * 30, "{peaks:?}");

    // Reading the list on every row is n^2 work: the budget stops it, with bounded memory.
    let text = format!(
        "WITH [{}] AS a UNWIND a AS x WITH a, x WHERE x IN a RETURN count(*) AS c",
        (0..20_000)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let (result, peak) = peak_of(|| run(&view, &text, &options));
    assert_eq!(
        result.expect_err("n^2 work").code,
        umx_query::check::TOO_LARGE
    );
    assert!(peak < 200 * MB, "peak {} MB", peak / MB);
}

/// The graph form: collect every node, then unwind it, at 100k nodes.
#[test]
fn collect_then_unwind_over_a_big_graph_is_bounded() {
    let n = 100_000_u32;
    let nodes: String = (0..n)
        .map(|i| {
            format!(
                r#"{{"id":"01JDBXDGET{i:0>16}","type":"decision","title":"n{i}","status":"open","note":"","evidence":[],"status_history":[],"created":"2026-11-10T00:00:00Z","updated":"2026-11-10T00:00:00Z","created_by":"fixture"}}"#
            ) + "\n"
        })
        .collect();
    let g = Graph::from_records(
        decode_nodes(nodes.as_bytes()).unwrap().records,
        decode_edges(b"").unwrap().records,
    );
    let view = GraphView::one("design", &g, &[]);
    let (result, peak) = peak_of(|| {
        run(
            &view,
            "MATCH (n) WITH collect(n) AS a UNWIND a AS x RETURN count(*) AS c",
            &RunOptions::default(),
        )
    });
    eprintln!("collect then unwind at 100k nodes: peak {} KB", peak / 1024);
    match result {
        Ok(rows) => assert_eq!(rows.rows[0][0], umx_query::Cell::Int(i64::from(n))),
        Err(p) => assert_eq!(p.code, umx_query::check::TOO_LARGE),
    }
    assert!(peak < 400 * MB, "peak {} MB", peak / MB);
}

/// Reviewer R115-2, depth: each `WITH [x] AS x` is shallow, but the value nests one level per clause; 5,000
/// clauses aborted a 2 MiB stack. A value past MAX_VALUE_DEPTH is refused when it is built.
#[test]
fn a_value_nested_across_clauses_is_capped() {
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(|| {
            let g = design();
            let view = GraphView::one("design", &g, &[]);
            let query = |k: usize| {
                format!(
                    "UNWIND [0] AS x{} RETURN size(x) AS n",
                    " WITH [x] AS x".repeat(k)
                )
            };
            let deepest = umx_cypher::MAX_VALUE_DEPTH;
            let rows = run(&view, &query(deepest), &RunOptions::default()).expect("fits");
            assert_eq!(rows.rows[0][0], umx_query::Cell::Int(1));
            for k in [deepest + 1, 5_000] {
                let p = run(&view, &query(k), &RunOptions::default()).expect_err("too deep");
                assert_eq!(p.code, umx_query::check::INVALID, "k = {k}");
                assert!(p.slot("reason").unwrap().contains("nested"), "{p:?}");
            }
        })
        .expect("thread")
        .join()
        .expect("no stack overflow");
}

/// Reviewer R115-2, width: each `WITH [x,x] AS x` doubles the value for one unit of work; at 19 clauses it held
/// 215 MB for 21 units. Copying a bound value costs ELEMENT_COST per element, so the budget stops the doubling
/// with a bounded peak.
#[test]
fn a_value_doubled_across_clauses_meets_the_budget() {
    let g = design();
    let view = GraphView::one("design", &g, &[]);
    for k in [16, 19, 22, 26, 30] {
        let text = format!(
            "UNWIND [0] AS x{} RETURN size(x) AS n",
            " WITH [x, x] AS x".repeat(k)
        );
        let (result, peak) = peak_of(|| run(&view, &text, &RunOptions::default()));
        eprintln!(
            "k = {k}: {} peak {} KB",
            match &result {
                Ok(r) => format!("ok work {}", r.work),
                Err(p) => p.code.to_string(),
            },
            peak / 1024
        );
        if let Err(p) = &result {
            assert_eq!(p.code, umx_query::check::TOO_LARGE, "k = {k}");
        }
        assert!(peak < 200 * MB, "k = {k}: peak {} MB", peak / MB);
    }
}

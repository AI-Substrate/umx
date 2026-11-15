//! ws008 TM2, security B1: `x IN <hoisted list>` checks a date against the list's dates one by one, so that
//! scan is charged to the work budget, and it runs only when `x` reads as a date.

use std::collections::BTreeMap;

use serde_json::json;
use umx_core::{Id, Node};
use umx_graph::Graph;
use umx_query::{GraphView, RunOptions, run};

const NODES: usize = 200;
const LIST: usize = 500;

fn graph() -> Graph {
    let nodes = (0..NODES)
        .map(|i| {
            let mut fields = BTreeMap::new();
            fields.insert(
                "due_on".to_string(),
                json!(format!("2025-{:02}", 1 + i % 12)),
            );
            Node {
                id: Id((0x0194_0000_0000_u128 << 80) | (i as u128 + 1)),
                kind: "action".into(),
                title: format!("action {i}"),
                status: "open".into(),
                note: String::new(),
                evidence: vec![],
                status_history: vec![],
                created: "2026-11-10T01:00:00Z".into(),
                updated: "2026-11-10T01:00:00Z".into(),
                created_by: "t".into(),
                locator: None,
                authored_by_session: None,
                edited_by_session: None,
                fields,
            }
        })
        .collect();
    Graph::from_records(nodes, vec![])
}

/// `WITH [<LIST days of 2024>] AS days MATCH (n) WHERE n.<field> IN days`: no day of 2024 falls in a 2025 month, so
/// every row scans the whole list.
fn query(field: &str) -> String {
    let days: Vec<String> = (0..LIST)
        .map(|i| format!("\"2024-{:02}-{:02}\"", 1 + i % 12, 1 + i % 28))
        .collect();
    format!(
        "WITH [{}] AS days MATCH (n) WHERE n.{field} IN days RETURN count(*) AS c",
        days.join(", ")
    )
}

#[test]
fn the_hoisted_in_date_scan_is_charged_and_only_for_dates() {
    let g = graph();
    let view = GraphView::one("dates", &g, &[]);
    let options = RunOptions::default();
    let dated = run(&view, &query("due_on"), &options).unwrap();
    assert_eq!(dated.rows[0][0], umx_query::Cell::Int(0));
    let titled = run(&view, &query("title"), &options).unwrap();
    // Each dated row pays for the list it scans; a title is no date, so it scans nothing.
    assert!(dated.work >= (NODES * LIST) as u64, "{}", dated.work);
    assert!(titled.work < (NODES * LIST) as u64 / 10, "{}", titled.work);
    // So a budget the scan cannot fit stops the query.
    let tight = RunOptions {
        max_work: titled.work + (NODES * LIST) as u64 / 2,
        ..RunOptions::default()
    };
    let refused = run(&view, &query("due_on"), &tight).unwrap_err();
    assert_eq!(refused.code, "query_too_large");
    assert!(run(&view, &query("title"), &tight).is_ok());
}

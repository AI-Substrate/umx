//! D71 step 10: the Cypher date functions over umx-graph, end to end through `run`. Values stay text, answers are in
//! UTC, a part or cut finer than the value is null, and a query has one now.

use std::collections::BTreeMap;

use serde_json::json;
use umx_core::{Id, Node};
use umx_graph::Graph;
use umx_query::{Cell, GraphView, RunOptions, run};

/// 2026-11-13T09:30:15Z.
const NOW_MS: u64 = 1_794_562_215_000;

fn node(i: u128, due: Option<&str>) -> Node {
    let mut fields = BTreeMap::new();
    if let Some(due) = due {
        fields.insert("due_on".to_string(), json!(due));
    }
    Node {
        id: Id((0x0194_0000_0000_u128 << 80) | i),
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
}

/// One node per precision, one with text that is no date, one without the field, and one due today (by `NOW_MS`).
fn graph() -> Graph {
    let dues = [
        Some("2025"),
        Some("2025-01"),
        Some("2026-12-17"),
        Some("2026-10-29T22:40:00Z"),
        Some("1066-10-14"),
        Some("soon"),
        None,
        Some("2026-11-13"),
    ];
    let nodes = dues
        .iter()
        .enumerate()
        .map(|(i, due)| node(i as u128 + 1, *due))
        .collect();
    Graph::from_records(nodes, vec![])
}

fn options() -> RunOptions<'static> {
    RunOptions {
        now_ms: Some(NOW_MS),
        ..RunOptions::default()
    }
}

fn rows(g: &Graph, text: &str) -> Vec<Vec<Cell>> {
    let view = GraphView::one("dates", g, &[]);
    run(&view, text, &options())
        .unwrap_or_else(|e| panic!("{text}: {e:?}"))
        .rows
}

fn one(g: &Graph, expr: &str) -> Cell {
    // umx's subset starts a query with MATCH, OPTIONAL MATCH, UNWIND or WITH.
    rows(g, &format!("WITH 1 AS one RETURN {expr} AS x"))
        .remove(0)
        .remove(0)
}

fn s(v: &str) -> Cell {
    Cell::Str(v.into())
}

#[test]
fn each_function_answers_in_utc_at_the_value_precision() {
    let g = graph();
    for (expr, want) in [
        ("date('2026-12-01')", s("2026-12-01")),
        ("date('2025-01')", s("2025-01")),
        ("date('2025')", s("2025")),
        ("date('2026-10-30T09:40:00+11:00')", s("2026-10-29")),
        ("date('1066-10-14')", s("1066-10-14")),
        ("date('soon')", Cell::Null),
        ("date(NULL)", Cell::Null),
        (
            "timestamp('2026-10-30T09:40:00+11:00')",
            s("2026-10-29T22:40:00Z"),
        ),
        (
            "timestamp('2026-10-30 04:10-05:30')",
            s("2026-10-30T09:40:00Z"),
        ),
        ("timestamp('2026-12-01')", s("2026-12-01T00:00:00Z")),
        ("timestamp('2025-01')", Cell::Null),
        (
            "timestamp('1969-07-20T20:17:40Z')",
            s("1969-07-20T20:17:40Z"),
        ),
        ("timestamp('1066-10-14')", s("1066-10-14T00:00:00Z")),
        ("date_part('year', date('2026-12-01'))", Cell::Int(2026)),
        ("DATEPART('week', '2027-01-01')", Cell::Int(53)),
        ("date_part('dayofweek', '2026-12-06')", Cell::Int(0)),
        (
            "date_part('hour', '2026-10-30T09:40:00+11:00')",
            Cell::Int(22),
        ),
        ("date_part('hour', '2026-12-01')", Cell::Null),
        ("date_part('day', '2025-01')", Cell::Null),
        ("date_part('year', NULL)", Cell::Null),
        ("date_part(NULL, '2026-12-01')", Cell::Null),
        ("date_trunc('week', '2026-12-17')", s("2026-12-14")),
        ("datetrunc('MONTH', '2026-12-17')", s("2026-12-01")),
        (
            "date_trunc('hour', '2026-10-29T22:40:59Z')",
            s("2026-10-29T22:00:00Z"),
        ),
        ("date_trunc('week', '2025-01')", Cell::Null),
        ("date_trunc('month', NULL)", Cell::Null),
        (
            "date_diff('month', '2025-01-31', '2025-02-01')",
            Cell::Int(1),
        ),
        (
            "date_diff('day', '2026-12-01', '2026-11-01')",
            Cell::Int(-30),
        ),
        (
            "date_diff('day', '1066-10-14', '2026-12-01')",
            Cell::Int(350_681),
        ),
        ("date_diff('day', '2025-01', '2025-03')", Cell::Null),
        ("date_diff('day', 'soon', '2025-03-01')", Cell::Null),
        ("current_date()", s("2026-11-13")),
        ("current_timestamp()", s("2026-11-13T09:30:15Z")),
        (
            "date_diff('day', '2026-11-01', current_date())",
            Cell::Int(12),
        ),
        // A date result is text that compares by instant (row 24): a day equals the month holding it.
        ("date('2026-12-17') = '2026-12'", Cell::Bool(true)),
        (
            "timestamp('2026-10-30T09:40:00+11:00') < '2026-10-30'",
            Cell::Bool(true),
        ),
    ] {
        assert_eq!(one(&g, expr), want, "{expr}");
    }
}

#[test]
fn stored_values_group_by_bucket_and_text_that_is_no_date_is_null() {
    let g = graph();
    let got = rows(
        &g,
        "MATCH (n) WHERE n.due_on IS NOT NULL \
         RETURN n.due_on AS due, date(n.due_on) AS d, date_part('month', n.due_on) AS m, \
         date_trunc('year', n.due_on) AS y ORDER BY due",
    );
    let want = [
        (
            "1066-10-14",
            s("1066-10-14"),
            Cell::Int(10),
            s("1066-01-01"),
        ),
        ("2025", s("2025"), Cell::Null, s("2025-01-01")),
        ("2025-01", s("2025-01"), Cell::Int(1), s("2025-01-01")),
        (
            "2026-10-29T22:40:00Z",
            s("2026-10-29"),
            Cell::Int(10),
            s("2026-01-01"),
        ),
        (
            "2026-11-13",
            s("2026-11-13"),
            Cell::Int(11),
            s("2026-01-01"),
        ),
        (
            "2026-12-17",
            s("2026-12-17"),
            Cell::Int(12),
            s("2026-01-01"),
        ),
        ("soon", Cell::Null, Cell::Null, Cell::Null),
    ];
    let want: Vec<Vec<Cell>> = want
        .into_iter()
        .map(|(due, d, m, y)| vec![s(due), d, m, y])
        .collect();
    assert_eq!(got, want);
    let buckets = rows(
        &g,
        "MATCH (n) WHERE date(n.due_on) IS NOT NULL \
         RETURN date_trunc('year', n.due_on) AS y, count(*) AS c ORDER BY y",
    );
    assert_eq!(
        buckets,
        vec![
            vec![s("1066-01-01"), Cell::Int(1)],
            vec![s("2025-01-01"), Cell::Int(2)],
            vec![s("2026-01-01"), Cell::Int(3)],
        ]
    );
}

/// One now per query: on every row, in a hoisted `IN` list, and against a stored day, all from `RunOptions::now_ms`.
#[test]
fn a_query_has_one_now() {
    let g = graph();
    let stamps = rows(
        &g,
        "MATCH (n) RETURN DISTINCT current_timestamp() AS t, current_date() AS d",
    );
    assert_eq!(
        stamps,
        vec![vec![s("2026-11-13T09:30:15Z"), s("2026-11-13")]]
    );
    let hoisted = rows(
        &g,
        "WITH [current_date(), '2020-01-01'] AS days MATCH (n) WHERE current_date() IN days \
         RETURN count(*) AS c",
    );
    assert_eq!(hoisted, vec![vec![Cell::Int(8)]]);
    let due_today = rows(
        &g,
        "MATCH (n) WHERE n.due_on IN [current_date()] RETURN n.due_on AS due ORDER BY due",
    );
    // `2026-11` would hold today too; the stored month-free values that overlap today are the day itself.
    assert_eq!(due_today, vec![vec![s("2026-11-13")]]);
    // Another instant, another now.
    let view = GraphView::one("dates", &g, &[]);
    let later = RunOptions {
        now_ms: Some(NOW_MS + 86_400_000),
        ..RunOptions::default()
    };
    let next = run(&view, "WITH 1 AS one RETURN current_date() AS d", &later).unwrap();
    assert_eq!(next.rows, vec![vec![s("2026-11-14")]]);
}

#[test]
fn bad_units_counts_and_kinds_are_errors_and_other_spellings_are_hinted() {
    let g = graph();
    let view = GraphView::one("dates", &g, &[]);
    for (text, reason) in [
        (
            "RETURN date_part('fortnight', '2026-12-01')",
            "a date function got a unit",
        ),
        (
            "RETURN date_trunc('dayofweek', '2026-12-01')",
            "a date function got a unit",
        ),
        (
            "RETURN date_diff('dayofweek', '2026-12-01', '2026-12-02')",
            "a date function got a unit",
        ),
        (
            "RETURN date_part('fortnight', NULL)",
            "a date function got a unit",
        ),
        (
            "RETURN date('2026-12-01', 'x')",
            "wrong number of arguments",
        ),
        ("RETURN current_date(1)", "wrong number of arguments"),
        ("RETURN date(20261201)", "a value of the wrong kind"),
        (
            "RETURN date_part(1, '2026-12-01')",
            "a value of the wrong kind",
        ),
    ] {
        let e = run(&view, &format!("WITH 1 AS one {text}"), &options()).unwrap_err();
        assert_eq!(e.code, "query_invalid", "{text}");
        let got = e.slot("reason").unwrap_or_default();
        assert!(got.contains(reason) && !got.contains(','), "{text}: {got}");
    }
    // A caller that gives no now gets an error, never 1970.
    let e = run(
        &view,
        "WITH 1 AS one RETURN current_date() AS d",
        &RunOptions::default(),
    )
    .unwrap_err();
    assert_eq!(e.code, "query_invalid");
    for (text, hint) in [
        ("RETURN now()", "current_timestamp()"),
        ("RETURN datetime('2026-12-01')", "current_timestamp()"),
        (
            "RETURN datediff('day', '2026-12-01', '2026-12-02')",
            "use date_diff",
        ),
    ] {
        let e = run(&view, &format!("WITH 1 AS one {text}"), &options()).unwrap_err();
        assert_eq!(e.code, "query_unknown_function", "{text}");
        let got = e.slot("hint").unwrap_or_default();
        assert!(got.contains(hint), "{text}: {got}");
        // The envelope shows a word's comma as `?`, so umx's words carry none.
        assert!(!got.contains(','), "{text}: {got}");
    }
}

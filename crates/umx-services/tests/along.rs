//! `--along <spine> [--after <end>] [--before <end>]` in services (D71 step 12, plan 206): the spine range filter on
//! node list, node find, search, edge list and query; ends inclusive; by-rule and unplaced rows left out and counted;
//! the built-ins; one graph only; and the refusals.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::json;
use umx_core::codec::GraphFile;
use umx_core::ports::GraphDir;
use umx_core::value::SetValue;
use umx_core::{Id, Problem};
use umx_fakes::{FakeClock, FakeGraphFiles, FakeIds, FakeLocalState};
use umx_graph::columns::FieldRef;
use umx_services::cypher::Cell;
use umx_services::{
    AlongRequest, AlongSource, Ctx, CypherRequest, EdgeListRequest, NodeAddRequest,
    NodeListRequest, Ports, Scope, Services,
};

fn services() -> Services {
    services_over(Arc::new(FakeGraphFiles::new(
        GraphDir::parse("/work").unwrap(),
    )))
}

fn services_over(files: Arc<FakeGraphFiles>) -> Services {
    Services::new(Ports {
        sources: Arc::new(umx_fakes::FakeSourceReader::open(
            GraphDir::parse("/").unwrap(),
        )),
        files: files.clone(),
        vector_cache: files,
        vector_cache_root: GraphDir::parse("/work/cache").unwrap(),
        clock: Arc::new(FakeClock::new(1_800_000_000_000)),
        ids: Arc::new(FakeIds::new()),
        state: Arc::new(FakeLocalState::new()),
        scopes: Arc::new(FakeLocalState::new()),
        registry: Arc::new(FakeLocalState::new()),
        allows: Arc::new(FakeLocalState::new()),
        lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
    })
}

fn ctx() -> Ctx {
    Ctx::new("tester").graph("demo")
}

fn add(
    svc: &mut Services,
    kind: &str,
    title: &str,
    sets: &[(&str, &str)],
    raised_in: Option<Id>,
) -> Id {
    svc.node_add(
        &ctx(),
        NodeAddRequest {
            kind: kind.into(),
            title: title.into(),
            sets: sets
                .iter()
                .map(|(f, v)| (f.to_string(), SetValue::Raw(v.to_string())))
                .collect(),
            raised_in: raised_in.map(|id| id.to_text()),
            ..Default::default()
        },
    )
    .unwrap()
    .value
    .node
    .id
}

fn link(svc: &mut Services, from: Id, relation: &str, to: Id) {
    svc.edge_add(&ctx(), &from.to_text(), relation, &to.to_text(), &[])
        .unwrap();
}

/// A synthetic day: four segments dated a day apart (steps 1 to 4), two points raised in each, one problem dated
/// before every segment, one linked only to a point of the second segment (placed by rule), and one on nothing;
/// the declared spine `day` over the segments.
struct Day {
    svc: Services,
    segs: [Id; 4],
    points: [[Id; 2]; 4],
    early: Id,
    ruled: Id,
    alone: Id,
}

fn day() -> Day {
    let mut svc = services();
    svc.graph_new(&Ctx::new("tester"), "/work/demo", "demo", None, false, None)
        .unwrap();
    let mut segs = [Id(0); 4];
    let mut points = [[Id(0); 2]; 4];
    for s in 0..4 {
        let at = format!("2026-11-1{}T09:00:00Z", s + 1);
        let step = format!("{}", s + 1);
        segs[s] = add(
            &mut svc,
            "segment",
            &format!("Segment {s}"),
            &[("starts_on", &at), ("step", &step)],
            None,
        );
        for (p, point) in points[s].iter_mut().enumerate() {
            *point = add(
                &mut svc,
                "risk",
                &format!("Point {s}{p}"),
                &[],
                Some(segs[s]),
            );
        }
    }
    let early = add(
        &mut svc,
        "risk",
        "Dated before the day",
        &[("starts_on", "2026-11-10")],
        None,
    );
    let ruled = add(&mut svc, "risk", "Linked to a point only", &[], None);
    link(&mut svc, ruled, "relates_to", points[1][0]);
    let alone = add(&mut svc, "risk", "On nothing", &[], None);
    let decl = json!({
        "name": "day", "title": "The day", "main": true,
        "anchors": { "type": "segment", "inside": "part_of" },
        "order": ["starts_on", "step"], "place": ["raised_in"],
    });
    svc.graph_spine_set(&ctx(), "demo", &decl).unwrap();
    Day {
        svc,
        segs,
        points,
        early,
        ruled,
        alone,
    }
}

fn along(spine: &str, after: Option<Id>, before: Option<Id>) -> AlongRequest {
    AlongRequest {
        spine: spine.into(),
        after: after.map(|id| id.to_text()),
        before: before.map(|id| id.to_text()),
    }
}

fn listed(
    svc: &Services,
    c: &Ctx,
    a: AlongRequest,
) -> (BTreeSet<Id>, umx_services::AlongInfo, Vec<Problem>) {
    let done = svc
        .node_list(
            c,
            NodeListRequest {
                limit: 1000,
                along: Some(a),
                ..Default::default()
            },
        )
        .unwrap();
    let ids = done.value.rows.iter().map(|n| n.id()).collect();
    (ids, done.value.along.unwrap(), done.notes)
}

fn ids(list: &[Id]) -> BTreeSet<Id> {
    list.iter().copied().collect()
}

fn code(err: Result<impl std::fmt::Debug, Problem>) -> &'static str {
    err.unwrap_err().code
}

#[test]
fn both_ends_are_inclusive_and_an_open_end_runs_to_the_edge_of_the_spine() {
    let d = day();
    let (got, info, _) = listed(
        &d.svc,
        &ctx(),
        along("day", Some(d.segs[1]), Some(d.segs[2])),
    );
    let mut want = vec![d.segs[1], d.segs[2]];
    want.extend(d.points[1]);
    want.extend(d.points[2]);
    assert_eq!(got, ids(&want));
    assert_eq!(
        (info.source, info.after.as_deref(), info.before.as_deref()),
        (
            AlongSource::Declared,
            Some(d.segs[1].to_text().as_str()),
            Some(d.segs[2].to_text().as_str())
        )
    );
    // One anchor at both ends: its own band only.
    let (got, _, _) = listed(
        &d.svc,
        &ctx(),
        along("day", Some(d.segs[3]), Some(d.segs[3])),
    );
    assert_eq!(got, ids(&[d.segs[3], d.points[3][0], d.points[3][1]]));
    // Open after: from the start, including the node dated before every segment.
    let (got, _, _) = listed(&d.svc, &ctx(), along("day", None, Some(d.segs[0])));
    assert_eq!(
        got,
        ids(&[d.segs[0], d.points[0][0], d.points[0][1], d.early])
    );
    // Open before: to the end.
    let (got, _, _) = listed(&d.svc, &ctx(), along("day", Some(d.segs[3]), None));
    assert_eq!(got, ids(&[d.segs[3], d.points[3][0], d.points[3][1]]));
    // No end: every node the spine places by data.
    let (got, info, _) = listed(&d.svc, &ctx(), along("day", None, None));
    assert_eq!(got.len(), 4 + 8 + 1);
    assert!(!got.contains(&d.ruled) && !got.contains(&d.alone));
    assert_eq!((info.not_placed, info.by_rule, info.coarse), (1, 1, 0));
}

#[test]
fn by_rule_and_unplaced_rows_are_left_out_counted_and_listed_by_the_hint() {
    let d = day();
    let (got, info, notes) = listed(&d.svc, &ctx(), along("day", None, None));
    assert!(!got.contains(&d.ruled));
    let codes: Vec<&str> = notes.iter().map(|p| p.code).collect();
    assert_eq!(codes, ["along_not_placed", "along_by_rule"]);
    assert_eq!((info.not_placed, info.by_rule), (1, 1));
    assert_eq!(
        notes[0].slot("fix").unwrap(),
        "umx graph spine show demo --unplaced day"
    );
    assert_eq!(
        notes[1].slot("fix").unwrap(),
        "umx graph spine show demo --by-rule day"
    );
    // The listings the hints name hold exactly those nodes.
    let shown = d
        .svc
        .graph_spine_show(&ctx(), "demo", Some("day"), false, true, 100, None)
        .unwrap()
        .value;
    assert_eq!(shown.unplaced.unwrap().ids, vec![d.alone]);
    let shown = d
        .svc
        .graph_spine_show(&ctx(), "demo", Some("day"), true, false, 100, None)
        .unwrap()
        .value;
    assert_eq!(shown.by_rule.unwrap().ids, vec![d.ruled]);
    // The counts are of the rows the other filters kept: --type segment keeps no unplaced or ruled row.
    let done = d
        .svc
        .node_list(
            &ctx(),
            NodeListRequest {
                kind: Some("segment".into()),
                along: Some(along("day", None, None)),
                ..Default::default()
            },
        )
        .unwrap();
    let info = done.value.along.unwrap();
    assert_eq!(
        (done.value.total, info.not_placed, info.by_rule),
        (Some(4), 0, 0)
    );
    assert!(done.notes.is_empty(), "{:?}", done.notes);
}

#[test]
fn with_no_dated_anchor_show_unplaced_and_along_agree_on_a_dated_point() {
    // Reviewer B3: anchors ordered by starts_on then step hold only step. A point's own starts_on places nothing:
    // unlinked it is not placed, counted by --along and listed by show --unplaced; linked it sits with its anchor.
    let mut svc = services();
    svc.graph_new(&Ctx::new("tester"), "/work/demo", "demo", None, false, None)
        .unwrap();
    let one = add(&mut svc, "segment", "One", &[("step", "1")], None);
    let two = add(&mut svc, "segment", "Two", &[("step", "2")], None);
    let alone = add(
        &mut svc,
        "risk",
        "Dated",
        &[("starts_on", "2026-11-12")],
        None,
    );
    let linked = add(
        &mut svc,
        "risk",
        "Dated and raised",
        &[("starts_on", "2026-11-12")],
        Some(two),
    );
    let decl = json!({
        "name": "steps", "title": "Steps", "main": true,
        "anchors": { "type": "segment" }, "order": ["starts_on", "step"], "place": ["raised_in"],
    });
    svc.graph_spine_set(&ctx(), "demo", &decl).unwrap();
    let (got, info, _) = listed(&svc, &ctx(), along("steps", None, None));
    assert_eq!(got, ids(&[one, two, linked]));
    let shown = svc
        .graph_spine_show(&ctx(), "demo", Some("steps"), false, true, 100, None)
        .unwrap()
        .value
        .unplaced
        .unwrap();
    assert_eq!((shown.ids, shown.total), (vec![alone], info.not_placed));
    let (got, _, _) = listed(&svc, &ctx(), along("steps", Some(two), None));
    assert_eq!(got, ids(&[two, linked]));
}

#[test]
fn paging_counts_in_range_rows_and_the_hints_cover_every_page() {
    let d = day();
    let page = |offset| {
        d.svc
            .node_list(
                &ctx(),
                NodeListRequest {
                    limit: 2,
                    offset,
                    along: Some(along("day", Some(d.segs[0]), Some(d.segs[1]))),
                    ..Default::default()
                },
            )
            .unwrap()
    };
    let mut seen = BTreeSet::new();
    for offset in [0, 2, 4] {
        let p = page(offset);
        assert_eq!(p.value.total, Some(6));
        assert_eq!(p.value.truncated, offset < 4);
        assert_eq!(
            p.value.along.as_ref().unwrap().by_rule,
            1,
            "offset {offset}"
        );
        seen.extend(p.value.rows.iter().map(|n| n.id()));
    }
    let mut want = vec![d.segs[0], d.segs[1]];
    want.extend(d.points[0]);
    want.extend(d.points[1]);
    assert_eq!(seen, ids(&want));
}

#[test]
fn an_end_must_be_an_anchor_of_this_graph_and_the_ends_in_order() {
    let d = day();
    let list = |a: AlongRequest| {
        d.svc.node_list(
            &ctx(),
            NodeListRequest {
                along: Some(a),
                ..Default::default()
            },
        )
    };
    let text = |after: &str| AlongRequest {
        spine: "day".into(),
        after: Some(after.into()),
        before: None,
    };
    assert_eq!(
        code(list(along("day", Some(d.segs[2]), Some(d.segs[1])))),
        "usage_invalid"
    );
    let not_anchor = list(along("day", Some(d.points[0][0]), None)).unwrap_err();
    assert_eq!(not_anchor.code, "along_end_invalid");
    assert_eq!(
        not_anchor.candidates.len(),
        4,
        "the spine's anchors in order"
    );
    assert_eq!(not_anchor.candidates[0].id, d.segs[0].to_text());
    // This graph's address and a unique prefix are ends; another graph's address, the graph itself, a fragment and
    // a repo hint are refused without a lookup; an id no node has is node_not_found, not along_end_invalid.
    let seg = d.segs[1].to_text();
    assert!(list(text(&format!("umx:demo/{seg}"))).is_ok());
    assert!(list(text(&seg.to_lowercase())).is_ok());
    for refused in [
        format!("umx:other/{seg}"),
        "umx:demo".to_string(),
        format!("umx:demo/{seg}#title"),
        format!("umx:demo/{seg}?repo=https://example.invalid/x.git"),
    ] {
        assert_eq!(code(list(text(&refused))), "along_end_invalid", "{refused}");
    }
    assert_eq!(
        code(list(text("01ZZZZZZZZZZZZZZZZZZZZZZZZ"))),
        "node_not_found"
    );
    // A built-in's end is a date.
    let builtin = AlongRequest {
        spine: "recorded".into(),
        after: Some(seg),
        before: None,
    };
    assert_eq!(code(list(builtin)), "along_end_invalid");
}

#[test]
fn a_missing_spine_names_every_spine_the_graph_has() {
    let d = day();
    let err = d
        .svc
        .node_list(
            &ctx(),
            NodeListRequest {
                along: Some(along("week", None, None)),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(err.code, "spine_not_found");
    let names: Vec<&str> = err.candidates.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(names, ["day", "recorded", "status"]);
    // A date field no type declares is no spine either.
    let err = d
        .svc
        .node_list(
            &ctx(),
            NodeListRequest {
                along: Some(along("date_field:due_on", None, None)),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(err.code, "spine_not_found");
}

#[test]
fn along_reads_one_graph_and_content_only() {
    let mut d = day();
    d.svc
        .graph_new(
            &Ctx::new("tester"),
            "/work/other",
            "other",
            None,
            false,
            None,
        )
        .unwrap();
    let ranged = || Some(along("day", None, None));
    let list = |c: &Ctx| {
        d.svc.node_list(
            c,
            NodeListRequest {
                along: ranged(),
                ..Default::default()
            },
        )
    };
    let refusals = [
        Ctx::new("tester").graph("demo,other"),
        Ctx::new("tester").scope(Scope::All {
            include_named: false,
        }),
        Ctx::new("tester").scope(Scope::Saved("both".into())),
        ctx().with_provenance(true),
    ];
    for c in &refusals {
        assert_eq!(code(list(c)), "usage_invalid");
        let edges = d.svc.edge_list(
            c,
            EdgeListRequest {
                along: ranged(),
                ..Default::default()
            },
        );
        assert_eq!(code(edges), "usage_invalid");
        let q = d.svc.query(
            c,
            &CypherRequest {
                text: "MATCH (n) RETURN n.id AS id".into(),
                limit: None,
                offset: None,
                params: Vec::new(),
                along: ranged(),
            },
        );
        assert_eq!(code(q), "usage_invalid");
        let s = d.svc.search_along(c, "point", None, 10, ranged());
        assert_eq!(code(s), "usage_invalid");
        let f = d
            .svc
            .node_find_along(c, "point", None, None, 10, ranged().as_ref());
        assert_eq!(code(f), "usage_invalid");
    }
}

#[test]
fn the_built_ins_compare_dates_by_instant_with_inclusive_ends() {
    let mut svc = services();
    svc.graph_new(&Ctx::new("tester"), "/work/demo", "demo", None, false, None)
        .unwrap();
    svc.ontology_add_type(&ctx(), "task", &["due_on:date".into()], None)
        .unwrap();
    let due = |svc: &mut Services, t: &str, v: &str| add(svc, "task", t, &[("due_on", v)], None);
    let day9 = due(&mut svc, "Nine", "2026-11-09");
    let late9 = due(&mut svc, "Late nine", "2026-11-09T23:30:00Z");
    let ten = due(&mut svc, "Ten", "2026-11-10");
    let month = due(&mut svc, "November", "2026-11");
    let none = add(&mut svc, "task", "Not due", &[], None);
    let builtin = |after: Option<&str>, before: Option<&str>| AlongRequest {
        spine: "date_field:due_on".into(),
        after: after.map(str::to_string),
        before: before.map(str::to_string),
    };
    let (got, info, notes) = listed(&svc, &ctx(), builtin(None, Some("2026-11-09")));
    // The whole of 9 November is on or before it; the month overlaps it, so it is kept.
    assert_eq!(got, ids(&[day9, late9, month]));
    assert_eq!(
        (info.source, info.not_placed, info.coarse),
        (AlongSource::Builtin, 1, 1)
    );
    assert_eq!(
        notes[0].slot("fix").unwrap(),
        "umx graph spine propose demo --unplaced date_field:due_on"
    );
    let (got, _, _) = listed(&svc, &ctx(), builtin(Some("2026-11-10"), None));
    assert_eq!(got, ids(&[ten, month]));
    let (got, info, _) = listed(
        &svc,
        &ctx(),
        builtin(
            Some("2026-11-09T23:00:00+00:00"),
            Some("2026-11-09T23:59:59Z"),
        ),
    );
    assert_eq!(got, ids(&[day9, late9, month]));
    assert_eq!(info.after.as_deref(), Some("2026-11-09T23:00:00Z"));
    assert!(!got.contains(&none));
    // Ends that do not read as dates, and ends out of order, are refused.
    assert_eq!(
        code(svc.node_list(
            &ctx(),
            NodeListRequest {
                along: Some(builtin(Some("soon"), None)),
                ..Default::default()
            }
        )),
        "along_end_invalid"
    );
    assert_eq!(
        code(svc.node_list(
            &ctx(),
            NodeListRequest {
                along: Some(builtin(Some("2026-12"), Some("2026-11"))),
                ..Default::default()
            }
        )),
        "usage_invalid"
    );
    // status: the latest move's time; a node never moved has no place.
    svc.node_status(&ctx(), &ten.to_text(), "active", Some("started"), None)
        .unwrap();
    let (got, info, _) = listed(
        &svc,
        &ctx(),
        AlongRequest {
            spine: "status".into(),
            after: Some("2020".into()),
            before: None,
        },
    );
    assert_eq!((got, info.not_placed), (ids(&[ten]), 4));
    // recorded: every node, by when it was written.
    let (got, info, _) = listed(
        &svc,
        &ctx(),
        AlongRequest {
            spine: "recorded".into(),
            after: None,
            before: Some("2030".into()),
        },
    );
    assert_eq!((got.len(), info.not_placed), (5, 0));
}

/// Rewrites the lines of `ids` in demo's nodes file so the scanner leaves them to the codec (a custom `locator` that
/// is not one), checks it does, and reloads the graph.
fn decoded_by_the_codec(svc: &mut Services, files: &FakeGraphFiles, field: &str, ids: &[Id]) {
    let dir = GraphDir::parse("/work/demo").unwrap();
    let text = String::from_utf8(files.raw_for_tests(&dir, GraphFile::Nodes).unwrap()).unwrap();
    let mut out = String::new();
    for line in text.lines() {
        if ids
            .iter()
            .any(|id| line.contains(&format!("\"{}\"", id.to_text())))
        {
            let line = line.strip_suffix('}').unwrap();
            out.push_str(&format!("{line},\"locator\":7}}\n"));
        } else {
            out.push_str(&format!("{line}\n"));
        }
    }
    let g = umx_graph::Graph::load_nodes(out.clone().into_bytes())
        .unwrap()
        .0;
    let f = g.field_id(field).unwrap();
    for id in ids {
        let node = g.node_at(g.node_ix(*id).unwrap());
        assert!(
            matches!(node.field_ref(f), Some(FieldRef::Other(_))) && node.date_at(f).is_some(),
            "{id:?} is a date the codec decodes"
        );
    }
    files.tamper_for_tests(&dir, GraphFile::Nodes, out.as_bytes());
    svc.graph_load(&ctx(), "/work/demo", None).unwrap();
}

#[test]
fn a_date_the_codec_decodes_is_read_as_one_the_scanner_reads() {
    // kingfisher B1: a line the scanner leaves to the codec holds its date as a decoded value, not scanned text. The
    // built-in and place() (an anchor's order key, a calendar node's own date) read the typed time column instead.
    let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let mut svc = services_over(files.clone());
    svc.graph_new(&Ctx::new("tester"), "/work/demo", "demo", None, false, None)
        .unwrap();
    svc.ontology_add_type(&ctx(), "task", &["due_on:date".into()], None)
        .unwrap();
    let task = add(&mut svc, "task", "Due", &[("due_on", "2026-11-09")], None);
    decoded_by_the_codec(&mut svc, &files, "due_on", &[task]);
    // The only holder is decoded: the built-in exists and places it.
    let builtin = AlongRequest {
        spine: "date_field:due_on".into(),
        after: Some("2026-11-09".into()),
        before: Some("2026-11-09".into()),
    };
    let (got, info, _) = listed(&svc, &ctx(), builtin);
    assert_eq!((got, info.not_placed), (ids(&[task]), 0));

    let seg =
        |svc: &mut Services, t: &str, at: &str| add(svc, "segment", t, &[("starts_on", at)], None);
    let one = seg(&mut svc, "One", "2026-11-11");
    let two = seg(&mut svc, "Two", "2026-11-13");
    let dated = add(
        &mut svc,
        "risk",
        "Dated",
        &[("starts_on", "2026-11-12")],
        None,
    );
    let decl = json!({
        "name": "day", "title": "The day", "main": true,
        "anchors": { "type": "segment" }, "order": ["starts_on"], "place": ["raised_in"],
    });
    svc.graph_spine_set(&ctx(), "demo", &decl).unwrap();
    let before = (
        listed(&svc, &ctx(), along("day", Some(one), Some(one))).0,
        listed(&svc, &ctx(), along("day", Some(two), None)).0,
    );
    assert_eq!(before, (ids(&[one, dated]), ids(&[two])));
    // The second anchor's key and the dated risk's own date decoded: the same range.
    decoded_by_the_codec(&mut svc, &files, "starts_on", &[two, dated]);
    let after = (
        listed(&svc, &ctx(), along("day", Some(one), Some(one))).0,
        listed(&svc, &ctx(), along("day", Some(two), None)).0,
    );
    assert_eq!(after, before);
}

#[test]
fn edge_list_keeps_the_links_inside_the_range() {
    let d = day();
    let done = d
        .svc
        .edge_list(
            &ctx(),
            EdgeListRequest {
                along: Some(along("day", Some(d.segs[1]), Some(d.segs[1]))),
                ..Default::default()
            },
        )
        .unwrap();
    let ends: BTreeSet<(Id, Id)> = done
        .value
        .rows
        .iter()
        .map(|r| (r.edge.from, r.edge.to))
        .collect();
    assert_eq!(
        ends,
        BTreeSet::from([(d.points[1][0], d.segs[1]), (d.points[1][1], d.segs[1])])
    );
    let info = done.value.along.unwrap();
    // The ruled node's link has an end placed only by rule; the other raised_in links are out of range.
    assert_eq!((info.by_rule, info.not_placed), (1, 0));
    assert_eq!(
        done.notes.iter().map(|p| p.code).collect::<Vec<_>>(),
        ["along_by_rule"]
    );
}

fn query(
    svc: &Services,
    text: &str,
    a: Option<AlongRequest>,
    params: &[(&str, Id)],
) -> umx_services::QueryRows {
    svc.query(
        &ctx(),
        &CypherRequest {
            text: text.into(),
            limit: None,
            offset: None,
            params: params
                .iter()
                .map(|(n, id)| (n.to_string(), SetValue::Str(id.to_text())))
                .collect(),
            along: a,
        },
    )
    .unwrap()
    .value
}

fn strings(rows: &umx_services::QueryRows) -> Vec<String> {
    rows.rows
        .iter()
        .map(|r| match &r[0] {
            Cell::Str(s) => s.clone(),
            Cell::Null => "null".into(),
            Cell::Int(i) => i.to_string(),
            other => format!("{other:?}"),
        })
        .collect()
}

#[test]
fn a_query_sees_the_induced_subgraph_of_the_range() {
    let mut d = day();
    // Two points of the fourth segment joined only through a point of the first (out of range below).
    let (x1, x2, m) = (d.points[3][0], d.points[3][1], d.points[0][0]);
    link(&mut d.svc, x1, "relates_to", m);
    link(&mut d.svc, m, "relates_to", x2);
    let ranged = || Some(along("day", Some(d.segs[2]), None));
    // C5: the query's nodes are node list's rows for the same range.
    let rows = query(&d.svc, "MATCH (n) RETURN n.id AS id", ranged(), &[]);
    let listed_ids: BTreeSet<String> = listed(&d.svc, &ctx(), ranged().unwrap())
        .0
        .iter()
        .map(|id| id.to_text())
        .collect();
    assert_eq!(
        strings(&rows).into_iter().collect::<BTreeSet<_>>(),
        listed_ids
    );
    let info = rows.along.unwrap();
    // C3: graph-wide counts.
    assert_eq!((info.not_placed, info.by_rule), (1, 1));
    // C2: no path goes through an out-of-range node.
    let path = "MATCH (a {id: $a})-[:relates_to*2]-(b {id: $b}) RETURN count(*) AS n";
    let through = |a| strings(&query(&d.svc, path, a, &[("a", x1), ("b", x2)]));
    assert_eq!(through(None), ["1"]);
    assert_eq!(through(ranged()), ["0"]);
    // OPTIONAL MATCH gives null for an out-of-range neighbour.
    let opt = "MATCH (a {id: $a}) OPTIONAL MATCH (a)-[:relates_to]->(b) RETURN b.id AS id";
    assert_eq!(
        strings(&query(&d.svc, opt, ranged(), &[("a", x1)])),
        ["null"]
    );
    // C1: a label only outside the range is not an unknown label: the name checks see the whole graph.
    add(
        &mut d.svc,
        "task",
        "Only in the first segment",
        &[],
        Some(d.segs[0]),
    );
    let done = d
        .svc
        .query(
            &ctx(),
            &CypherRequest {
                text: "MATCH (n:task) RETURN n.id AS id".into(),
                limit: None,
                offset: None,
                params: Vec::new(),
                along: ranged(),
            },
        )
        .unwrap();
    assert!(done.value.rows.is_empty());
    let warned: Vec<&str> = done.notes.iter().map(|p| p.code).collect();
    assert!(!warned.contains(&"query_unknown_label"), "{warned:?}");
}

#[test]
fn node_find_keeps_only_matches_in_range() {
    let d = day();
    let found = d
        .svc
        .node_find_along(
            &ctx(),
            "Point",
            None,
            None,
            50,
            Some(&along("day", Some(d.segs[2]), None)),
        )
        .unwrap();
    let got: BTreeSet<Id> = found.value.rows.iter().map(|(n, _)| n.id).collect();
    let mut want = d.points[2].to_vec();
    want.extend(d.points[3]);
    assert_eq!(got, ids(&want));
    assert!(found.notes.iter().all(|p| p.code != "find_no_match"));
}

#[test]
fn search_narrows_every_leg_before_fusion() {
    // graph-storage: 120 strong out-of-range matches would fill a leg's LEG_DEPTH; narrowed first, the 15 weaker
    // in-range ones are all found, exactly as a search over that set built by hand.
    let mut svc = services();
    svc.graph_new(&Ctx::new("tester"), "/work/demo", "demo", None, false, None)
        .unwrap();
    let early = add(
        &mut svc,
        "segment",
        "Early",
        &[("starts_on", "2026-11-01T09:00:00Z")],
        None,
    );
    let late = add(
        &mut svc,
        "segment",
        "Late",
        &[("starts_on", "2026-11-02T09:00:00Z")],
        None,
    );
    for i in 0..120 {
        add(
            &mut svc,
            "risk",
            &format!("alpha alpha alpha {i}"),
            &[],
            Some(early),
        );
    }
    let wanted: BTreeSet<Id> = (0..15)
        .map(|i| {
            add(
                &mut svc,
                "risk",
                &format!("alpha gamma delta epsilon {i}"),
                &[],
                Some(late),
            )
        })
        .collect();
    let decl = json!({
        "name": "day", "title": "The day", "main": false,
        "anchors": { "type": "segment", "inside": null },
        "order": ["starts_on"], "place": ["raised_in"],
    });
    svc.graph_spine_set(&ctx(), "demo", &decl).unwrap();
    const { assert!(umx_services::LEG_DEPTH < 120) };
    let done = svc
        .search_along(
            &ctx(),
            "alpha",
            None,
            50,
            Some(along("day", Some(late), None)),
        )
        .unwrap();
    let got: BTreeSet<Id> = done.value.hits.iter().map(|h| h.node.id).collect();
    assert_eq!(got, wanted);
    let info = done.value.along.unwrap();
    assert_eq!((info.not_placed, info.by_rule), (0, 0));
}

#[test]
fn a_declaration_may_not_take_a_built_in_name() {
    let mut d = day();
    for name in ["recorded", "status", "date_field:due_on"] {
        let decl = json!({
            "name": name, "title": "x", "main": false,
            "anchors": { "type": "segment", "inside": "part_of" },
            "order": ["starts_on"], "place": ["raised_in"],
        });
        let err = d.svc.graph_spine_set(&ctx(), "demo", &decl).unwrap_err();
        // date_field:due_on is not a plain name, so the shape check refuses it first.
        assert!(
            ["spine_name_reserved", "usage_invalid"].contains(&err.code),
            "{name}: {err:?}"
        );
    }
    let decl = json!({
        "name": "recorded", "title": "x", "main": false,
        "anchors": { "type": "segment", "inside": "part_of" },
        "order": ["starts_on"], "place": ["raised_in"],
    });
    let err = d.svc.graph_spine_set(&ctx(), "demo", &decl).unwrap_err();
    assert_eq!(err.code, "spine_name_reserved");
    assert_eq!(err.slot("suggest").unwrap(), "recorded_spine");
}

/// `node list --columns spine:<name>`: each node's band in the spine's own order and how it was placed, from the one
/// placement rule, whatever order the ids come in.
#[test]
fn spine_places_give_each_node_its_band_and_how() {
    use umx_services::{SpineBy, SpinePlace};
    let d = day();
    let mut ids = vec![d.alone, d.ruled, d.early, d.points[3][1], d.points[0][0]];
    ids.extend(d.segs.iter().rev());
    ids.push(Id(u128::MAX));
    let at = |band: Option<i64>, by: SpineBy, anchor: bool| SpinePlace { band, by, anchor };
    let places = d.svc.spine_places("demo", "day", &ids).unwrap();
    assert_eq!(
        places,
        vec![
            at(None, SpineBy::NotPlaced, false),
            at(Some(1), SpineBy::Rule, false),
            at(Some(-1), SpineBy::Field, false),
            at(Some(3), SpineBy::Link, false),
            at(Some(0), SpineBy::Link, false),
            at(Some(3), SpineBy::Field, true),
            at(Some(2), SpineBy::Field, true),
            at(Some(1), SpineBy::Field, true),
            at(Some(0), SpineBy::Field, true),
            at(None, SpineBy::NotPlaced, false),
        ]
    );
    assert_eq!(d.svc.spine_places("demo", "nope", &ids), None);
    assert_eq!(d.svc.spine_places("other", "day", &ids), None);
}

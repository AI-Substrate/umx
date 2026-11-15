//! Word search (req-68; workshop 008 section 11): BM25F arithmetic, ranking, determinism across machines and load
//! orders, and the index staying equal to a fresh load through every kind of write.

use std::collections::BTreeMap;
use umx_core::ontology::{Ontology, TextFields};
use umx_core::{Edge, Id, Node};
use umx_graph::{Form, Graph, PREFIX_CAP, WordQuery, WordStats};

fn id(n: u128) -> Id {
    Id((0x0194_0000_0000_u128 << 80) | n)
}

fn node(n: u128, title: &str, note: &str) -> Node {
    Node {
        id: id(n),
        kind: "problem".into(),
        title: title.into(),
        status: "open".into(),
        note: note.into(),
        evidence: vec![],
        status_history: vec![],
        created: "2026-11-10T01:00:00Z".into(),
        updated: "2026-11-10T01:00:00Z".into(),
        created_by: "tester".into(),
        locator: None,
        authored_by_session: None,
        edited_by_session: None,
        fields: BTreeMap::new(),
    }
}

fn with(mut n: Node, field: &str, value: &str) -> Node {
    n.fields.insert(field.into(), value.into());
    n
}

fn part_of(n: u128, child: u128, parent: u128) -> Edge {
    Edge {
        id: id(n),
        kind: "part_of".into(),
        from_graph: None,
        from: id(child),
        to_graph: None,
        to: id(parent),
        created: "2026-11-10T01:00:00Z".into(),
        created_by: "tester".into(),
        authored_by_session: None,
        edited_by_session: None,
        evidence: Vec::new(),
        fields: BTreeMap::new(),
    }
}

/// (node number, score, field) for every hit of `query` over `graphs`, figures summed across them.
fn search(graphs: &[&Graph], query: &str) -> Vec<(u128, f64, String)> {
    let q = WordQuery::new(query).unwrap();
    let mut stats = WordStats::default();
    for g in graphs {
        stats.add(&g.word_stats(&q));
    }
    let mut out = Vec::new();
    for g in graphs {
        for (n, score, field) in g.word_search(&q, &stats, None, 100) {
            out.push((n.id().0 & 0xffff, score, field.to_string()));
        }
    }
    out
}

fn score_of(hits: &[(u128, f64, String)], n: u128) -> f64 {
    hits.iter().find(|h| h.0 == n).map_or(0.0, |h| h.1)
}

#[test]
fn bm25f_scores_match_the_hand_arithmetic() {
    let g = Graph::from_records(
        vec![
            node(1, "Approval delay", ""),
            node(2, "Launch", "approval delay approval"),
            node(3, "Budget", "delay"),
        ],
        vec![],
    );
    // N = 3. title: 3 nodes, 4 words, avg 4/3. note: 2 nodes (2, 3), 4 words, avg 2.
    // df(approval) = 2 (1 title, 2 note): idf = ln(1 + 1.5/2.5) = ln 1.6 = 0.470003629245736
    // df(delay) = 3: idf = ln(1 + 0.5/3.5) = ln(8/7) = 0.133531392624523
    // Node 1, title len 2: norm = 0.25 + 0.75 * 2 / (4/3) = 1.375; each word tf' = 3 * 1 / 1.375 = 24/11,
    //   tf'/(1.2 + tf') = 24/37.2; score = (ln 1.6 + ln 8/7) * 24/37.2 = 0.389377433464683
    // Node 2, note len 3: norm = 0.25 + 0.75 * 3/2 = 1.375; approval tf' = 1.5 * 2 / 1.375 = 24/11 -> 24/37.2;
    //   delay tf' = 1.5 / 1.375 = 12/11 -> 12/25.2; score = ln 1.6 * 24/37.2 + ln 8/7 * 12/25.2 = 0.366814525340723
    // Node 3, note len 1: norm = 0.25 + 0.75 / 2 = 0.625; delay tf' = 1.5 / 0.625 = 2.4 -> 2.4/3.6;
    //   score = ln 8/7 * 2/3 = 0.089020928416348
    let hits = search(&[&g], "approval delay");
    let expected = [
        (1, 0.389_377_433_464_682_7, "title"),
        (2, 0.366_814_525_340_723_45, "note"),
        (3, 0.089_020_928_416_348_39, "note"),
    ];
    assert_eq!(hits.len(), 3);
    for (hit, (n, score, field)) in hits.iter().zip(expected) {
        assert_eq!(hit.0, n);
        assert!(
            (hit.1 - score).abs() < 1e-9,
            "node {n}: {} vs {score}",
            hit.1
        );
        assert_eq!(hit.2, field);
    }
}

#[test]
fn a_note_match_is_found_and_a_title_match_outranks_it() {
    let g = Graph::from_records(
        vec![
            node(
                1,
                "Vendor onboarding",
                "Legal sign-off: the approval delay hits every vendor.",
            ),
            node(2, "Approval delay", ""),
            node(3, "Launch date", "Nothing to see."),
        ],
        vec![],
    );
    let hits = search(&[&g], "approval delay");
    assert_eq!(
        hits.iter().map(|h| (h.0, h.2.as_str())).collect::<Vec<_>>(),
        vec![(2, "title"), (1, "note")]
    );
    // Scores repeat exactly.
    assert_eq!(
        hits.iter().map(|h| h.1.to_bits()).collect::<Vec<_>>(),
        search(&[&g], "delay approval")
            .iter()
            .map(|h| h.1.to_bits())
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_rare_word_outranks_a_common_one() {
    let nodes = (1..=100)
        .map(|n| {
            let title = match n {
                1 => "rare",
                2..=51 => "common",
                _ => "filler",
            };
            node(n, title, "")
        })
        .collect();
    let g = Graph::from_records(nodes, vec![]);
    let hits = search(&[&g], "rare common");
    assert_eq!(hits[0].0, 1);
    assert!(score_of(&hits, 1) > score_of(&hits, 2));
}

#[test]
fn title_weight_dominates_and_a_second_field_does_not_double() {
    let g = Graph::from_records(
        vec![
            node(1, "kite", "kite"),
            node(2, "kite", "other"),
            node(3, "zebra", "kite kite kite"),
            node(4, "filler", "words here"),
        ],
        vec![],
    );
    let hits = search(&[&g], "kite");
    let (both, title, note) = (score_of(&hits, 1), score_of(&hits, 2), score_of(&hits, 3));
    assert!(both > title, "{both} {title}");
    assert!(
        both < 1.5 * title,
        "a second field must not double: {both} {title}"
    );
    assert!(title > note, "title over a note-only node: {title} {note}");
}

#[test]
fn ties_go_to_the_lower_handle_and_a_small_ranking_is_pinned() {
    let g = Graph::from_records(
        vec![
            node(5, "beta gamma", ""),
            node(2, "alpha beta", ""),
            node(9, "alpha beta", ""),
            node(7, "gamma", "alpha"),
        ],
        vec![],
    );
    let order: Vec<u128> = search(&[&g], "alpha beta").iter().map(|h| h.0).collect();
    assert_eq!(order, vec![2, 9, 5, 7]);
}

#[test]
fn numbers_and_mixed_tokens_are_words() {
    let g = Graph::from_records(
        vec![
            node(1, "ISO27001 audit", "v2 of the plan, 2026"),
            node(2, "Audit", ""),
        ],
        vec![],
    );
    assert_eq!(search(&[&g], "iso27001")[0].0, 1);
    assert_eq!(search(&[&g], "V2 2026")[0].0, 1);
    assert_eq!(
        umx_graph::words("Ünïcode-Mixed v2"),
        vec!["ünïcode", "mixed", "v2"]
    );
}

/// Two graphs where field numbers differ by history: `a` meets `zeta` first, `b` meets `alpha` first.
fn two_histories() -> (Graph, Graph) {
    let fields = TextFields::strings(["alpha", "zeta"]);
    let n1 = with(node(1, "Kite design", "wind tunnel"), "zeta", "kite tail");
    let n2 = with(node(2, "Tail fin", "kite"), "alpha", "kite string");
    let a = Graph::from_records_indexing(vec![n1.clone(), n2.clone()], vec![], fields.clone());
    let mut b = Graph::from_records_indexing(vec![node(1, "x", "")], vec![], fields);
    b.insert_node(n2).expect("the image fits");
    b.insert_node(n1).expect("the image fits");
    assert_eq!(a.word_index_image(), b.word_index_image());
    (a, b)
}

#[test]
fn scores_are_bit_identical_whatever_the_load_order_or_field_numbering() {
    let (a, b) = two_histories();
    let bits = |hits: Vec<(u128, f64, String)>| {
        hits.into_iter()
            .map(|h| (h.0, h.1.to_bits(), h.2))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        bits(search(&[&a], "kite tail")),
        bits(search(&[&b], "kite tail"))
    );
    let other = Graph::from_records(
        vec![node(3, "Kite shop", "tail"), node(4, "Paper", "kite")],
        vec![],
    );
    let mut g1_g2 = bits(search(&[&a, &other], "kite tail"));
    let mut g2_g1 = bits(search(&[&other, &a], "kite tail"));
    g1_g2.sort();
    g2_g1.sort();
    assert_eq!(g1_g2, g2_g1);
    // Figures add up across graphs: a word rare in one graph is judged over both.
    assert_ne!(
        bits(search(&[&a], "kite tail")),
        bits(search(&[&a, &other], "kite tail"))[..2].to_vec()
    );
}

#[test]
fn a_scope_limits_the_nodes_scored_never_the_figures() {
    let g = Graph::from_records(
        vec![
            node(1, "Root", ""),
            node(2, "Approval delay", ""),
            node(3, "Approval", "delay"),
            node(4, "Delay elsewhere", ""),
        ],
        vec![part_of(10, 2, 1), part_of(11, 3, 1)],
    );
    let q = WordQuery::new("approval delay").unwrap();
    let stats = g.word_stats(&q);
    let all: BTreeMap<Id, u64> = g
        .word_search(&q, &stats, None, 10)
        .into_iter()
        .map(|(n, s, _)| (n.id(), s.to_bits()))
        .collect();
    let within = g.within(id(1));
    let scoped = g.word_search(&q, &stats, Some(&within), 10);
    assert_eq!(scoped.len(), 2);
    for (n, s, _) in scoped {
        assert!(within.contains(n.id()));
        assert_eq!(all[&n.id()], s.to_bits());
    }
}

#[test]
fn declared_text_fields_are_indexed_and_a_change_reindexes() {
    let n = with(node(1, "Pump", ""), "detail", "cavitation noise");
    let mut g = Graph::from_records(vec![n, node(2, "Valve", "")], vec![]);
    assert!(search(&[&g], "cavitation").is_empty());
    g.set_text_fields(TextFields::strings(["detail"]));
    assert_eq!(search(&[&g], "cavitation")[0].2, "detail");
    g.set_text_fields(TextFields::default());
    assert!(search(&[&g], "cavitation").is_empty());
}

/// A tiny deterministic generator (no rand dependency).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        (self.0 >> 33) % n
    }
}

#[test]
fn the_index_equals_a_fresh_load_after_mixed_writes() {
    // Stem classes (vendor, vendors; approve, approval, approved) and prefixes are part of the image (s21).
    const WORDS: [&str; 11] = [
        "kite", "tail", "wind", "delay", "approval", "v2", "iso27001", "vendor", "vendors",
        "approve", "approved",
    ];
    let mut r = Lcg(7);
    let text = |r: &mut Lcg, n: u64| {
        (0..r.next(n))
            .map(|_| WORDS[r.next(WORDS.len() as u64) as usize])
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut other = false;
    let fields_of = |other: bool| {
        if other {
            TextFields::strings(["detail", "other"])
        } else {
            TextFields::strings(["detail"])
        }
    };
    let mut g = Graph::from_records_indexing(vec![], vec![], fields_of(other));
    let mut next = 1000u128;
    for step in 0..400 {
        let ids: Vec<u128> = g.nodes().map(|n| n.id().0 & 0xffff).collect();
        match r.next(10) {
            // Mostly appends; sometimes an id below the last (the rebuild path).
            0..=3 => {
                next += 1;
                let mut n = node(next, &text(&mut r, 4), &text(&mut r, 12));
                n = with(
                    n,
                    if r.next(2) == 0 { "detail" } else { "other" },
                    &text(&mut r, 5),
                );
                g.insert_node(n).expect("the image fits");
            }
            4 => {
                let low = 1 + r.next(999) as u128;
                if !ids.contains(&low) {
                    g.insert_node(node(low, &text(&mut r, 4), &text(&mut r, 6)))
                        .expect("the image fits");
                }
            }
            // Replace a node's texts.
            5..=7 if !ids.is_empty() => {
                let n = ids[r.next(ids.len() as u64) as usize];
                g.insert_node(with(
                    node(n, &text(&mut r, 4), &text(&mut r, 8)),
                    "detail",
                    &text(&mut r, 3),
                ))
                .expect("the image fits");
            }
            // Remove the last node, or one in the middle (the rebuild path).
            8 if !ids.is_empty() => {
                let n = if r.next(2) == 0 {
                    *ids.last().unwrap()
                } else {
                    ids[r.next(ids.len() as u64) as usize]
                };
                g.remove_node(id(n));
            }
            9 => {
                other = !other;
                g.set_text_fields(fields_of(other));
            }
            _ => {}
        }
        let records: Vec<Node> = g.nodes().map(|n| n.to_node()).collect();
        let fresh = Graph::from_records_indexing(records, vec![], fields_of(other));
        assert_eq!(
            g.word_index_image(),
            fresh.word_index_image(),
            "after step {step}"
        );
    }
    assert!(g.node_count() > 50);
}

#[test]
fn many_text_fields_widen_and_are_never_merged() {
    let names: Vec<String> = (0..300).map(|i| format!("f{i:03}")).collect();
    let mut n = node(1, "Wide", "");
    for (i, name) in names.iter().enumerate() {
        n = with(n, name, &format!("word{i}"));
    }
    let g =
        Graph::from_records_indexing(vec![n], vec![], TextFields::strings(names.iter().cloned()));
    for i in [0, 63, 64, 299] {
        let hits = search(&[&g], &format!("word{i}"));
        assert_eq!(hits[0].2, names[i]);
    }
}

fn typed(n: u128, kind: &str, title: &str, detail: &str) -> Node {
    let mut node = with(node(n, title, ""), "detail", detail);
    node.kind = kind.into();
    node
}

/// R83-1: a field is text per node type, by the rule a field's kind follows: a type's own declaration first, else the
/// first type that declares it. A date field stays out of the index though another type's field of that name is text.
#[test]
fn text_fields_are_chosen_per_node_type() {
    let ontology: Ontology = serde_json::from_value(serde_json::json!({
        "types": [
            { "name": "problem", "fields": [{ "name": "detail", "kind": "string" }] },
            { "name": "decision", "fields": [{ "name": "detail", "kind": "date" }] },
            { "name": "risk", "fields": [] }
        ],
        "statuses": [],
        "relations": []
    }))
    .unwrap();
    let fields = ontology.text_fields();
    assert!(fields.is_text("problem", "detail"));
    assert!(!fields.is_text("decision", "detail"));
    // Not declared on risk: the first declaring type (problem) says string.
    assert!(fields.is_text("risk", "detail"));
    assert!(!fields.is_text("problem", "owner"));
    let mut g = Graph::from_records_indexing(
        vec![
            typed(1, "problem", "Narrative", "launchword"),
            typed(2, "decision", "Calendar", "2026-11-10"),
            typed(3, "risk", "Slip", "riskword"),
        ],
        vec![],
        fields,
    );
    assert_eq!(search(&[&g], "launchword")[0].2, "detail");
    assert_eq!(search(&[&g], "riskword")[0].2, "detail");
    assert!(search(&[&g], "2026").is_empty());
    // A node whose type changes to one that declares the field as a date leaves the index for that field.
    g.insert_node(typed(1, "decision", "Narrative", "launchword"))
        .expect("the image fits");
    assert!(search(&[&g], "launchword").is_empty());
    g.insert_node(typed(1, "problem", "Narrative", "launchword"))
        .expect("the image fits");
    assert_eq!(search(&[&g], "launchword")[0].0, 1);
    g.check_rows();
}

/// R83-4: words a write removes leave the index, so a long edit history does not grow it. One node's title replaced
/// 5,000 times with a new word each time holds about what a fresh load of the final graph holds.
#[test]
fn replaced_words_are_reclaimed_so_edits_do_not_grow_the_index() {
    let mut g = Graph::from_records(
        vec![node(1, "start", "steady note"), node(2, "other", "")],
        vec![],
    );
    for i in 0..5000 {
        g.insert_node(node(1, &format!("word{i}"), "steady note"))
            .expect("the image fits");
    }
    let records: Vec<Node> = g.nodes().map(|n| n.to_node()).collect();
    let fresh = Graph::from_records(records, vec![]);
    let (churned, loaded) = (g.memory_bytes().word_index, fresh.memory_bytes().word_index);
    assert!(
        churned <= 2 * loaded + 512,
        "{churned} after 5,000 edits, {loaded} fresh"
    );
    assert_eq!(g.word_index_image(), fresh.word_index_image());
    assert!(search(&[&g], "word0").is_empty());
    assert_eq!(search(&[&g], "word4999")[0].0, 1);
}

// ws004 s21 (req-92): stems and prefixes.

/// (node number, score, [(query word, term, form)], held): one hit of [`matched`].
type Matched = (u128, f64, Vec<(String, String, Form)>, f64);

/// Every hit of `query`, every query word marked, with the marked words each hit holds.
fn matched(graphs: &[&Graph], query: &str) -> Vec<Matched> {
    let q = WordQuery::new(query).unwrap();
    let mut stats = WordStats::default();
    for g in graphs {
        stats.add(&g.word_stats(&q));
    }
    let marks = vec![true; q.terms().len()];
    let mut out = Vec::new();
    for g in graphs {
        for hit in g.word_search_marked(&q, &stats, None, 100, &marks) {
            let terms = hit
                .terms
                .into_iter()
                .map(|t| (t.query, t.term, t.form))
                .collect();
            out.push((hit.node.id().0 & 0xffff, hit.score, terms, hit.held));
        }
    }
    out
}

fn numbers(hits: &[(u128, f64, String)]) -> Vec<u128> {
    hits.iter().map(|h| h.0).collect()
}

/// A tester's friction: `vendor` missed "Pre-approve listed vendors" and `vendors` found only the plural.
#[test]
fn vendor_finds_vendors_and_vendors_finds_vendor() {
    let g = Graph::from_records(
        vec![
            node(1, "Pre-approve listed vendors", ""),
            node(2, "Vendor onboarding", ""),
            node(3, "Launch date", ""),
        ],
        vec![],
    );
    assert_eq!(numbers(&search(&[&g], "vendor")), [2, 1]);
    assert_eq!(numbers(&search(&[&g], "vendors")), [1, 2]);
    let hits = matched(&[&g], "vendor");
    assert_eq!(
        hits[1].2,
        [("vendor".to_string(), "vendors".to_string(), Form::Stem)]
    );
    assert_eq!(
        hits[0].2,
        [("vendor".to_string(), "vendor".to_string(), Form::Exact)]
    );
}

/// A tester's friction: `approv` found nothing. It is approve's and approval's Porter2 stem, and a prefix.
#[test]
fn approv_finds_approve_approval_and_pre_approve() {
    let g = Graph::from_records(
        vec![
            node(1, "Approve the budget", ""),
            node(2, "Approval delay", ""),
            node(3, "Pre-approve listed vendors", ""),
            node(4, "Approximate costs", ""),
            node(5, "Launch date", "approved last week"),
        ],
        vec![],
    );
    let mut found = numbers(&search(&[&g], "approv"));
    found.sort();
    assert_eq!(found, [1, 2, 3, 5]);
    // A longer prefix than the stem meets only the words that start with it.
    let hits = matched(&[&g], "approva");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0, 2);
    assert_eq!(
        hits[0].2,
        [("approva".to_string(), "approval".to_string(), Form::Prefix)]
    );
}

/// Same-shaped nodes: the word itself, another form of it, a word that only starts with it, by the expert's
/// weights (hand arithmetic). N = 4, every title one word (avg 1, norm 1, tf' = 3); each term in one node, so each
/// tier's df is 1 and idf = ln(1 + 3.5 / 1.5) = 1.2039728043259361; tf'/(1.2 + tf') = 3 / 4.2.
#[test]
fn exact_outranks_stem_outranks_prefix_by_the_tier_weights() {
    let g = Graph::from_records(
        vec![
            node(1, "vendorship", ""),
            node(2, "vendors", ""),
            node(3, "vendor", ""),
            node(4, "other", ""),
        ],
        vec![],
    );
    let hits = matched(&[&g], "vendor");
    let expected = [
        (3, 0.859_980_574_518_525_8, "vendor", Form::Exact),
        (2, 0.601_986_402_162_968, "vendors", Form::Stem),
        (1, 0.343_992_229_807_410_34, "vendorship", Form::Prefix),
    ];
    assert_eq!(hits.len(), 3);
    for (hit, (n, score, term, form)) in hits.iter().zip(expected) {
        assert_eq!(hit.0, n);
        assert!(
            (hit.1 - score).abs() < 1e-12,
            "node {n}: {} vs {score}",
            hit.1
        );
        assert_eq!(hit.2, [("vendor".to_string(), term.to_string(), form)]);
    }
    // Coverage credit: an exact or stem match holds the word (1), a prefix match only partly.
    let held: Vec<f64> = hits.iter().map(|h| h.3).collect();
    assert_eq!(held, [1.0, 1.0, umx_graph::PREFIX_WEIGHT]);
}

#[test]
fn a_word_under_four_letters_is_not_a_prefix() {
    let g = Graph::from_records(
        vec![node(1, "Apple pie", ""), node(2, "App store", "")],
        vec![],
    );
    assert_eq!(numbers(&search(&[&g], "app")), [2]);
    let mut found = numbers(&search(&[&g], "appl"));
    found.sort();
    assert_eq!(found, [1]);
}

/// req-92: words with digits or other letters match exactly only (no stem, no prefix).
#[test]
fn words_with_digits_or_other_letters_match_exactly_only() {
    let g = Graph::from_records(
        vec![
            node(1, "ISO27001 audit", ""),
            node(2, "Café openings", ""),
            node(3, "cafes", ""),
        ],
        vec![],
    );
    assert!(search(&[&g], "iso2700").is_empty());
    assert_eq!(numbers(&search(&[&g], "iso27001")), [1]);
    assert_eq!(numbers(&search(&[&g], "café")), [2]);
    // The query word is not ASCII, so it does not prefix-match `cafes`, and `cafe` stems only ASCII words.
    assert_eq!(numbers(&search(&[&g], "cafe")), [3]);
}

/// R186-2: a word with digits or other letters is no prefix target either. `plan` meets plans and planning by
/// stem; plan2026 and planètes need their full tokens, and take no slot under the prefix cap.
#[test]
fn a_prefix_expands_only_into_words_made_of_ascii_letters() {
    let g = Graph::from_records(
        vec![
            node(1, "plan", ""),
            node(2, "plans", ""),
            node(3, "planning", ""),
            node(4, "plan2026", ""),
            node(5, "planètes", ""),
            node(6, "Filler", ""),
        ],
        vec![],
    );
    let forms: Vec<(u128, String, Form)> = matched(&[&g], "plan")
        .into_iter()
        .map(|h| (h.0, h.2[0].1.clone(), h.2[0].2))
        .collect();
    assert_eq!(
        forms,
        [
            (1, "plan".to_string(), Form::Exact),
            (2, "plans".to_string(), Form::Stem),
            (3, "planning".to_string(), Form::Stem)
        ]
    );
    let q = WordQuery::new("plan").unwrap();
    let stats = g.word_stats(&q);
    let targets: Vec<&str> = stats.variants[0]
        .iter()
        .filter(|(_, v)| v.prefix)
        .map(|(w, _)| w.as_str())
        .collect();
    assert_eq!(targets, ["plan", "planning", "plans"]);
    assert_eq!(numbers(&search(&[&g], "plan2026")), [4]);
    assert_eq!(numbers(&search(&[&g], "planètes")), [5]);
}

/// 70 words start with `plank`: the 64 most common are searched (df, then string order), and the cap is reported.
fn plank_nodes() -> Vec<Node> {
    let word = |i: u128| {
        let a = (b'a' + (i / 26) as u8) as char;
        let b = (b'a' + (i % 26) as u8) as char;
        format!("plank{a}{b}")
    };
    let mut nodes: Vec<Node> = (0..70).map(|i| node(100 + i, &word(i), "")).collect();
    // The last eight words are twice as common, so they are kept though last in string order.
    nodes.extend((62..70).map(|i| node(200 + i, &word(i), "")));
    nodes.push(node(1, "Filler", ""));
    nodes
}

#[test]
fn the_prefix_cap_holds_and_is_reported() {
    let g = Graph::from_records(plank_nodes(), vec![]);
    let q = WordQuery::new("plank").unwrap();
    let stats = g.word_stats(&q);
    assert_eq!(stats.capped(&q), [("plank", 70)]);
    let mut found: Vec<u128> = numbers(&search(&[&g], "plank"));
    found.sort();
    // Kept: the eight common words (nodes 162..170 and 262..270), then 56 more in string order (nodes 100..156).
    let mut want: Vec<u128> = (100..156).chain(162..170).chain(262..270).collect();
    want.sort();
    assert_eq!(found.len(), PREFIX_CAP + 8);
    assert_eq!(found, want);
    // One letter more and nothing is capped.
    assert!(
        g.word_stats(&WordQuery::new("plankb").unwrap())
            .capped(&WordQuery::new("plankb").unwrap())
            .is_empty()
    );
}

/// The capped expansion and its scores are the same, to the bit, whatever the load order or graph split.
#[test]
fn a_capped_expansion_scores_the_same_whatever_the_load_order() {
    let nodes = plank_nodes();
    let a = Graph::from_records(nodes.clone(), vec![]);
    let mut b = Graph::from_records(vec![node(1, "Filler", "")], vec![]);
    for n in nodes.iter().rev() {
        b.insert_node(n.clone()).expect("the image fits");
    }
    assert_eq!(a.word_index_image(), b.word_index_image());
    let bits = |hits: Vec<(u128, f64, String)>| {
        let mut v: Vec<(u128, u64)> = hits.into_iter().map(|h| (h.0, h.1.to_bits())).collect();
        v.sort();
        v
    };
    assert_eq!(
        bits(search(&[&a], "plank vendor")),
        bits(search(&[&b], "plank vendor"))
    );
    let (left, right) = nodes.split_at(40);
    let g1 = Graph::from_records(left.to_vec(), vec![]);
    let g2 = Graph::from_records(right.to_vec(), vec![]);
    let split = bits(search(&[&g1, &g2], "plank"));
    assert_eq!(split, bits(search(&[&g2, &g1], "plank")));
    assert_eq!(split.len(), PREFIX_CAP + 8);
}

/// Hybrid's informative test (embeddings-expert, s21): a word's df is its exact and stem matches' summed, at most N;
/// prefix matches never count.
#[test]
fn the_informative_df_sums_the_stem_class_and_skips_prefixes() {
    let g = Graph::from_records(
        vec![
            node(1, "vendor", ""),
            node(2, "vendors", ""),
            node(3, "vendor vendors", ""),
            node(4, "vendorship", ""),
            node(5, "iso27001", ""),
        ],
        vec![],
    );
    let q = WordQuery::new("vendor iso27001 vend").unwrap();
    let stats = g.word_stats(&q);
    // In string order: iso27001, vend, vendor.
    assert_eq!(stats.informative_df(0), 1);
    assert_eq!(stats.informative_df(1), 0);
    assert_eq!(stats.informative_df(2), 4);
}

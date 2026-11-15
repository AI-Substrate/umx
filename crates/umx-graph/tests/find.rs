//! Title find (req-57) against s06's answer (`old_find`, which s06 matched to the bit): since ws004 s15 the word
//! index picks the candidates by an upper bound on the score, so the answer is s06's exactly (nothing omitted, added
//! or reordered), the near-duplicate answer never changes, and find follows every write. Since ws004 s24 find's
//! reference is s06's scan with s24's rule on top (`old_find::find_rule`: a title holding every query word scores
//! max(d, 0.5 + 0.2 d)); the near-duplicate check's is still s06's.

mod old_find;

use std::collections::BTreeMap;
use umx_core::{Id, Node};
use umx_graph::{Graph, title_score};

const WORDS: &[&str] = &[
    "approval",
    "delays",
    "launch",
    "date",
    "budget",
    "supplier",
    "contract",
    "review",
    "risk",
    "safety",
    "audit",
    "migration",
    "server",
    "outage",
    "customer",
    "churn",
    "pricing",
    "model",
    "hiring",
    "freeze",
    "vendor",
    "lock",
    "data",
    "loss",
    "backup",
    "policy",
    "compliance",
    "gap",
    "release",
    "train",
    "testing",
    "debt",
    "latency",
    "spike",
    "capacity",
    "plan",
    "security",
    "patch",
    "network",
    "partner",
    "onboarding",
    "training",
    "roadmap",
    "scope",
    "creep",
    "design",
    "decision",
    "quality",
];

const ODD_TITLES: &[&str] = &[
    "Σίσυφος ΟΔΟΣ",
    "ΟΔΟΣ Σίσυφος",
    "Straße Planung",
    "İSTANBUL office move",
    "東京 計画 review",
    "rocket 🚀 launch date",
    "ǅungla audit",
    "—",
    "",
    "   ...   ",
    "Approval delays",
    "approval-delays!!",
    "APPROVAL   DELAYS",
    "delays approval",
    "Launch date 2027",
    "launch date",
    "a b c",
];

/// SplitMix64.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn node(id: u128, kind: &str, title: &str) -> Node {
    Node {
        id: Id(id),
        kind: kind.into(),
        title: title.into(),
        status: "open".into(),
        note: String::new(),
        evidence: Vec::new(),
        status_history: Vec::new(),
        created: "2026-11-10T01:00:00Z".into(),
        updated: "2026-11-10T01:00:00Z".into(),
        created_by: "test".into(),
        locator: None,
        authored_by_session: None,
        edited_by_session: None,
        fields: BTreeMap::new(),
    }
}

#[test]
fn provenance_title_find_matches_the_full_scan_without_changing_content_find() {
    let mut nodes = vec![
        node(1, "problem", "Launch review"),
        node(2, "agent", "Ordinary launch agent"),
        node(3, "session", "Ordinary launch session"),
    ];
    let bare = Graph::from_records(nodes.clone(), vec![]);
    for (id, kind, key, title) in [
        (
            4,
            "session",
            "session/unknown/unknown",
            "Launch review for finance operations legal engineering and regional delivery teams",
        ),
        (5, "agent", "agent/pij/launch_reviewer", "Launch review"),
        (6, "model", "model/launch-review", "Launch review model"),
    ] {
        let mut n = node(id, kind, title);
        n.fields
            .insert("prov_key".into(), serde_json::Value::String(key.into()));
        nodes.push(n);
    }
    let graph = Graph::from_records(nodes.clone(), vec![]);
    assert_eq!(graph.provenance_count(), 3);
    for text in [
        "launch review",
        "launc review",
        "review launch",
        "launch launch",
        "zzzz qqqq",
        "",
    ] {
        #[cfg(feature = "bench")]
        assert_eq!(graph.title_candidates(text), bare.title_candidates(text));
        for kind in [
            None,
            Some("session"),
            Some("agent"),
            Some("model"),
            Some("problem"),
        ] {
            let actual: Vec<_> = graph
                .find_with_provenance(text, kind, 20)
                .into_iter()
                .map(|(n, score)| (n.id(), score.to_bits()))
                .collect();
            let expected = old_find::find_rule(
                nodes
                    .iter()
                    .map(|n| (n.id, n.kind.as_str(), n.title.as_str())),
                text,
                kind,
                20,
            );
            assert_eq!(actual, expected, "{text:?} {kind:?}");
            assert_eq!(
                old_find::new(&graph, text, kind, 20),
                old_find::new(&bare, text, kind, 20)
            );
            assert_eq!(
                graph
                    .near_duplicates(text, None, 0.85, 0.85, 3)
                    .iter()
                    .map(|(n, s)| (n.id(), *s))
                    .collect::<Vec<_>>(),
                bare.near_duplicates(text, None, 0.85, 0.85, 3)
                    .iter()
                    .map(|(n, s)| (n.id(), *s))
                    .collect::<Vec<_>>()
            );
        }
    }
    let contained = graph.find_with_provenance("launch review", Some("session"), 20);
    assert_eq!(contained[0].0.id(), Id(4));
    assert!(contained[0].1 >= 0.5);
}

#[test]
#[ignore = "release-only provenance title-find measurement"]
fn provenance_title_find_100k_measurement() {
    let mut nodes: Vec<_> = (1..=100_000)
        .map(|id| node(id, "problem", &format!("Content record {id}")))
        .collect();
    nodes[0].title = "Launch review".into();
    for i in 0..10_000 {
        let key = format!("session/copilot/h-{i:016x}");
        let mut n = node(100_001 + i, "session", &key);
        n.fields
            .insert("prov_key".into(), serde_json::Value::String(key));
        nodes.push(n);
    }
    let graph = Graph::from_records(nodes, vec![]);
    let (mut plain, mut shown) = (Vec::new(), Vec::new());
    for _ in 0..9 {
        let start = std::time::Instant::now();
        std::hint::black_box(graph.find("Launch review", None, 20));
        plain.push(start.elapsed().as_secs_f64() * 1000.0);
        let start = std::time::Instant::now();
        std::hint::black_box(graph.find_with_provenance("Launch review", None, 20));
        shown.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    plain.sort_by(f64::total_cmp);
    shown.sort_by(f64::total_cmp);
    println!(
        "{}",
        serde_json::json!({
            "content_nodes": 100_000, "sessions": graph.provenance_count(), "runs": 9,
            "find_ms": plain[4], "find_with_provenance_ms": shown[4], "delta_ms": shown[4] - plain[4],
        })
    );
}

fn title(rng: &mut Rng) -> String {
    let words: Vec<&str> = (0..2 + rng.below(4))
        .map(|_| WORDS[rng.below(WORDS.len())])
        .collect();
    let mut t = words.join([" ", " ", " ", ", ", " - ", ": "][rng.below(6)]);
    if rng.below(3) == 0 {
        t = t[..1].to_uppercase() + &t[1..];
    }
    t
}

/// One character dropped, doubled or replaced, `n` times.
fn typos(rng: &mut Rng, text: &str, n: usize) -> String {
    let mut c: Vec<char> = text.chars().collect();
    for _ in 0..n {
        let i = rng.below(c.len());
        match rng.below(3) {
            0 if c.len() > 3 => {
                c.remove(i);
            }
            1 => c.insert(i, c[i]),
            _ => c[i] = (b'a' + rng.below(26) as u8) as char,
        }
    }
    c.into_iter().collect()
}

fn swapped(text: &str) -> String {
    let mut w: Vec<&str> = text.split_whitespace().collect();
    w.reverse();
    w.join(" ")
}

fn fixture() -> (Graph, Vec<Node>) {
    let mut rng = Rng(27);
    let kinds = ["problem", "risk", "decision"];
    let mut nodes: Vec<Node> = (0..3000)
        .map(|i| node(1000 + i as u128, kinds[rng.below(3)], &title(&mut rng)))
        .collect();
    nodes.extend(
        ODD_TITLES
            .iter()
            .enumerate()
            .map(|(i, t)| node(10_000 + i as u128, kinds[i % 3], t)),
    );
    (Graph::from_records(nodes.clone(), Vec::new()), nodes)
}

fn queries(nodes: &[Node]) -> Vec<(String, Option<&'static str>, usize)> {
    let mut rng = Rng(57);
    let mut q: Vec<(String, Option<&'static str>, usize)> = Vec::new();
    let pick = |rng: &mut Rng| nodes[rng.below(3000)].title.clone();
    for _ in 0..12 {
        q.push((pick(&mut rng), None, 10));
    }
    for _ in 0..10 {
        let t = pick(&mut rng);
        q.push((typos(&mut rng, &t, 1), None, 10));
    }
    for _ in 0..10 {
        let t = pick(&mut rng);
        q.push((typos(&mut rng, &t, 2), None, 10));
    }
    for _ in 0..8 {
        let t = pick(&mut rng);
        q.push((swapped(&t), None, 10));
    }
    for (i, kind) in [
        "problem",
        "risk",
        "decision",
        "problem",
        "nope",
        "similar_to",
    ]
    .into_iter()
    .enumerate()
    {
        let t = pick(&mut rng);
        q.push((typos(&mut rng, &t, i % 2), Some(kind), 10));
    }
    for t in ["zzzz qqqq", "xylophone", "q"] {
        q.push((t.into(), None, 10));
    }
    for t in ["", "   ", "!!! ---", "—"] {
        q.push((t.into(), None, 10));
    }
    for t in [
        "ΟΔΟΣ σίσυφος",
        "strasse planung",
        "Straße",
        "istanbul office",
        "東京 計画",
        "rocket launch",
        "ǆungla",
        "approval delays",
        "aproval delays",
        "delays approval",
    ] {
        q.push((t.into(), None, 10));
    }
    // Every hit, a node add's 3 and 1, and ties broken by id.
    for (t, limit) in [
        ("approval delays", usize::MAX),
        ("launch date", usize::MAX),
        ("review", usize::MAX),
        ("budget risk", 3),
        ("budget risk", 1),
        ("plan", 0),
    ] {
        q.push((t.into(), None, limit));
    }
    q
}

/// One query, ws004 s15 against s06 (`old_find::find`, which s06 matched to the bit), as the graph-storage-expert set
/// it: the full list is s06's (same entries, scores to the bit and order), so is the list at limit 20, and the
/// near-duplicate check (best 3 at 0.85 or more) gives s06's answer.
fn compare(g: &Graph, nodes: &[Node], text: &str, kind: Option<&str>) {
    let rows = || {
        nodes
            .iter()
            .map(|n| (n.id, n.kind.as_str(), n.title.as_str()))
    };
    assert_eq!(
        old_find::new(g, text, kind, usize::MAX),
        old_find::find_rule(rows(), text, kind, usize::MAX),
        "{text:?} {kind:?}: the full list differs from the reference"
    );
    assert_eq!(
        old_find::new(g, text, kind, 20),
        old_find::find_rule(rows(), text, kind, 20),
        "{text:?} {kind:?} at limit 20"
    );
    // The check node add and batch run, with its own threshold: the same answer as s06's (a hard gate), whole-title
    // Dice only, less the titles ws004 s30's keep-rule drops before the best 3 are chosen. Also at 0.5, below where
    // s24's rule lifts a title (0.625), so a lift there would show.
    if kind.is_none() {
        let s06 = old_find::find(rows(), text, kind, usize::MAX);
        let title = |id: Id| &nodes.iter().find(|n| n.id == id).expect("held").title;
        for min in [0.85, 0.5] {
            let near: Vec<(Id, u64)> = s06
                .iter()
                .filter(|(_, b)| f64::from_bits(*b) >= min)
                .filter(|(id, _)| umx_graph::near_duplicate_keeps(text, title(*id)))
                .take(3)
                .copied()
                .collect();
            let checked: Vec<(Id, u64)> = g
                .near_duplicates(text, None, min, min, 3)
                .into_iter()
                .map(|(n, s)| (n.id(), s.to_bits()))
                .collect();
            assert_eq!(
                checked, near,
                "{text:?}: near_duplicates at {min} differs from s06"
            );
        }
    }
}

/// Seeded fuzzed queries from the fixture's titles: 0, 1 or 2 typos per word, a word dropped, the words swapped,
/// spaces inserted inside words or removed between them (split and joined words), and a type filter on some.
fn fuzzed(nodes: &[Node], count: usize, seed: u64) -> Vec<(String, Option<&'static str>)> {
    let mut rng = Rng(seed);
    let kinds = [
        None,
        None,
        None,
        None,
        Some("problem"),
        Some("risk"),
        Some("decision"),
    ];
    (0..count)
        .map(|_| {
            let title = &nodes[rng.below(nodes.len())].title;
            let mut words: Vec<String> = title.split_whitespace().map(str::to_owned).collect();
            if words.is_empty() {
                words.push("plan".into());
            }
            match rng.below(5) {
                0 if words.len() > 1 => {
                    let i = rng.below(words.len());
                    words.remove(i);
                }
                1 => words.reverse(),
                _ => {}
            }
            let per_word = rng.below(3);
            let words: Vec<String> = words
                .iter()
                .map(|w| {
                    if w.chars().count() > 3 {
                        typos(&mut rng, w, per_word)
                    } else {
                        w.clone()
                    }
                })
                .collect();
            let mut text = words.join(" ");
            match rng.below(4) {
                // Split: a space inside a word.
                0 => {
                    let c: Vec<char> = text.chars().collect();
                    for _ in 0..1 + rng.below(3) {
                        let i = 1 + rng.below(c.len().max(2) - 1);
                        let (a, b) = c.split_at(i.min(c.len()));
                        text = a.iter().chain([&' ']).chain(b).collect();
                    }
                }
                // Joined: spaces between words taken out.
                1 => {
                    let n = rng.below(3);
                    text = if n == 0 {
                        text.replace(' ', "")
                    } else {
                        text.replacen(' ', "", n)
                    };
                }
                _ => {}
            }
            (text, kinds[rng.below(kinds.len())])
        })
        .collect()
}

#[test]
fn title_find_on_the_word_index_gives_s06s_answer() {
    let (g, nodes) = fixture();
    let mut queries: Vec<(String, Option<&'static str>)> = queries(&nodes)
        .into_iter()
        .map(|(q, k, _)| (q, k))
        .collect();
    let fixed = queries.len();
    assert!(fixed >= 69, "{fixed} fixed queries");
    queries.extend(fuzzed(&nodes, 750, 15));
    let mut hits = 0;
    for (q, k) in &queries {
        compare(&g, &nodes, q, *k);
        hits += usize::from(!old_find::new(&g, q, *k, 1).is_empty());
    }
    // The set exercises real results, not only empty answers.
    assert!(
        hits >= queries.len() / 2,
        "{hits} of {} queries hit",
        queries.len()
    );
}

/// ws004 s18: titles made of unique tokens (the project manager's batch: 64-hex), with near copies so the
/// near-duplicate check has answers, and hex beside words; the fuzzed queries (typos, drops, swaps, split and joined)
/// give s06's answer.
#[test]
fn unique_token_titles_give_s06s_answer() {
    let mut rng = Rng(1818);
    let kinds = ["problem", "risk", "decision"];
    let hex = |rng: &mut Rng, n: usize| -> String {
        (0..n)
            .map(|_| b"0123456789abcdef"[rng.below(16)] as char)
            .collect()
    };
    let mut titles: Vec<String> = Vec::new();
    for i in 0..1500 {
        titles.push(match i % 6 {
            0..=2 => hex(&mut rng, 64),
            3 => {
                let t = titles[rng.below(titles.len())].clone();
                let n = 1 + rng.below(3);
                typos(&mut rng, &t, n)
            }
            4 => {
                let (w, n) = (WORDS[rng.below(WORDS.len())], 8 + rng.below(32));
                format!("{w} {}", hex(&mut rng, n))
            }
            _ => {
                let n = 4 + rng.below(12);
                hex(&mut rng, n).to_uppercase()
            }
        });
    }
    let nodes: Vec<Node> = titles
        .iter()
        .enumerate()
        .map(|(i, t)| node(50_000 + i as u128, kinds[i % 3], t))
        .collect();
    let g = Graph::from_records(nodes.clone(), Vec::new());
    let mut queries: Vec<(String, Option<&'static str>)> = titles
        .iter()
        .step_by(25)
        .map(|t| (t.clone(), None))
        .collect();
    queries.extend(fuzzed(&nodes, 400, 18));
    let mut near = 0;
    for (q, k) in &queries {
        compare(&g, &nodes, q, *k);
        near += usize::from(!g.near_duplicates(q, None, 0.85, 0.85, 3).is_empty());
    }
    assert!(
        near >= queries.len() / 4,
        "{near} of {} have a near duplicate",
        queries.len()
    );
}

/// The reviewer's witnesses (R107-1): a title whose words are joined or split in the query shares most of
/// its bigrams with it while no single word is close, so a per-word rule lost them; the bound keeps them.
#[test]
fn joined_and_split_words_are_found() {
    let nodes = vec![
        node(1, "problem", "customer approval workflow"),
        node(2, "problem", "abcd efgh ijkl mnop qrst uvwx"),
        node(3, "problem", "customerapprovalworkflow"),
        node(4, "risk", "unrelated launch plan"),
    ];
    let g = Graph::from_records(nodes.clone(), Vec::new());
    let near = |text: &str| -> Vec<(u128, String)> {
        g.near_duplicates(text, None, 0.85, 0.85, 3)
            .into_iter()
            .map(|(n, s)| (n.id().0, format!("{s:.3}")))
            .collect()
    };
    // Joined against split, both ways: the near-duplicate check at 0.885.
    assert_eq!(
        near("customerapprovalworkflow"),
        [(3, "1.000".into()), (1, "0.885".into())]
    );
    assert_eq!(
        near("customer approval workflow"),
        [(1, "1.000".into()), (3, "0.885".into())]
    );
    // Six split words against the joined query: a find hit at 0.727.
    let hits = g.find("abcdefghijklmnopqrstuvwx", None, 10);
    assert!(
        hits.iter()
            .any(|(n, s)| n.id().0 == 2 && format!("{s:.3}") == "0.727"),
        "{:?}",
        hits.iter().map(|(n, s)| (n.id().0, *s)).collect::<Vec<_>>()
    );
    for text in [
        "customerapprovalworkflow",
        "customer approval workflow",
        "abcdefghijklmnopqrstuvwx",
    ] {
        compare(&g, &nodes, text, None);
    }
}

#[test]
fn title_score_equals_the_old_score_to_the_bit() {
    let (_, nodes) = fixture();
    let qs = queries(&nodes);
    for (q, _, _) in &qs {
        for n in nodes.iter().step_by(37).chain(&nodes[3000..]) {
            assert_eq!(
                title_score(q, &n.title).to_bits(),
                old_find::title_score(q, &n.title).to_bits(),
                "{q:?} against {:?}",
                n.title
            );
        }
    }
}

#[test]
fn find_follows_appends_replaces_out_of_order_inserts_and_removes() {
    let (mut g, mut nodes) = fixture();
    // Every write keeps the word index, so find follows it under the same comparison as above.
    let check = |g: &Graph, nodes: &[Node], text: &str| compare(g, nodes, text, None);
    let sort = |nodes: &mut Vec<Node>| nodes.sort_by_key(|n| n.id);
    // Append.
    let n = node(20_000, "risk", "Quantum flux capacitor");
    g.insert_node(n.clone()).unwrap();
    nodes.push(n);
    check(&g, &nodes, "quantum flux capacitor");
    // Replace with a longer, then a shorter title, in the middle.
    for t in [
        "Quantum flux capacitor overload in the main reactor",
        "Flux",
    ] {
        let mut n = nodes[1500].clone();
        n.title = t.into();
        g.insert_node(n.clone()).unwrap();
        nodes[1500] = n;
        check(&g, &nodes, "quantum flux capacitor");
        check(&g, &nodes, "flux");
    }
    // An id below the last: the load-path rebuild.
    let n = node(999, "problem", "Flux quantum capacitor");
    g.insert_node(n.clone()).unwrap();
    nodes.push(n);
    sort(&mut nodes);
    check(&g, &nodes, "quantum flux capacitor");
    // Remove the last row, append in its place, then remove one in the middle.
    g.remove_node(Id(20_000));
    nodes.retain(|n| n.id != Id(20_000));
    check(&g, &nodes, "quantum flux capacitor");
    let n = node(20_001, "decision", "Approval delays again");
    g.insert_node(n.clone()).unwrap();
    nodes.push(n);
    check(&g, &nodes, "approval delays");
    check(&g, &nodes, "quantum flux capacitor");
    let id = nodes[700].id;
    g.remove_node(id);
    nodes.retain(|n| n.id != id);
    check(&g, &nodes, "quantum flux capacitor");
    check(&g, &nodes, "approval delays");
}
#[test]
fn review_probe_split_words_preserve_a_near_duplicate_above_the_hard_floor() {
    let query = "customerapprovalworkflow";
    let n = node(1, "problem", "customer approval workflow");
    let g = Graph::from_records(vec![n.clone()], Vec::new());
    let old = old_find::find(
        [(n.id, n.kind.as_str(), n.title.as_str())].into_iter(),
        query,
        None,
        3,
    );
    assert_eq!(old.len(), 1);
    let score = f64::from_bits(old[0].1);
    assert!(score >= 0.85, "old score {score}");
    assert_eq!(old_find::new(&g, query, None, 3), old);
    let near: Vec<_> = g
        .near_duplicates(query, None, 0.85, 0.85, 3)
        .into_iter()
        .map(|(n, s)| (n.id(), s.to_bits()))
        .collect();
    assert_eq!(near, old, "the old {score} near-duplicate was dropped");
}

#[test]
fn review_probe_split_words_do_not_omit_a_strong_find_result() {
    let query = "abcdefghijklmnopqrstuvwx";
    let n = node(1, "problem", "abcd efgh ijkl mnop qrst uvwx");
    let g = Graph::from_records(vec![n.clone()], Vec::new());
    let old = old_find::find(
        [(n.id, n.kind.as_str(), n.title.as_str())].into_iter(),
        query,
        None,
        3,
    );
    assert_eq!(old.len(), 1);
    let score = f64::from_bits(old[0].1);
    assert!(score >= 0.6, "old score {score}");
    assert_eq!(
        old_find::new(&g, query, None, 3),
        old,
        "the old {score} hit exceeds the accepted weak-omission band"
    );
}

/// ws004 s24, a tester's shape on a synthetic graph: a third short titles (2 to 5 words), the rest long (12 to
/// 15 words), from 400 pseudo-words, so a query's words sit in several titles.
fn long_titles() -> (Graph, Vec<Node>) {
    let mut rng = Rng(2424);
    let vocab: Vec<String> = (0..400)
        .map(|_| {
            (0..3 + rng.below(8))
                .map(|_| (b'a' + rng.below(26) as u8) as char)
                .collect()
        })
        .collect();
    let kinds = ["problem", "risk", "decision"];
    let nodes: Vec<Node> = (0..600)
        .map(|i| {
            let n = if i % 3 == 0 {
                2 + rng.below(4)
            } else {
                12 + rng.below(4)
            };
            let words: Vec<&str> = (0..n)
                .map(|_| vocab[rng.below(vocab.len())].as_str())
                .collect();
            node(70_000 + i as u128, kinds[i % 3], &words.join(" "))
        })
        .collect();
    (Graph::from_records(nodes.clone(), Vec::new()), nodes)
}

/// Short queries (2 to 4 of a long title's words, any order; joined by spaces or hyphens; some capitalised) find
/// the title they came from, at 0.5 or more, where whole-title Dice misses most of them; the answer is the
/// reference's to the bit, and the near-duplicate check is s06's.
#[test]
fn short_queries_find_long_titles_holding_their_words() {
    let (g, nodes) = long_titles();
    let mut rng = Rng(24);
    let (mut queries, mut missed_by_dice) = (0, 0);
    for _ in 0..300 {
        let source = &nodes[rng.below(nodes.len())];
        let mut words: Vec<&str> = source.title.split(' ').collect();
        if words.len() < 12 {
            continue;
        }
        let k = 2 + rng.below(3);
        let mut picked = Vec::new();
        for _ in 0..k {
            picked.push(words.swap_remove(rng.below(words.len())));
        }
        let mut q = picked.join(["  ", "-", " - ", " "][rng.below(4)]);
        match rng.below(3) {
            0 => q = q.to_uppercase(),
            1 => q = q[..1].to_uppercase() + &q[1..],
            _ => {}
        }
        queries += 1;
        missed_by_dice += usize::from(title_score(&q, &source.title) < old_find::MIN_SCORE);
        let hit = g
            .find(&q, None, usize::MAX)
            .into_iter()
            .find(|(n, _)| n.id() == source.id);
        assert!(
            hit.is_some_and(|(_, s)| s >= 0.5),
            "{q:?} does not find {:?}",
            source.title
        );
        compare(&g, &nodes, &q, None);
        compare(&g, &nodes, &q, Some(source.kind.as_str()));
    }
    // The set is the bug's shape: Dice alone misses most of these.
    assert!(queries >= 150, "{queries} queries");
    assert!(
        missed_by_dice * 2 >= queries,
        "Dice alone missed only {missed_by_dice} of {queries}"
    );
    // s15's fuzz (typos, drops, swaps, split and joined words, type filters) on long titles.
    for (q, k) in fuzzed(&nodes, 300, 2424) {
        compare(&g, &nodes, &q, k);
    }
}

/// ws004 s24's edges: an exact title first at 1.0, repeats counted, whole words only, every word needed, and the
/// near-duplicate check unchanged.
#[test]
fn containment_needs_every_word_as_often_as_the_query_holds_it() {
    let long = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi";
    let nodes = vec![
        node(1, "problem", "alpha beta"),
        node(2, "problem", long),
        node(
            3,
            "risk",
            "risk audit plan for the quarterly supplier review and the vendor contract renewal",
        ),
        node(
            4,
            "risk",
            "risk risk audit plan for the quarterly supplier review and vendor contract renewal",
        ),
        node(
            5,
            "decision",
            "abc rate tea date for the quarterly supplier review and the vendor plan",
        ),
        node(
            6,
            "problem",
            "Approval delays at month end for the regional finance team and the shared services desk",
        ),
    ];
    let g = Graph::from_records(nodes.clone(), Vec::new());
    let found = |q: &str| -> Vec<(u128, String)> {
        g.find(q, None, usize::MAX)
            .into_iter()
            .map(|(n, s)| (n.id().0, format!("{s:.3}")))
            .collect()
    };
    // The exact title first; the long title holding both words next, lifted from Dice 0.262 to 0.552.
    assert_eq!(
        found("alpha beta"),
        [(1, "1.000".into()), (2, "0.552".into())]
    );
    // A close fuzzy match (Dice 0.957, not held) outranks the held long title: the lift stays under 0.625.
    let fuzzy = Graph::from_records(
        vec![node(7, "risk", "alpha betta"), node(2, "problem", long)],
        Vec::new(),
    );
    let ranked: Vec<(u128, String)> = fuzzy
        .find("alpha beta", None, usize::MAX)
        .into_iter()
        .map(|(n, s)| (n.id().0, format!("{s:.3}")))
        .collect();
    assert_eq!(ranked, [(7, "0.957".into()), (2, "0.552".into())]);
    // node find's docs example: whole-title Dice 0.355, under MIN_SCORE; held, so 0.571.
    assert_eq!(found("month-end approval"), [(6, "0.571".into())]);
    // A close match keeps its score: the long title itself.
    assert_eq!(found(long)[0], (2, "1.000".into()));
    // Repeats counted: "risk risk" is held only by the title with "risk" twice.
    let risk = found("risk risk");
    assert!(risk.iter().any(|(id, _)| *id == 4), "{risk:?}");
    assert!(!risk.iter().any(|(id, _)| *id == 3), "{risk:?}");
    // Whole words: every bigram of "ate" is in title 5 (" a", "at", "te", "e "), the word is not.
    assert!(!found("ate").iter().any(|(id, _)| *id == 5));
    // Every word: one word missing from the title, or from the graph, finds nothing long.
    for q in ["alpha omega", "alpha zzzz", "lambda risk"] {
        assert!(!found(q).iter().any(|(id, _)| *id == 2), "{q:?}");
    }
    // The near-duplicate check is whole-title Dice: the long title is no near duplicate of "alpha beta".
    let near: Vec<u128> = g
        .near_duplicates("alpha beta", None, 0.5, 0.5, 3)
        .into_iter()
        .map(|(n, _)| n.id().0)
        .collect();
    assert_eq!(near, [1]);
    for q in [
        "alpha beta",
        long,
        "risk risk",
        "risk",
        "ate",
        "alpha omega",
        "alpha zzzz",
        "lambda risk",
        "supplier review",
        "the the",
        "Vendor-Contract",
        "month-end approval",
        "aproval month",
    ] {
        compare(&g, &nodes, q, None);
    }
}

/// ws004 s24: one word held by 200 titles of 1 to 15 words makes all 200 candidates, and find keeps the best
/// `limit` of them before sorting; every limit's answer is the full answer's prefix.
#[test]
fn a_limit_keeps_the_best_of_many_held_titles() {
    let kinds = ["problem", "risk", "decision"];
    let nodes: Vec<Node> = (0..200)
        .map(|i| {
            let rest: Vec<String> = (0..i % 15).map(|j| format!("w{i}x{j}")).collect();
            node(
                80_000 + i as u128,
                kinds[i % 3],
                &format!("shared {}", rest.join(" ")),
            )
        })
        .collect();
    let g = Graph::from_records(nodes.clone(), Vec::new());
    let all = old_find::new(&g, "shared", None, usize::MAX);
    assert_eq!(all.len(), 200);
    for limit in [0, 1, 3, 5, 20, 50, 199] {
        assert_eq!(
            old_find::new(&g, "shared", None, limit),
            all[..limit],
            "limit {limit}"
        );
    }
    compare(&g, &nodes, "shared", None);
    compare(&g, &nodes, "shared", Some("risk"));
}

// ws004 s30 (field report): near_duplicate and short-query precision. Every title here is synthetic.

/// The leftover gate on its own: words joined, split, misspelled or made plural stay duplicates; a shared scaffold
/// with a different word, or a different number, does not.
#[test]
fn near_duplicate_keeps_typos_and_joins_but_not_a_scaffold_or_a_number() {
    use umx_graph::near_duplicate_keeps;
    for (a, b) in [
        ("Customer approval workflow", "Customer approvel workflow"),
        ("Custmer approvel workflow", "Customer approval workflow"),
        ("Customer approval workflow", "customerapprovalworkflow"),
        (
            "Release checklist for the workflow",
            "Release checklist for the work flow",
        ),
        ("Acme Ops Data Science team", "Acme Ops Data Sciences team"),
        ("Colour palette tokens", "Color palette tokens"),
        ("Acme platform tema", "Acme platform team"),
        (
            "Northwind Analytics Platform team",
            "Northwind Platform Analytics team",
        ),
        ("Pin the capture window", "Pin the capture window now"),
        ("Phase1 rollout plan", "Phase 1 rollout plan"),
        // A short token split off a word is the same letters: kept (R294 B1's fix keeps joins and splits).
        ("Send the e mail digest", "Send the email digest"),
        // Characters, not bytes: two letters differ in 11, within 11 / 4.
        ("Überprüfung der Kundendaten", "Uberprufung der Kundendaten"),
    ] {
        assert!(near_duplicate_keeps(a, b), "{a:?} / {b:?} dropped");
        assert!(near_duplicate_keeps(b, a), "{b:?} / {a:?} dropped");
    }
    for (a, b) in [
        (
            "Acme Ops Data Science team",
            "Acme Ops Geoscience Data team",
        ),
        ("Acme Ops team rota", "Acme DevOps team rota"),
        ("Acme Lab Biology unit", "Acme Lab Microbiology unit"),
        (
            "Phase 1 rollout plan for the billing service",
            "Phase 2 rollout plan for the billing service",
        ),
        (
            "v2 rollout of the billing service",
            "v3 rollout of the billing service",
        ),
        (
            "Billing service budget for 2025",
            "Billing service budget for 2026",
        ),
        (
            "Billing service rollout plan",
            "Billing service rollout plan v2",
        ),
        // Tester shapes (the project manager, 2026-11-12), Dice 0.94 and 0.93: a period token, a close but other word.
        (
            "FY24 budget review for the smelter",
            "FY25 budget review for the smelter",
        ),
        (
            "Aluminium supply contract for the smelter",
            "Alumina supply contract for the smelter",
        ),
        (
            "Unit A rollout plan for the smelter",
            "Unit B rollout plan for the smelter",
        ),
        // R294 B1: a short token plus a longer word's typo (Dice 0.921, pooled OSA 2 within a budget of 2).
        (
            "Unit A rollout routing plan for the smelter",
            "Unit B rollout routings plan for the smelter",
        ),
    ] {
        assert!(!near_duplicate_keeps(a, b), "{a:?} / {b:?} kept");
        assert!(!near_duplicate_keeps(b, a), "{b:?} / {a:?} kept");
    }
}

fn near_ids(g: &Graph, title: &str, kind: Option<&str>, limit: usize) -> Vec<u128> {
    g.near_duplicates(title, kind, 0.85, 0.95, limit)
        .into_iter()
        .map(|(n, _)| n.id().0)
        .collect()
}

/// A node of another type is a near duplicate only at 0.95 or more; the type is compared as stored, and a type the
/// graph does not hold yet makes every node another type's.
#[test]
fn near_duplicates_of_another_type_need_near_identical_wording() {
    let g = Graph::from_records(
        vec![
            node(1, "component", "Pricing engine cache layer"),
            node(2, "limitation", "Pricing engine cache layer"),
            node(3, "limitation", "Pricing engine cache layers"),
        ],
        Vec::new(),
    );
    let title = "Pricing engine cache layers";
    // Same type at 0.945 (1); another type at 0.945 (2) is not near, at 1.0 (3) it is.
    assert_eq!(near_ids(&g, title, Some("component"), 3), [3, 1]);
    assert_eq!(near_ids(&g, title, Some("limitation"), 3), [3, 2]);
    assert_eq!(near_ids(&g, title, Some("risk"), 3), [3]);
    assert_eq!(near_ids(&g, title, None, 3), [3, 1, 2]);
}

/// The rules drop hits before the best `limit` are chosen: a dropped scaffold title scoring higher never hides a
/// kept one.
#[test]
fn near_duplicate_rules_run_before_the_limit() {
    let g = Graph::from_records(
        vec![
            node(1, "team", "Acme Ops Geoscience Data team"),
            node(2, "team", "Acme Ops Data Science"),
            node(3, "component", "Acme Ops Data Science teams"),
        ],
        Vec::new(),
    );
    // 1 scores 0.912 and is a scaffold; 2 scores 0.898 and only lacks a word; 3 is another type at 0.945.
    assert_eq!(
        near_ids(&g, "Acme Ops Data Science team", Some("team"), 1),
        [2]
    );
    assert_eq!(
        near_ids(&g, "Acme Ops Data Science team", Some("team"), 3),
        [2]
    );
}

const FIRST: &[&str] = &[
    "james",
    "mary",
    "robert",
    "patricia",
    "john",
    "jennifer",
    "michael",
    "linda",
    "david",
    "elizabeth",
    "william",
    "barbara",
    "richard",
    "susan",
    "joseph",
    "jessica",
    "thomas",
    "sarah",
    "charles",
    "karen",
    "daniel",
    "lisa",
    "matthew",
    "nancy",
    "anthony",
    "betty",
    "mark",
    "sandra",
    "donald",
    "ashley",
    "steven",
    "kimberly",
    "paul",
    "emily",
    "andrew",
    "donna",
    "joshua",
    "michelle",
    "kenneth",
    "carol",
    "kevin",
    "amanda",
    "brian",
    "melissa",
    "george",
    "deborah",
    "timothy",
    "stephanie",
    "ronald",
    "rebecca",
    "jason",
    "sharon",
    "edward",
    "laura",
    "jeffrey",
    "cynthia",
    "ryan",
    "dorothy",
    "jacob",
    "amy",
    "gary",
    "kathleen",
    "nicholas",
    "angela",
    "eric",
    "shirley",
    "jonathan",
    "emma",
    "stephen",
    "brenda",
    "larry",
    "pamela",
    "justin",
    "nicole",
    "scott",
    "anna",
    "brandon",
    "samantha",
    "benjamin",
    "katherine",
    "samuel",
    "christine",
    "gregory",
    "debra",
    "alexander",
    "rachel",
    "patrick",
    "carolyn",
    "frank",
    "janet",
    "raymond",
    "maria",
    "jack",
    "olivia",
    "dennis",
    "heather",
    "jerry",
    "helen",
    "tyler",
    "catherine",
    "aaron",
    "diane",
    "jose",
    "julie",
    "adam",
    "victoria",
    "nathan",
    "joyce",
    "henry",
    "lauren",
    "zachary",
    "kelly",
    "douglas",
    "christina",
    "peter",
    "ruth",
    "kyle",
    "joan",
    "noah",
    "virginia",
    "ethan",
    "judith",
    "jeremy",
    "evelyn",
    "christian",
    "hannah",
    "walter",
    "andrea",
    "keith",
    "megan",
    "austin",
    "cheryl",
    "roger",
    "jacqueline",
    "terry",
    "madison",
    "sean",
    "teresa",
    "gerald",
    "abigail",
    "carl",
    "sophia",
    "dylan",
    "martha",
    "harold",
    "sara",
    "jordan",
    "gloria",
    "jesse",
    "janice",
    "bryan",
    "kathryn",
];

const LAST: &[&str] = &[
    "smith",
    "johnson",
    "williams",
    "brown",
    "jones",
    "garcia",
    "miller",
    "davis",
    "rodriguez",
    "martinez",
    "hernandez",
    "lopez",
    "gonzalez",
    "wilson",
    "anderson",
    "thomas",
    "taylor",
    "moore",
    "jackson",
    "martin",
    "lee",
    "perez",
    "thompson",
    "white",
    "harris",
    "sanchez",
    "clark",
    "ramirez",
    "lewis",
    "robinson",
    "walker",
    "young",
    "allen",
    "king",
    "wright",
    "scott",
    "torres",
    "nguyen",
    "hill",
    "flores",
    "green",
    "adams",
    "nelson",
    "baker",
    "hall",
    "rivera",
    "campbell",
    "mitchell",
    "carter",
    "roberts",
    "gomez",
    "phillips",
    "evans",
    "turner",
    "diaz",
    "parker",
    "cruz",
    "edwards",
    "collins",
    "reyes",
    "stewart",
    "morris",
    "morales",
    "murphy",
    "cook",
    "rogers",
    "gutierrez",
    "ortiz",
    "morgan",
    "cooper",
    "peterson",
    "bailey",
    "reed",
    "kelly",
    "howard",
    "ramos",
    "kim",
    "cox",
    "ward",
    "richardson",
    "watson",
    "brooks",
    "chavez",
    "wood",
    "james",
    "bennett",
    "gray",
    "mendoza",
    "ruiz",
    "hughes",
    "price",
    "alvarez",
    "castillo",
    "sanders",
    "patel",
    "myers",
    "long",
    "ross",
    "foster",
    "jimenez",
    "powell",
    "jenkins",
    "perry",
    "russell",
    "sullivan",
    "bell",
    "coleman",
    "butler",
    "henderson",
    "barnes",
    "fisher",
    "vasquez",
    "simmons",
    "romero",
    "patterson",
    "hamilton",
    "graham",
    "reynolds",
    "griffin",
    "wallace",
    "moreno",
    "west",
    "cole",
    "hayes",
    "bryant",
    "herrera",
    "gibson",
    "ellis",
    "tran",
    "medina",
    "aguilar",
    "stevens",
    "murray",
    "ford",
    "castro",
    "marshall",
    "owens",
    "harrison",
    "fernandez",
    "mcdonald",
    "woods",
    "washington",
    "kennedy",
    "wells",
    "vargas",
    "henry",
    "chen",
    "freeman",
    "webb",
    "tucker",
    "salazar",
    "cunningham",
];

fn person(rng: &mut Rng) -> String {
    let cap = |w: &str| w[..1].to_uppercase() + &w[1..];
    format!(
        "{} {}",
        cap(FIRST[rng.below(FIRST.len())]),
        cap(LAST[rng.below(LAST.len())])
    )
}

/// The field report shape: a two-word name finds unrelated two-word names through shared letters (Dice 0.4 to 0.6).
/// Over 2,000 synthetic people and 300 names none of them holds, whole-title Dice alone returns many per query; the
/// short-query rule keeps a few close spellings at most. A one-letter typo of a held name still finds it.
#[test]
fn short_name_queries_do_not_find_unrelated_names() {
    let mut rng = Rng(30);
    let mut people: Vec<String> = Vec::new();
    while people.len() < 2_000 {
        let p = person(&mut rng);
        if !people.contains(&p) {
            people.push(p);
        }
    }
    let nodes: Vec<Node> = people
        .iter()
        .enumerate()
        .map(|(i, p)| node(90_000 + i as u128, "person", p))
        .collect();
    let g = Graph::from_records(nodes.clone(), Vec::new());
    let (mut queries, mut by_dice, mut found) = (0, 0, 0);
    while queries < 300 {
        let q = person(&mut rng);
        if people.contains(&q) {
            continue;
        }
        queries += 1;
        // The instrument can find: whole-title Dice reaches find's floor for many names.
        by_dice += people.iter().filter(|p| title_score(&q, p) >= 0.4).count();
        found += g.find(&q, None, usize::MAX).len();
        if queries <= 30 {
            compare(&g, &nodes, &q, None);
        }
    }
    assert!(
        by_dice >= 10 * queries,
        "{by_dice} Dice hits for {queries} queries"
    );
    assert!(
        found * 10 <= queries,
        "{found} found for {queries} unheld names"
    );
    // Two neighbours swapped in each word: Dice falls (often below 0.8) while each word stays one edit away.
    let mut checked = 0;
    for p in people.iter().take(300) {
        let q: Vec<String> = p
            .split(' ')
            .map(|w| {
                let mut c: Vec<char> = w.chars().collect();
                let i = rng.below(c.len() - 1);
                c.swap(i, i + 1);
                c.into_iter().collect()
            })
            .collect();
        let q = q.join(" ");
        if title_score(&q, p) < 0.4 {
            continue;
        }
        checked += usize::from(title_score(&q, p) < 0.8);
        assert!(
            g.find(&q, None, usize::MAX)
                .iter()
                .any(|(n, _)| n.title() == p),
            "{q:?} does not find {p:?}"
        );
    }
    assert!(checked >= 100, "{checked} typo queries under 0.8");
}

/// The short-query rule's edges: a word held is enough (s24), a joined query word counts, a number must match, and a
/// close whole title (0.8 or more) or a long query is kept on Dice alone.
#[test]
fn short_queries_need_every_word_close_unless_the_title_is() {
    let g = Graph::from_records(
        vec![
            node(1, "person", "Jacqueline Salazar"),
            node(2, "process", "Customer approval workflow"),
            node(3, "plan", "Budget 2026 forecast"),
            node(4, "plan", "Budget 2026"),
            node(5, "person", "Rebecca Hamilton"),
            node(6, "org", "Central Trolley"),
            node(7, "org", "Contoso Patrol"),
            node(8, "org", "Controller Group"),
        ],
        Vec::new(),
    );
    let found = |q: &str| -> Vec<u128> {
        g.find(q, None, usize::MAX)
            .into_iter()
            .map(|(n, _)| n.id().0)
            .collect()
    };
    // Dice 0.595: a shared first name is not a match.
    assert!(!found("Jacqueline Moreno").contains(&1));
    assert!(found("Jacqueline").contains(&1));
    // Dice 0.706: each word one swap away.
    assert!(found("Rebceca Hamilotn").contains(&5));
    // Dice 0.727: one query word joining three title words.
    assert!(found("approvalworkflow").contains(&2));
    // Dice 0.606: 2025 is not 2026; 0.833 keeps the short title on Dice alone.
    assert!(!found("budget 2025").contains(&3));
    assert!(found("budget 2025").contains(&4));
    // One word, no title word close (tester shape, the project manager): Dice 0.5 and 0.696 alone are not a match; a
    // title word holding the query word is (0.56).
    let control = found("control");
    assert!(
        !control.contains(&6) && !control.contains(&7),
        "{control:?}"
    );
    assert!(control.contains(&8), "{control:?}");
    // Four words: Dice alone, as before.
    assert!(found("jacqueline moreno salazar smith").contains(&1));
    for q in [
        "Jacqueline Moreno",
        "Rebceca Hamilotn",
        "approvalworkflow",
        "budget 2025",
        "control",
        "jacqueline moreno salazar smith",
    ] {
        compare(
            &g,
            &[
                node(1, "person", "Jacqueline Salazar"),
                node(2, "process", "Customer approval workflow"),
                node(3, "plan", "Budget 2026 forecast"),
                node(4, "plan", "Budget 2026"),
                node(5, "person", "Rebecca Hamilton"),
                node(6, "org", "Central Trolley"),
                node(7, "org", "Contoso Patrol"),
                node(8, "org", "Controller Group"),
            ],
            q,
            None,
        );
    }
}

/// Optimal string alignment distance by the textbook full table, in characters: the reference for the word rule.
fn osa(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut d = vec![vec![0; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

/// ws004 s30: the word rule equals the reference: a word is close exactly when its optimal string alignment distance
/// is at most max(1, longer / 4), so the cheap tests (length, letters present) and the banded table with its early
/// exit drop nothing. Random words of 4 to 14 letters, some non-ASCII, take 1 to 4 random edits (substitute,
/// insert, delete, swap neighbours), so both sides of the budget occur. Checked through near_duplicate_keeps (one
/// word left each side) and, for close words, find_keeps (an ASCII title split on its raw bytes, a non-ASCII one
/// normalised).
#[test]
fn the_word_rule_equals_the_reference_edit_distance() {
    let letters: Vec<char> = "abcdefghijklmnopqrstuvwxyz\u{e9}\u{fc}".chars().collect();
    let mut rng = Rng(3030);
    let (mut close, mut far) = (0, 0);
    for _ in 0..20_000 {
        let len = 4 + rng.below(11);
        let word: Vec<char> = (0..len)
            .map(|_| letters[rng.below(letters.len())])
            .collect();
        let mut typo = word.clone();
        for _ in 0..1 + rng.below(4) {
            let i = rng.below(typo.len());
            match rng.below(4) {
                0 => typo[i] = letters[rng.below(letters.len())],
                1 => typo.insert(i, letters[rng.below(letters.len())]),
                2 if typo.len() > 4 => {
                    typo.remove(i);
                }
                _ if i + 1 < typo.len() => typo.swap(i, i + 1),
                _ => typo[i] = letters[rng.below(letters.len())],
            }
        }
        let (w, t): (String, String) = (word.iter().collect(), typo.iter().collect());
        let within = osa(&w, &t) <= (len.max(typo.len()) / 4).max(1);
        if within {
            close += 1;
        } else {
            far += 1;
        }
        assert_eq!(
            umx_graph::near_duplicate_keeps(&format!("alpha {w}"), &format!("alpha {t}")),
            within,
            "{w:?} / {t:?}"
        );
        if within {
            assert!(
                umx_graph::find_keeps(&w, &format!("Gamma-{}", t.to_uppercase()), 0.5, false),
                "{w:?} in {t:?}"
            );
        }
    }
    assert!(close > 5_000 && far > 5_000, "{close} close, {far} far");
}

/// ws004 s30 with a later fix: `find --provenance` applies the short-query rule to provenance titles as find does to
/// content titles; a provenance title holding the query word is kept, as a content one is.
#[test]
fn provenance_find_drops_short_query_noise_as_content_find_does() {
    let mut nodes = vec![
        node(1, "org", "Central Trolley"),
        node(2, "org", "Controller Group"),
    ];
    for (id, kind, key, title) in [
        (3, "agent", "agent/pij/central_trolley", "Central Trolley"),
        (4, "model", "model/controller-group", "Controller Group"),
    ] {
        let mut n = node(id, kind, title);
        n.fields
            .insert("prov_key".into(), serde_json::Value::String(key.into()));
        nodes.push(n);
    }
    let graph = Graph::from_records(nodes, vec![]);
    assert_eq!(graph.provenance_count(), 2);
    // Dice 0.5 on letters alone: the instrument can score it.
    assert!(title_score("control", "Central Trolley") >= 0.4);
    let ids = |hits: Vec<(umx_graph::NodeRef<'_>, f64)>| -> Vec<u128> {
        hits.into_iter().map(|(n, _)| n.id().0).collect()
    };
    assert_eq!(ids(graph.find("control", None, 20)), [2]);
    assert_eq!(ids(graph.find_with_provenance("control", None, 20)), [2, 4]);
}

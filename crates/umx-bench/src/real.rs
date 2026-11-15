//! The ruler's realistic fixture (workstream 003 s14, plan 153; graph-storage-expert's s14 spec, section 1). The
//! rich fixture has 64 distinct words, so it flatters every text path (architecture review, ap-34). This one draws its
//! text from 60,000 synthetic words on a Zipf spread (s = 1.07), so a few words sit in most titles and most words
//! are rare. Synthetic only: words are built from syllables, never real names or client terms.
//!
//! Same seed, same bytes on every machine: only SplitMix64 draws, integer arithmetic and IEEE `+ - * /` (Rust never
//! contracts them to fused ops); the Zipf weights use [`pow_neg`] below, not the platform's `powf`, whose last bit
//! may differ between C libraries. No hash map is iterated.

use crate::fixture::{Fixture, Rng};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use umx_core::codec;
use umx_core::ontology::{FieldDecl, FieldKind, PART_OF, STARTER_STATUSES, STARTER_TYPES};
use umx_core::{Edge, GraphMeta, Id, Locator, Node, Ontology};

/// 2026-11-09T00:00:00Z, as the other fixtures.
const EPOCH_MS: u64 = 1_794_182_400_000;
/// The Zipf exponent over vocabulary rank (spec: s = 1.07).
const ZIPF_S: f64 = 1.07;
pub const VOCABULARY: usize = 60_000;
/// Share of non-tree edges whose target is drawn hub-first (Zipf over a seeded node order); the rest are uniform.
/// Tuned so the top node holds about 1% of all edges at 100k nodes.
const HUB_PERCENT: usize = 12;
/// The relations: part_of (the tree) and five starter relations that are not symmetric (D49), so every stored edge
/// is one edge and the load's reversed-pair pass has nothing to fold.
pub const RELATIONS: &[&str] = &[
    PART_OF,
    "supports",
    "depends_on",
    "blocks",
    "answers",
    "raises",
];
/// Evidence kinds the product accepts (`EvidenceKind`), with the share of nodes holding each, in parts per 1,000.
/// `analyst` is the rare kind (about 2%), `room` the common one (about 40%); 30% of nodes hold none.
pub const EVIDENCE_KINDS: &[(&str, u64)] = &[
    ("analyst", 20),
    ("document", 200),
    ("screen", 150),
    ("chat", 250),
    ("room", 400),
];
pub const RARE_KIND: &str = "analyst";
pub const COMMON_KIND: &str = "room";
/// The custom text field the ontology declares (indexed by word search), the custom number field, and the date field
/// (the typed time column, D71 step 10).
pub const TEXT_FIELD: &str = "topic_note";
pub const NUMBER_FIELD: &str = "weight";
pub const DATE_FIELD: &str = "due_on";

/// Node `i`'s `due_on`, from `i` alone so the rest of the fixture draws the same numbers: every second node has one,
/// a day from 1960 to 2029 most of the time, a month one time in ten and a date-time one in ten (from 2020).
fn due_on(i: usize) -> Option<String> {
    use umx_core::time::{DateSpan, date_text, timestamp_text};
    if !i.is_multiple_of(2) {
        return None;
    }
    let k = i / 2;
    let day = |days: i64| DateSpan {
        start: days * 86_400,
        end: days * 86_400 + 86_400,
    };
    // 1960-01-01 is day -3653 of the epoch; 2020-01-01 is day 18262.
    let days = -3653 + (k as i64 * 7919) % 25_567;
    Some(match k % 10 {
        0 => date_text(day(days))?[..7].to_owned(),
        1 => {
            let s = (18_262 + (k as i64 * 7919) % 3653) * 86_400 + (k as i64 * 4243) % 86_400;
            timestamp_text(DateSpan {
                start: s,
                end: s + 1,
            })?
        }
        _ => date_text(day(days))?,
    })
}

const ONSETS: &[&str] = &[
    "b", "d", "f", "g", "k", "l", "m", "n", "p", "r", "s", "t", "v", "z", "br", "dr", "kr", "pl",
    "st", "tr", "sh", "ch", "gl", "fr",
];
const VOWELS: &[&str] = &["a", "e", "i", "o", "u", "ai", "ou", "ea", "io"];
const CODAS: &[&str] = &["", "", "", "n", "r", "s", "l", "m", "k", "nd", "st"];

/// `x^(-s)` for `x >= 1` from IEEE `+ - * /` only: ln by the atanh series on the mantissa, exp by its Taylor series
/// on the remainder after whole powers of two. Deterministic everywhere; accurate to a few ulps, which is plenty for
/// sampling weights.
fn pow_neg(x: f64, s: f64) -> f64 {
    const LN2: f64 = std::f64::consts::LN_2;
    let bits = x.to_bits();
    let e = ((bits >> 52) & 0x7ff) as i64 - 1023;
    let m = f64::from_bits((bits & !(0x7ff << 52)) | (1023 << 52));
    let t = (m - 1.0) / (m + 1.0);
    let (t2, mut term, mut ln_m) = (t * t, t, 0.0);
    for k in 0..40 {
        ln_m += term / (2 * k + 1) as f64;
        term *= t2;
    }
    let y = -s * (e as f64 * LN2 + 2.0 * ln_m);
    let k = (y / LN2).round();
    let r = y - k * LN2;
    let (mut sum, mut term) = (1.0, 1.0);
    for i in 1..30 {
        term *= r / i as f64;
        sum += term;
    }
    sum * f64::from_bits(((k as i64 + 1023) as u64) << 52)
}

/// Draws ranks `0..n` with weight `(rank + 1)^-s`.
pub struct Zipf(Vec<f64>);

impl Zipf {
    pub fn new(n: usize, s: f64) -> Zipf {
        let mut total = 0.0;
        Zipf(
            (1..=n)
                .map(|r| {
                    total += pow_neg(r as f64, s);
                    total
                })
                .collect(),
        )
    }

    pub fn draw(&self, rng: &mut Rng) -> usize {
        let u = (rng.next() >> 11) as f64 / (1u64 << 53) as f64;
        let target = u * self.0[self.0.len() - 1];
        self.0
            .partition_point(|&c| c <= target)
            .min(self.0.len() - 1)
    }
}

/// `n` distinct synthetic words, by rank (0 the most used). 2% hold one non-ASCII letter and 1% end in two digits.
/// Shorter words take the lower ranks (stable by length), as in real text, where the most used words are short.
pub fn vocabulary(rng: &mut Rng, n: usize) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut words = Vec::with_capacity(n);
    while words.len() < n {
        let mut w = String::new();
        for _ in 0..2 + rng.below(3) {
            w.push_str(ONSETS[rng.below(ONSETS.len())]);
            w.push_str(VOWELS[rng.below(VOWELS.len())]);
            w.push_str(CODAS[rng.below(CODAS.len())]);
        }
        match rng.below(100) {
            0 | 1 => {
                let (from, to) = [('e', "é"), ('u', "ü"), ('s', "ß")][rng.below(3)];
                if let Some(at) = w.find(from) {
                    w.replace_range(at..at + 1, to);
                } else {
                    w.push_str(to);
                }
            }
            2 => w.push_str(&format!("{:02}", rng.below(100))),
            _ => {}
        }
        if seen.insert(w.clone()) {
            words.push(w);
        }
    }
    words.sort_by_key(String::len);
    words
}

fn id(n: u64) -> Id {
    Id(((EPOCH_MS as u128) << 80) | n as u128)
}

/// The fixed synthetic graph every external edge points into (never loaded).
pub fn external_graph() -> Id {
    Id((((EPOCH_MS + 86_400_000) as u128) << 80) | 1)
}

pub fn external_node(n: u64) -> Id {
    Id((((EPOCH_MS + 86_400_000) as u128) << 80) | (1000 + n as u128))
}

/// An edge's target while the edges are drawn: a node of this graph by file position, or an external node.
#[derive(Clone, Copy)]
enum End {
    Local(usize),
    External(u64),
}

/// `count` words drawn by Zipf rank, joined by spaces.
fn words(
    rng: &mut Rng,
    zipf: &Zipf,
    vocab: &[String],
    count: usize,
    ranks: &mut Vec<usize>,
) -> String {
    let mut out = String::new();
    for i in 0..count {
        let r = zipf.draw(rng);
        ranks.push(r);
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&vocab[r]);
    }
    out
}

/// The fixture and what the ruler's queries and report need from it.
pub struct Real {
    pub fx: Fixture,
    pub vocab: Vec<String>,
    /// Node ids in file order.
    pub ids: Vec<Id>,
    pub titles: Vec<String>,
    /// Each node's most used title word's rank (the lowest rank), so `>= r` means every title word is past rank `r`.
    pub title_min_rank: Vec<usize>,
    pub meta: GraphMeta,
    /// Distinct vocabulary words used in titles, notes and the text field.
    pub distinct_words: usize,
    pub external_edges: usize,
    /// Nodes holding each evidence kind, in `EVIDENCE_KINDS` order.
    pub kind_nodes: Vec<(String, usize)>,
    pub no_evidence: usize,
    /// The largest local degree (in plus out) and its share of all edges.
    pub top_degree: usize,
    /// Nodes with no part_of children, in file order (for the new-parent edge row).
    pub leaves: Vec<Id>,
    next_id: u64,
}

/// The fixture's ontology: the starter one, with the text and number fields declared on the eight types used, so the
/// text field is word indexed as a real graph's declared text field is.
fn ontology(types: &[&str]) -> Ontology {
    let mut o = Ontology::starter();
    for t in o
        .types
        .iter_mut()
        .filter(|t| types.contains(&t.name.as_str()))
    {
        for (name, kind) in [
            (TEXT_FIELD, FieldKind::String),
            (NUMBER_FIELD, FieldKind::Int),
            (DATE_FIELD, FieldKind::Date),
        ] {
            t.fields.push(FieldDecl {
                name: name.into(),
                kind,
                role: None,
                description: None,
                extra: BTreeMap::new(),
            });
        }
    }
    o
}

/// `nodes` nodes and about `3 * nodes` edges: a part_of tree with fan-out 2 to 20, hub-biased edges on five more
/// relations, and 1% external edges into [`external_graph`]. No duplicate triples, no self-loops.
pub fn generate(seed: u64, nodes: usize) -> Real {
    let mut rng = Rng(seed);
    let vocab = vocabulary(&mut rng, VOCABULARY);
    let zipf = Zipf::new(vocab.len(), ZIPF_S);
    let types = &STARTER_TYPES[..8];
    let statuses = &STARTER_STATUSES[..4];
    let stamp = |i: usize| umx_core::time::iso_from_ms(EPOCH_MS + i as u64 * 1000);
    let mut counter = 1u64;
    let meta = GraphMeta {
        spines: Vec::new(),
        former_aliases: Vec::new(),
        former_paths: Vec::new(),
        umx: 1,
        id: id(counter),
        alias: "ruler".into(),
        title: format!("Synthetic ruler graph (seed {seed}, {nodes} nodes)"),
        created: stamp(0),
        embedding: None,
        ontology: ontology(types),
    };
    let mut used = vec![false; vocab.len()];
    let mut records = Vec::with_capacity(nodes);
    let (mut titles, mut title_min_rank) = (Vec::with_capacity(nodes), Vec::with_capacity(nodes));
    let mut kind_nodes = vec![0usize; EVIDENCE_KINDS.len()];
    let mut no_evidence = 0;
    let mut ranks = Vec::new();
    for i in 0..nodes {
        counter += 1;
        ranks.clear();
        let count = 3 + rng.below(7);
        let title = words(&mut rng, &zipf, &vocab, count, &mut ranks);
        title_min_rank.push(ranks.iter().copied().min().unwrap_or(0));
        let mut note = String::new();
        let mut left = 20 + rng.below(181);
        while left > 0 {
            let n = left.min(8 + rng.below(9));
            if !note.is_empty() {
                note.push(' ');
            }
            note.push_str(&words(&mut rng, &zipf, &vocab, n, &mut ranks));
            note.push('.');
            left -= n;
        }
        let count = 3 + rng.below(6);
        let topic = words(&mut rng, &zipf, &vocab, count, &mut ranks);
        for &r in &ranks {
            used[r] = true;
        }
        // Each kind independently by its share; at most three items, rarest kinds first.
        let mut evidence = Vec::new();
        for (k, (kind, per_mille)) in EVIDENCE_KINDS.iter().enumerate() {
            if (rng.below(1000) as u64) < *per_mille && evidence.len() < 3 {
                let from = 1 + rng.below(5000);
                evidence.push(json!({
                    "source": format!("sources/s{:05}.md", rng.below(20_000)),
                    "seq": [from, from + 1 + rng.below(40)],
                    "kind": kind,
                    "exact": false,
                    "hash": format!("sha256:{:016x}{:016x}{:016x}{:016x}", rng.next(), rng.next(), rng.next(), rng.next()),
                }));
                kind_nodes[k] += 1;
            }
        }
        no_evidence += usize::from(evidence.is_empty());
        let mut fields = BTreeMap::new();
        fields.insert(TEXT_FIELD.to_owned(), json!(topic));
        fields.insert(NUMBER_FIELD.to_owned(), json!(1 + rng.below(100)));
        if let Some(due) = due_on(i) {
            fields.insert(DATE_FIELD.to_owned(), json!(due));
        }
        titles.push(title.clone());
        records.push(Node {
            id: id(counter),
            kind: types[rng.below(types.len())].to_owned(),
            title,
            status: statuses[rng.below(statuses.len())].to_owned(),
            note,
            evidence,
            status_history: Vec::new(),
            created: stamp(i),
            updated: stamp(i),
            created_by: "umx-bench".into(),
            locator: None,
            authored_by_session: None,
            edited_by_session: None,
            fields,
        });
    }
    let ids: Vec<Id> = records.iter().map(|n| n.id).collect();
    let target = 3 * nodes;
    let external = target / 100;
    let mut triples: BTreeSet<(u32, u8, u32)> = BTreeSet::new();
    // The part_of tree: each parent takes 2 to 20 children in file order.
    let (mut parent, mut left) = (0usize, 2 + rng.below(19));
    let mut has_child = vec![false; nodes];
    let mut tree = Vec::with_capacity(nodes);
    for i in 1..nodes {
        if left == 0 {
            parent += 1;
            left = 2 + rng.below(19);
        }
        left -= 1;
        has_child[parent] = true;
        tree.push((i, parent));
    }
    for &(child, parent) in &tree {
        triples.insert((child as u32, 0, parent as u32));
    }
    // Hubs: Zipf over a seeded order of the nodes, so the hubs are spread through the file.
    let mut order: Vec<usize> = (0..nodes).collect();
    for i in (1..nodes).rev() {
        order.swap(i, rng.below(i + 1));
    }
    let hub = Zipf::new(nodes, ZIPF_S);
    let random = target.saturating_sub(tree.len() + external);
    let mut added = 0;
    while added < random && nodes > 1 {
        let from = rng.below(nodes);
        let to = if rng.below(100) < HUB_PERCENT {
            order[hub.draw(&mut rng)]
        } else {
            rng.below(nodes)
        };
        let rel = 1 + rng.below(RELATIONS.len() - 1) as u8;
        if from != to && triples.insert((from as u32, rel, to as u32)) {
            added += 1;
        }
    }
    let mut ext_seen = BTreeSet::new();
    while ext_seen.len() < external && nodes > 0 {
        let from = rng.below(nodes);
        let to = rng.below(1000) as u64;
        let rel = 1 + rng.below(RELATIONS.len() - 1) as u8;
        ext_seen.insert((from, rel, to));
    }
    // Edges in a seeded order, so ids do not follow the triple order.
    let mut all: Vec<(usize, u8, End)> = triples
        .iter()
        .map(|&(f, r, t)| (f as usize, r, End::Local(t as usize)))
        .chain(ext_seen.iter().map(|&(f, r, t)| (f, r, End::External(t))))
        .collect();
    for i in (1..all.len()).rev() {
        all.swap(i, rng.below(i + 1));
    }
    let mut degree = vec![0usize; nodes];
    let mut edges = Vec::with_capacity(all.len());
    for (k, &(from, rel, to)) in all.iter().enumerate() {
        counter += 1;
        let (to_graph, to) = match to {
            End::Local(t) => {
                degree[t] += 1;
                (None, ids[t])
            }
            End::External(t) => (Some(external_graph()), external_node(t)),
        };
        degree[from] += 1;
        edges.push(Edge {
            id: id(counter),
            kind: RELATIONS[rel as usize].to_owned(),
            from_graph: None,
            from: ids[from],
            to_graph,
            to,
            created: stamp(k % nodes.max(1)),
            created_by: "umx-bench".into(),
            authored_by_session: None,
            edited_by_session: None,
            evidence: Vec::new(),
            fields: BTreeMap::new(),
        });
    }
    let leaves = (0..nodes)
        .filter(|&i| !has_child[i])
        .map(|i| ids[i])
        .collect();
    Real {
        fx: Fixture {
            graph: codec::encode_graph(&meta),
            nodes: codec::encode_nodes(&records),
            edges: codec::encode_edges(&edges),
            node_count: records.len(),
            edge_count: edges.len(),
        },
        distinct_words: used.iter().filter(|u| **u).count(),
        vocab,
        ids,
        titles,
        title_min_rank,
        meta,
        external_edges: external,
        kind_nodes: EVIDENCE_KINDS
            .iter()
            .zip(kind_nodes)
            .map(|((k, _), n)| ((*k).to_owned(), n))
            .collect(),
        no_evidence,
        top_degree: degree.iter().copied().max().unwrap_or(0),
        leaves,
        next_id: counter + 1,
    }
}

/// Deliberate duplicate entity keys in the keyed variant (the duplicate path runs on load).
pub const DUPLICATE_KEYS: usize = 20;

/// One keyed node's key from its unique number `n`: a `locator` (repo paths, `file#Lnn`, 10% URLs) as `Ok`, or an
/// `entity_key` (`email:`, `id:`, `domain:`) as `Err`.
fn new_key(rng: &mut Rng, base: &Real, n: usize, locator: bool) -> Result<Locator, String> {
    if locator {
        let w = base.vocab[rng.below(2000)].clone();
        let text = match rng.below(10) {
            0 => format!("https://docs.example.org/{w}/page-{n}"),
            1..=5 => format!("src/{w}/mod_{n}.rs"),
            _ => format!("notes/{w}/file_{n}.md#L{}", 1 + rng.below(400)),
        };
        return Ok(Locator::parse(&text).expect("a synthetic locator parses"));
    }
    Err(match rng.below(3) {
        0 => format!("email:user{n}@example.org"),
        // An id: key holds a 26-character umx id (entity.rs), here a synthetic one.
        1 => format!(
            "id:{}",
            Id((((EPOCH_MS + 2 * 86_400_000) as u128) << 80) | n as u128).to_text()
        ),
        _ => format!("domain:host{n}.example.net"),
    })
}

/// Unique numbers `0..count` in random order: keys made from them arrive unsorted.
fn shuffled(rng: &mut Rng, count: usize) -> Vec<usize> {
    let mut numbers: Vec<usize> = (0..count).collect();
    for i in (1..count).rev() {
        numbers.swap(i, rng.below(i + 1));
    }
    numbers
}

/// The kind the `distinct` fixture's `=` row looks for: node 500's own kind.
pub const DISTINCT_KIND: &str = "k500";

/// The one-kind-per-node variant (`distinct`, ws003 s29): the realistic fixture with every evidence item's string
/// `kind` on node i (file order) rewritten to `k<i>`, so each node with evidence holds a kind no other node holds.
pub fn distinct(base: &Real) -> Fixture {
    let mut records = codec::decode_nodes(&base.fx.nodes)
        .expect("the base decodes")
        .records;
    for (i, n) in records.iter_mut().enumerate() {
        for item in &mut n.evidence {
            if let Some(kind) = item.get_mut("kind").filter(|k| k.is_string()) {
                *kind = json!(format!("k{i}"));
            }
        }
    }
    Fixture {
        graph: base.fx.graph.clone(),
        nodes: codec::encode_nodes(&records),
        edges: base.fx.edges.clone(),
        node_count: records.len(),
        edge_count: base.fx.edge_count,
    }
}

/// The keyed variant (`fixture_real_keyed`, B2): the base plus `extra` nodes after every base id, half with a
/// `locator` and half with an `entity_key` ([`new_key`]), all unique and in random order in the file, then
/// [`DUPLICATE_KEYS`] of the keyed nodes reuse an earlier key.
pub fn keyed(seed: u64, base: &Real, extra: usize) -> Fixture {
    let mut rng = Rng(seed ^ 0x006b_6579_6564);
    let mut records = codec::decode_nodes(&base.fx.nodes)
        .expect("the base decodes")
        .records;
    let half = extra / 2;
    let numbers = shuffled(&mut rng, extra);
    let word = |rng: &mut Rng| base.vocab[rng.below(2000)].clone();
    let stamp = umx_core::time::iso_from_ms(EPOCH_MS);
    // Each unique key with its node's type: a duplicate counts only within one type (D63), so it copies both.
    let mut keys: Vec<(String, &str)> = Vec::new();
    for (i, &n) in numbers.iter().enumerate() {
        let mut fields = BTreeMap::new();
        let mut locator = None;
        let mut kind = STARTER_TYPES[rng.below(8)];
        match new_key(&mut rng, base, n, i < half) {
            Ok(l) => locator = Some(l),
            Err(key) => {
                // The last DUPLICATE_KEYS keyed nodes each reuse a different earlier node's key and type.
                let dup = (i + DUPLICATE_KEYS).checked_sub(extra);
                let key = match dup {
                    Some(d) if keys.len() >= DUPLICATE_KEYS => {
                        let (key, original) = keys[d * keys.len() / DUPLICATE_KEYS].clone();
                        kind = original;
                        key
                    }
                    _ => {
                        keys.push((key.clone(), kind));
                        key
                    }
                };
                fields.insert(umx_core::entity::ENTITY_KEY.to_owned(), json!([key]));
            }
        }
        records.push(Node {
            id: id(base.next_id + i as u64),
            kind: kind.to_owned(),
            title: format!("{} {}", word(&mut rng), word(&mut rng)),
            status: STARTER_STATUSES[0].to_owned(),
            note: String::new(),
            evidence: Vec::new(),
            status_history: Vec::new(),
            created: stamp.clone(),
            updated: stamp.clone(),
            created_by: "umx-bench".into(),
            locator,
            authored_by_session: None,
            edited_by_session: None,
            fields,
        });
    }
    Fixture {
        graph: base.fx.graph.clone(),
        nodes: codec::encode_nodes(&records),
        edges: base.fx.edges.clone(),
        node_count: records.len(),
        edge_count: base.fx.edge_count,
    }
}

/// The all-keyed variant (`keyed100k`, B2 at full size): every base node gets a key ([`new_key`]), a `locator` or an
/// `entity_key` by the parity of a shuffled unique number, so keys arrive unsorted and the two families interleave.
/// Then up to [`DUPLICATE_KEYS`] of the last entity-keyed nodes each reuse the key of a different node of the same
/// type from the first half of the entity-keyed nodes. A small base has fewer pairs, or none. Ids, types, titles and
/// edges stay the base's. Returns the fixture and how many duplicate keys it holds.
pub fn keyed_all(seed: u64, base: &Real) -> (Fixture, usize) {
    let mut rng = Rng(seed ^ 0x6b65_7965_645f_616c);
    let mut records = codec::decode_nodes(&base.fx.nodes)
        .expect("the base decodes")
        .records;
    let numbers = shuffled(&mut rng, records.len());
    for (node, &n) in records.iter_mut().zip(&numbers) {
        match new_key(&mut rng, base, n, n % 2 == 0) {
            Ok(l) => node.locator = Some(l),
            Err(key) => {
                node.fields
                    .insert(umx_core::entity::ENTITY_KEY.to_owned(), json!([key]));
            }
        }
    }
    let made = reuse_keys(&mut records);
    let fx = Fixture {
        graph: base.fx.graph.clone(),
        nodes: codec::encode_nodes(&records),
        edges: base.fx.edges.clone(),
        node_count: records.len(),
        edge_count: base.fx.edge_count,
    };
    (fx, made)
}

/// Up to [`DUPLICATE_KEYS`] of the last entity-keyed records each take the `entity_key` of a different record of the
/// same type from the first half of the entity-keyed records, so no record is both. A late record with no such donor
/// left keeps its own key. Returns how many keys were reused.
fn reuse_keys(records: &mut [Node]) -> usize {
    let entity: Vec<usize> = (0..records.len())
        .filter(|&i| records[i].fields.contains_key(umx_core::entity::ENTITY_KEY))
        .collect();
    let (early, late) = entity.split_at(entity.len() / 2);
    let late = &late[late.len().saturating_sub(DUPLICATE_KEYS)..];
    let mut taken = BTreeSet::new();
    for &l in late {
        let Some(donor) = early
            .iter()
            .copied()
            .find(|e| !taken.contains(e) && records[*e].kind == records[l].kind)
        else {
            continue;
        };
        taken.insert(donor);
        let key = records[donor].fields[umx_core::entity::ENTITY_KEY].clone();
        records[l]
            .fields
            .insert(umx_core::entity::ENTITY_KEY.to_owned(), key);
    }
    taken.len()
}

/// The ruler's fixed query lists and picks, drawn from the seed (written beside each run's result).
#[derive(Debug)]
pub struct Queries {
    /// 2 words from vocabulary ranks 20k to 60k.
    pub rare: Vec<String>,
    /// 2 words, one of them a top-20 word.
    pub common: Vec<String>,
    /// Titles of nodes whose words are all past rank 1,000, one letter changed per word.
    pub typo: Vec<String>,
    /// A single top-5 word.
    pub wide: Vec<String>,
    /// New titles for the near-duplicate check.
    pub near: Vec<String>,
    /// Node pairs for path, and walk starts, as id text.
    pub pairs: Vec<(String, String)>,
    pub starts: Vec<String>,
}

/// One letter of each word changed (the middle letter, to the next one in `a..z`; a non-letter becomes `x`).
pub fn typo(title: &str) -> String {
    title
        .split(' ')
        .map(|w| {
            let chars: Vec<char> = w.chars().collect();
            let mid = chars.len() / 2;
            chars
                .iter()
                .enumerate()
                .map(|(i, &c)| match (i == mid, c) {
                    (false, c) => c,
                    (true, 'z') => 'a',
                    (true, c) if c.is_ascii_lowercase() => (c as u8 + 1) as char,
                    (true, _) => 'x',
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn queries(seed: u64, real: &Real) -> Queries {
    let mut rng = Rng(seed ^ 0x0071_7565_7279);
    let v = &real.vocab;
    let lo = 20_000.min(v.len() - 1);
    let rare_word = |rng: &mut Rng| v[lo + rng.below(v.len() - lo)].clone();
    let rare = (0..50)
        .map(|_| format!("{} {}", rare_word(&mut rng), rare_word(&mut rng)))
        .collect();
    let common = (0..50)
        .map(|_| format!("{} {}", v[rng.below(20)], v[20 + rng.below(v.len() - 20)]))
        .collect();
    let n = real.ids.len();
    let start = rng.below(n);
    let typo_titles = (0..n)
        .map(|k| (start + k) % n)
        .filter(|&i| {
            real.title_min_rank[i] >= 1000 && real.titles[i].split(' ').all(|w| w.len() >= 4)
        })
        .take(50)
        .map(|i| typo(&real.titles[i]))
        .collect();
    let wide = (0..20).map(|i| v[i % 5].clone()).collect();
    let zipf = Zipf::new(v.len(), ZIPF_S);
    let mut sink = Vec::new();
    let near = (0..50)
        .map(|_| {
            let count = 3 + rng.below(7);
            words(&mut rng, &zipf, v, count, &mut sink)
        })
        .collect();
    let pairs = (0..50)
        .map(|_| {
            (
                real.ids[rng.below(n)].to_text(),
                real.ids[rng.below(n)].to_text(),
            )
        })
        .collect();
    let starts = (0..50).map(|_| real.ids[rng.below(n)].to_text()).collect();
    Queries {
        rare,
        common,
        typo: typo_titles,
        wide,
        near,
        pairs,
        starts,
    }
}

/// The same row set on the old rich fixture (64 words): its own titles as the find queries, its own ids as the
/// path pairs and walk starts. `titles` are the rich graph's titles in id order, `ids` its node ids.
pub fn rich_picks(seed: u64, ids: &[Id], titles: &[String]) -> Queries {
    let mut rng = Rng(seed ^ 0x7269_6368);
    let n = ids.len();
    let title = |rng: &mut Rng| titles[rng.below(n)].clone();
    let two = |t: String| t.split(' ').take(2).collect::<Vec<_>>().join(" ");
    Queries {
        rare: (0..50).map(|_| two(title(&mut rng))).collect(),
        common: (0..50).map(|_| two(title(&mut rng))).collect(),
        typo: (0..50).map(|_| typo(&title(&mut rng))).collect(),
        wide: (0..20)
            .map(|_| title(&mut rng).split(' ').next().unwrap_or("").to_owned())
            .collect(),
        near: (0..50).map(|_| title(&mut rng) + " added").collect(),
        pairs: (0..50)
            .map(|_| (ids[rng.below(n)].to_text(), ids[rng.below(n)].to_text()))
            .collect(),
        starts: (0..50).map(|_| ids[rng.below(n)].to_text()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn hash(fx: &Fixture) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(&fx.graph);
        h.update(&fx.nodes);
        h.update(&fx.edges);
        h.finalize().into()
    }

    #[test]
    fn same_seed_same_bytes_and_another_seed_differs() {
        let (a, b) = (generate(1, 3000), generate(1, 3000));
        assert_eq!(hash(&a.fx), hash(&b.fx));
        assert_eq!(hash(&keyed(1, &a, 300)), hash(&keyed(1, &b, 300)));
        assert_eq!(hash(&keyed_all(1, &a).0), hash(&keyed_all(1, &b).0));
        // The plant: a different seed (as a seed from the clock would be) must change the bytes.
        // graph.json's title names the seed, so the plant compares the nodes and edges bytes alone.
        let other = generate(2, 3000).fx;
        assert_ne!(
            (other.nodes, other.edges),
            (a.fx.nodes.clone(), a.fx.edges.clone())
        );
    }

    #[test]
    fn pow_neg_matches_the_platform_closely() {
        for x in [1.0, 2.0, 3.0, 17.0, 1000.0, 59_999.0, 100_000.0] {
            let (ours, theirs) = (pow_neg(x, ZIPF_S), x.powf(-ZIPF_S));
            assert!(
                ((ours - theirs) / theirs).abs() < 1e-12,
                "{x}: {ours} {theirs}"
            );
        }
    }

    #[test]
    fn the_fixture_has_the_spec_shape_and_decodes() {
        let r = generate(1, 5000);
        let nodes = codec::decode_nodes(&r.fx.nodes).unwrap().records;
        let edges = codec::decode_edges(&r.fx.edges).unwrap().records;
        assert_eq!(nodes.len(), 5000);
        assert_eq!(edges.len(), 15_000);
        assert_eq!(edges.iter().filter(|e| e.to_graph.is_some()).count(), 150);
        assert_eq!(edges.iter().filter(|e| e.kind == PART_OF).count(), 4999);
        let triples: BTreeSet<_> = edges.iter().map(|e| (e.from, &e.kind, e.to)).collect();
        assert_eq!(triples.len(), edges.len(), "no duplicate triples");
        assert!(edges.iter().all(|e| e.from != e.to));
        assert!(
            nodes
                .iter()
                .all(|n| (3..=9).contains(&n.title.split(' ').count()))
        );
        let share = |k: &str| r.kind_nodes.iter().find(|(n, _)| n == k).unwrap().1 as f64 / 5000.0;
        assert!(
            (0.01..0.03).contains(&share(RARE_KIND)),
            "{}",
            share(RARE_KIND)
        );
        assert!(
            (0.35..0.45).contains(&share(COMMON_KIND)),
            "{}",
            share(COMMON_KIND)
        );
        assert!((0.25..0.35).contains(&(r.no_evidence as f64 / 5000.0)));
        assert!(r.vocab.iter().any(|w| !w.is_ascii()));
        assert!(
            r.vocab
                .iter()
                .any(|w| w.bytes().any(|b| b.is_ascii_digit()))
        );
        let k = keyed(1, &r, 500);
        let keyed_nodes = codec::decode_nodes(&k.nodes).unwrap().records;
        assert_eq!(keyed_nodes.len(), 5500);
        assert_eq!(
            keyed_nodes.iter().filter(|n| n.locator.is_some()).count(),
            250
        );
        assert_eq!(
            keyed_nodes
                .iter()
                .filter(|n| n.fields.contains_key(umx_core::entity::ENTITY_KEY))
                .count(),
            250
        );
        assert_eq!(duplicate_pairs(&keyed_nodes), DUPLICATE_KEYS);
        // keyed100k: the same nodes, edges and types, every node keyed, the two families near half each.
        let (all, made) = keyed_all(1, &r);
        let all = codec::decode_nodes(&all.nodes).unwrap().records;
        assert_eq!(all.len(), 5000);
        assert!(
            all.iter()
                .zip(&nodes)
                .all(|(a, b)| a.id == b.id && a.kind == b.kind)
        );
        let located = all.iter().filter(|n| n.locator.is_some()).count();
        let held = all
            .iter()
            .filter(|n| n.fields.contains_key(umx_core::entity::ENTITY_KEY))
            .count();
        assert_eq!((located + held, located), (5000, 2500));
        assert_eq!(
            (duplicate_pairs(&all), made),
            (DUPLICATE_KEYS, DUPLICATE_KEYS)
        );
    }

    /// How many (type, entity key) pairs two nodes share; no pair is held three times.
    fn duplicate_pairs(nodes: &[Node]) -> usize {
        let mut held: BTreeMap<(String, String), usize> = BTreeMap::new();
        for n in nodes {
            if let Some(k) = n.fields.get(umx_core::entity::ENTITY_KEY) {
                *held.entry((n.kind.clone(), k.to_string())).or_default() += 1;
            }
        }
        assert!(held.values().all(|c| *c <= 2));
        held.values().filter(|c| **c == 2).count()
    }

    /// review (B1): with fewer than 2 * DUPLICATE_KEYS entity-keyed nodes, the all-keyed variant still
    /// tried to make 20 duplicates, and panicked (an underflow at 39 nodes, no donor at 40). Small bases now make the
    /// pairs they can, and say how many.
    #[test]
    fn the_all_keyed_variant_fits_small_bases() {
        for n in [1, 3, 39, 40, 41, 100] {
            let r = generate(1, n);
            let (fx, made) = keyed_all(1, &r);
            let all = codec::decode_nodes(&fx.nodes).unwrap().records;
            assert_eq!(all.len(), n);
            assert!(
                all.iter()
                    .all(|x| x.locator.is_some()
                        != x.fields.contains_key(umx_core::entity::ENTITY_KEY)),
                "{n}: every node holds exactly one key"
            );
            let entity = all.iter().filter(|x| x.locator.is_none()).count();
            assert!(
                made <= DUPLICATE_KEYS.min(entity / 2),
                "{n}: {made} of {entity}"
            );
            assert_eq!(duplicate_pairs(&all), made, "{n}");
        }
    }

    /// Entity-keyed records of the given types, each with its own key, plus a plain record between them.
    fn keyed_records(kinds: &[&str]) -> Vec<Node> {
        let mut out = Vec::new();
        for (i, kind) in kinds.iter().enumerate() {
            for (j, key) in [None, Some(format!("email:u{i}@example.org"))]
                .into_iter()
                .enumerate()
            {
                out.push(Node {
                    id: id(2 * i as u64 + j as u64),
                    kind: (*kind).to_owned(),
                    title: "t".into(),
                    status: STARTER_STATUSES[0].to_owned(),
                    note: String::new(),
                    evidence: Vec::new(),
                    status_history: Vec::new(),
                    created: String::new(),
                    updated: String::new(),
                    created_by: "umx-bench".into(),
                    locator: None,
                    authored_by_session: None,
                    edited_by_session: None,
                    fields: key
                        .map(|k| [(umx_core::entity::ENTITY_KEY.to_owned(), json!([k]))].into())
                        .unwrap_or_default(),
                });
            }
        }
        out
    }

    /// B1's two edges: the split between donors and duplicates at every small count (an odd count leaves the extra
    /// record late; never more than DUPLICATE_KEYS), and a late record whose type no early record has.
    #[test]
    fn reused_keys_stop_at_the_split_and_at_a_missing_donor() {
        for (count, want) in [
            (0, 0),
            (1, 0),
            (2, 1),
            (19, 9),
            (39, 19),
            (40, 20),
            (41, 20),
            (100, 20),
        ] {
            let mut records = keyed_records(&vec!["a"; count]);
            assert_eq!(reuse_keys(&mut records), want, "{count}");
            assert_eq!(duplicate_pairs(&records), want, "{count}");
        }
        // The early half is all type a; of the late half, one record is type b and has no donor.
        let mut kinds = vec!["a"; 40];
        kinds[39] = "b";
        let mut records = keyed_records(&kinds);
        assert_eq!(reuse_keys(&mut records), 19);
        assert_eq!(duplicate_pairs(&records), 19);
        // No late type has an early donor: nothing is reused.
        let mut records = keyed_records(&[["a"; 20], ["b"; 20]].concat());
        assert_eq!(reuse_keys(&mut records), 0);
        assert_eq!(duplicate_pairs(&records), 0);
    }
}

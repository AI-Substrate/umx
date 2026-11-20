//! Exact similarity over int8 vectors (workshop 008 section 2.5): a dot product, the inverse length that turns it
//! into a cosine, and a bounded top-k scan over one contiguous block of rows. Pure functions over slices; the blocks
//! themselves live in umx-graph. Single threaded and exact (D12).

use alloc::collections::BinaryHeap;
use alloc::vec::Vec;
use core::cmp::Ordering;

/// The dot product of two int8 vectors of the same length, exactly (256 dims of +-127 fit an `i32` many times over).
#[inline]
pub fn dot(a: &[i8], b: &[i8]) -> i32 {
    debug_assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(&x, &y)| i32::from(x) * i32::from(y))
        .sum()
}

/// `1 / |v|`, so `dot(a, b) * inverse_length(a) * inverse_length(b)` is the cosine. 0 for a zero vector, whose
/// cosine with anything is then 0.
pub fn inverse_length(v: &[i8]) -> f32 {
    let squares: u64 = v
        .iter()
        .map(|&x| (i64::from(x) * i64::from(x)) as u64)
        .sum();
    if squares == 0 {
        return 0.0;
    }
    (1.0 / libm::sqrt(squares as f64)) as f32
}

/// What a floor cuts: a search's vector leg, or a node's stored similar_to list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Use {
    Search,
    Similar,
}

/// Where a floor came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloorSource {
    /// The calibrated table ([`CALIBRATED`]), from a recorded run.
    Calibrated,
    /// The machine config: `embedders.<name>.search_floor` or `similar_floor`.
    Config,
    /// graph.json's `embedding.similarity.floor` (similar_to only).
    Graph,
    /// No calibration for this key: no floor (0.0), so only k limits.
    None,
}

impl FloorSource {
    pub fn word(self) -> &'static str {
        match self {
            FloorSource::Calibrated => "calibrated",
            FloorSource::Config => "config",
            FloorSource::Graph => "graph",
            FloorSource::None => "none",
        }
    }
}

/// A floor with its provenance. Only a [`FloorSource::Calibrated`] floor is calibrated; `run` names its run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Floor {
    pub value: f32,
    pub source: FloorSource,
    pub run: Option<&'static str>,
}

impl Floor {
    pub const NONE: Floor = Floor {
        value: 0.0,
        source: FloorSource::None,
        run: None,
    };

    pub fn calibrated(&self) -> bool {
        self.source == FloorSource::Calibrated
    }
}

/// One key's calibrated floors (workstream 004 s26, embeddings-expert ruling 2026-11-11): `search` is the p99 of
/// query-to-random-document cosines (lowered only to keep BEIR hybrid nDCG@10 within 0.01 of no floor), `similar`
/// the p99 of document-to-random-document cosines, both over int8 vectors as umx stores them. `strength` holds the
/// key's strength cuts (PRD 002 search-10), None when they were not measured for it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Calibrated {
    pub model: &'static str,
    pub dims: usize,
    pub prompt_version: Option<u32>,
    pub search: f32,
    pub similar: f32,
    pub run: &'static str,
    pub strength: Option<StrengthCuts>,
}

/// PRD 002 search-10: how strongly a hit found by meaning matches, measured against every node searched.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Strength {
    Weak,
    Fair,
    Strong,
    /// The hit's whole value equals the query (an exact match), whatever its vectors say.
    Exact,
}

impl Strength {
    pub fn word(self) -> &'static str {
        match self {
            Strength::Weak => "weak",
            Strength::Fair => "fair",
            Strength::Strong => "strong",
            Strength::Exact => "exact",
        }
    }
}

/// One key's strength cuts (PRD 002 search-10, embeddings-expert ruling on search-10), measured by umx-bench's
/// floor-calibration `--strength` and recorded beside the floors. A hit is strong when BOTH its robust z (its
/// whole-node cosine against the median and MAD of every searched node's whole-node cosine) is at least
/// `strong_z + strong_z_per_ln * ln N` (N the nodes in the population: the maximum of N off-topic draws grows with
/// N) AND its cosine is at least `strong_score`; fair likewise with the fair cuts; else weak. A population below
/// `min_population` nodes, or with a MAD of 0, labels nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrengthCuts {
    pub strong_z: f32,
    pub strong_z_per_ln: f32,
    pub strong_score: f32,
    pub fair_z: f32,
    pub fair_z_per_ln: f32,
    pub fair_score: f32,
    pub min_population: usize,
    /// Whether the z cuts apply. False is the cosine-only rule: a label needs only the cosine cuts (and the population
    /// minimum), so the record alone can switch the rule's shape.
    pub z_part: bool,
    /// The largest population the cuts were measured on. Above twice it nothing is labelled (the z cuts are a fit on
    /// ln N, never checked that far), until a larger corpus is measured.
    pub max_n_measured: usize,
    /// The strength run that measured these cuts, from the rows of the key's floors run.
    pub run: &'static str,
}

impl StrengthCuts {
    /// The z a strong hit needs in a population of `n` nodes.
    pub fn strong_z_at(&self, n: usize) -> f64 {
        f64::from(self.strong_z) + f64::from(self.strong_z_per_ln) * libm::log(n.max(1) as f64)
    }

    /// The z a fair hit needs in a population of `n` nodes.
    pub fn fair_z_at(&self, n: usize) -> f64 {
        f64::from(self.fair_z) + f64::from(self.fair_z_per_ln) * libm::log(n.max(1) as f64)
    }
}

/// The scores of every node searched in one vector space for one phrasing, summarised by order statistics only: the
/// lower median `median` and the lower median of `|s - median|` (`mad`). Selection by `total_cmp`, no sums, so the
/// result depends only on the multiset of scores, never on their order or the machine.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Population {
    pub n: usize,
    pub median: f32,
    pub mad: f32,
}

impl Population {
    /// The statistics of `scores` (reordered in place). None when empty.
    pub fn of(scores: &mut [f32]) -> Option<Population> {
        let n = scores.len();
        let median = lower_median(scores)?;
        for s in scores.iter_mut() {
            *s = (*s - median).abs();
        }
        let mad = lower_median(scores)?;
        Some(Population { n, median, mad })
    }

    /// The robust z of a score: `(s - median) / mad`. None when the MAD is 0 (no spread to measure against).
    pub fn z(&self, s: f32) -> Option<f64> {
        (self.mad > 0.0).then(|| (f64::from(s) - f64::from(self.median)) / f64::from(self.mad))
    }
}

/// The element of rank `(n - 1) / 2` by `total_cmp`: the median for odd n, the lower of the middle two for even n.
fn lower_median(v: &mut [f32]) -> Option<f32> {
    if v.is_empty() {
        return None;
    }
    let mid = (v.len() - 1) / 2;
    Some(*v.select_nth_unstable_by(mid, f32::total_cmp).1)
}

/// The label of a hit whose whole-node cosine is `s` in `population`, by `cuts`. None (no label) when the population
/// is below the key's `min_population` or above twice its `max_n_measured`, or, when the z cuts apply, has no spread.
pub fn strength(population: &Population, cuts: &StrengthCuts, s: f32) -> Option<Strength> {
    if !labels(population, cuts) {
        return None;
    }
    let (strong, fair) = if cuts.z_part {
        let z = population.z(s)?;
        (
            z >= cuts.strong_z_at(population.n),
            z >= cuts.fair_z_at(population.n),
        )
    } else {
        (true, true)
    };
    Some(if strong && s >= cuts.strong_score {
        Strength::Strong
    } else if fair && s >= cuts.fair_score {
        Strength::Fair
    } else {
        Strength::Weak
    })
}

/// Whether `population` can label anything by `cuts`: at least the key's minimum, at most twice the largest
/// population measured, and some spread when the z cuts apply.
pub fn labels(population: &Population, cuts: &StrengthCuts) -> bool {
    population.n >= cuts.min_population
        && population.n <= cuts.max_n_measured.saturating_mul(2)
        && (!cuts.z_part || population.mad > 0.0)
}

/// The strength cuts for `key`: its calibrated row's, else None (an uncalibrated key or a test fake labels nothing).
pub fn strength_cuts(key: &crate::ports::ModelKey) -> Option<StrengthCuts> {
    CALIBRATED
        .iter()
        .find(|c| {
            c.model == key.model()
                && c.dims == key.dims()
                && c.prompt_version == key.prompt_version()
        })
        .and_then(|c| c.strength)
}

/// The calibrated floors, one row per key. The record is docs/project/benchmarks/floor-calibration/floors.json and a
/// test checks this table against it; recalibrate when a key's model, dims, prompt version or text form changes.
pub const CALIBRATED: &[Calibrated] = &[
    Calibrated {
        model: "embeddinggemma-2",
        dims: 256,
        prompt_version: Some(1),
        search: 0.7101,
        similar: 0.8462,
        run: "fc-2026-11-11-cdc1965",
        strength: Some(StrengthCuts {
            strong_z: 5.302,
            strong_z_per_ln: 0.5996,
            strong_score: 0.7382,
            fair_z: -1.5398,
            fair_z_per_ln: 1.1069,
            fair_score: 0.7117,
            min_population: 100,
            z_part: true,
            max_n_measured: 8674,
            run: "sc-2026-11-19-c5485aa",
        }),
    },
    Calibrated {
        model: "text-embedding-3-large",
        dims: 256,
        prompt_version: None,
        search: 0.409,
        similar: 0.6133,
        run: "fc-2026-11-11-cdc1965",
        strength: Some(StrengthCuts {
            strong_z: 3.3557,
            strong_z_per_ln: 0.6124,
            strong_score: 0.4728,
            fair_z: -0.797,
            fair_z_per_ln: 0.8843,
            fair_score: 0.409,
            min_population: 200,
            z_part: true,
            max_n_measured: 8674,
            run: "sc-2026-11-19-c5485aa",
        }),
    },
    Calibrated {
        model: "text-embedding-3-small",
        dims: 256,
        prompt_version: None,
        search: 0.4095,
        similar: 0.5807,
        run: "fc-2026-11-11-cdc1965",
        strength: Some(StrengthCuts {
            strong_z: 6.0899,
            strong_z_per_ln: 0.5293,
            strong_score: 0.4883,
            fair_z: 1.0806,
            fair_z_per_ln: 0.6599,
            fair_score: 0.4095,
            min_population: 100,
            z_part: true,
            max_n_measured: 8674,
            run: "sc-2026-11-19-c5485aa",
        }),
    },
];

/// The default floor for `key` and `use_`: its calibrated row, else none (0.0, so only k limits). A fake model
/// (`fake*`, the test embedders) is never calibrated and has no floor, so its tests see every row.
pub fn floor(key: &crate::ports::ModelKey, use_: Use) -> Floor {
    CALIBRATED
        .iter()
        .find(|c| {
            c.model == key.model()
                && c.dims == key.dims()
                && c.prompt_version == key.prompt_version()
        })
        .map_or(Floor::NONE, |c| Floor {
            value: match use_ {
                Use::Search => c.search,
                Use::Similar => c.similar,
            },
            source: FloorSource::Calibrated,
            run: Some(c.run),
        })
}

/// The uses of `key` that cut nothing because no floor applies to them, given the floor each one will use (the
/// effective floors: machine config, graph.json for similar_to, else the calibrated table): `search`, `similar_to`,
/// or `search and similar_to`. None when both have a floor, and always for a test fake (`fake*`).
pub fn uses_without_floor(
    key: &crate::ports::ModelKey,
    search: FloorSource,
    similar: FloorSource,
) -> Option<&'static str> {
    if key.model().starts_with("fake") {
        return None;
    }
    match (search == FloorSource::None, similar == FloorSource::None) {
        (true, true) => Some("search and similar_to"),
        (true, false) => Some("search"),
        (false, true) => Some("similar_to"),
        (false, false) => None,
    }
}

/// The machine config's floors (`embedders.<name>.search_floor`, `similar_floor`), by model key. When two instances
/// of one key set a floor, the first set (instances go in name order) is the one used.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Floors {
    overrides: alloc::collections::BTreeMap<alloc::string::String, (Option<f32>, Option<f32>)>,
}

impl Floors {
    pub fn set(&mut self, key: &crate::ports::ModelKey, search: Option<f32>, similar: Option<f32>) {
        use alloc::string::ToString;
        let entry = self
            .overrides
            .entry(key.to_string())
            .or_insert((None, None));
        entry.0 = entry.0.or(search);
        entry.1 = entry.1.or(similar);
    }

    /// The floor `key` uses for `use_`: the config's, else [`floor`].
    pub fn get(&self, key: &crate::ports::ModelKey, use_: Use) -> Floor {
        use alloc::string::ToString;
        let set = self
            .overrides
            .get(&key.to_string())
            .and_then(|(s, d)| match use_ {
                Use::Search => *s,
                Use::Similar => *d,
            });
        match set {
            Some(value) => Floor {
                value,
                source: FloorSource::Config,
                run: None,
            },
            None => floor(key, use_),
        }
    }
}

/// The most dims whose int8 dot product (at most `dims * 127 * 127`) converts to `f32` exactly.
pub const MAX_EXACT_DIMS: usize = 1024;

/// One scan result: the row in the block and its cosine with the query.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub row: u32,
    pub score: f32,
}

/// Orders hits worst first: lower score, then higher row. The heap's top is the hit to drop.
struct Worst(Hit);

impl PartialEq for Worst {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Worst {}

impl PartialOrd for Worst {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Worst {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .0
            .score
            .total_cmp(&self.0.score)
            .then(self.0.row.cmp(&other.0.row))
    }
}

/// The `k` rows of `rows` (row `i` is `rows[i * dims..(i + 1) * dims]`, `dims = query.len()`) most similar to
/// `query` by cosine, best first, ties by lower row. `inv[i]` is row `i`'s inverse length. Only rows for which
/// `keep(i)` is true are scored: a presence bit, a scope bitset, or both. A zero query has no direction, so it
/// matches nothing. At most 1,024 dims: an `i32` dot product is then below 2^24 and exact as an `f32`, so equal
/// vectors score equal everywhere (workshop 008 section 2.7).
pub fn top_k(
    query: &[i8],
    rows: &[i8],
    inv: &[f32],
    k: usize,
    keep: impl FnMut(usize) -> bool,
) -> Vec<Hit> {
    top_k_scored(query, rows, inv, k, keep, |_, _| {})
}

/// [`top_k`] that also hands every kept row's score to `scored(row, score)` from the same loop, so a caller can
/// measure the whole distribution (PRD 002 search-10) with no second pass and no new dot products. A zero query or
/// `k = 0` scores nothing.
pub fn top_k_scored(
    query: &[i8],
    rows: &[i8],
    inv: &[f32],
    k: usize,
    mut keep: impl FnMut(usize) -> bool,
    mut scored: impl FnMut(usize, f32),
) -> Vec<Hit> {
    let dims = query.len();
    debug_assert!(
        dims <= MAX_EXACT_DIMS,
        "{dims} dims: the f32 score is not exact"
    );
    let q = inverse_length(query);
    if k == 0 || q == 0.0 {
        return Vec::new();
    }
    debug_assert_eq!(rows.len(), inv.len() * dims);
    let mut heap: BinaryHeap<Worst> = BinaryHeap::with_capacity(k + 1);
    for (i, row) in rows.chunks_exact(dims).enumerate() {
        if !keep(i) {
            continue;
        }
        let hit = Hit {
            row: i as u32,
            score: dot(query, row) as f32 * q * inv[i],
        };
        scored(i, hit.score);
        if heap.len() < k {
            heap.push(Worst(hit));
        } else if heap.peek().is_some_and(|w| Worst(hit) < *w) {
            heap.pop();
            heap.push(Worst(hit));
        }
    }
    heap.into_sorted_vec().into_iter().map(|w| w.0).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn uncalibrated_keys_have_no_floor_and_the_config_wins_per_use() {
        let k = |t: &str| crate::ports::ModelKey::parse(t).unwrap();
        for use_ in [Use::Search, Use::Similar] {
            assert_eq!(floor(&k("fake-embed@8"), use_), Floor::NONE);
            assert_eq!(floor(&k("other@384"), use_), Floor::NONE);
        }
        let (none, set) = (FloorSource::None, FloorSource::Config);
        let other = k("other@384");
        assert_eq!(
            uses_without_floor(&other, none, none),
            Some("search and similar_to")
        );
        assert_eq!(uses_without_floor(&other, set, none), Some("similar_to"));
        assert_eq!(
            uses_without_floor(&other, none, FloorSource::Graph),
            Some("search")
        );
        assert_eq!(uses_without_floor(&other, set, set), None);
        assert_eq!(
            uses_without_floor(&k("fake-embed@8"), none, none),
            None,
            "test fakes are never reported"
        );
        let mut floors = Floors::default();
        floors.set(&k("other@384"), Some(0.3), None);
        // A second instance of the same key does not replace the first's floor, but may fill the one it left unset.
        floors.set(&k("other@384"), Some(0.9), Some(0.4));
        let search = floors.get(&k("other@384"), Use::Search);
        assert_eq!(
            (search.value, search.source, search.calibrated()),
            (0.3, FloorSource::Config, false)
        );
        assert_eq!(floors.get(&k("other@384"), Use::Similar).value, 0.4);
        assert_eq!(floors.get(&k("other@8"), Use::Search), Floor::NONE);
        for c in CALIBRATED {
            let mut key = crate::ports::ModelKey::new(c.model, c.dims).unwrap();
            if let Some(p) = c.prompt_version {
                key = key.with_prompt_version(p);
            }
            let f = Floors::default().get(&key, Use::Search);
            assert_eq!(
                (f.value, f.calibrated(), f.run),
                (c.search, true, Some(c.run))
            );
            assert_eq!(floor(&key, Use::Similar).value, c.similar);
            let similar = floor(&key, Use::Similar).source;
            assert_eq!(uses_without_floor(&key, f.source, similar), None);
        }
    }

    #[test]
    fn population_is_the_order_statistic_median_and_mad() {
        // Odd: sorted 0.1 0.2 0.3 0.5 0.9 → median 0.3; deviations 0.2 0.1 0 0.2 0.6 → sorted 0 0.1 0.2 0.2 0.6.
        let p = Population::of(&mut [0.5, 0.1, 0.9, 0.3, 0.2]).unwrap();
        assert_eq!((p.n, p.median), (5, 0.3));
        assert_eq!(p.mad, (0.1f32 - 0.3).abs().min((0.5f32 - 0.3).abs()));
        // Even: the lower of the middle two, an element, never an average.
        let p = Population::of(&mut [0.4, 0.1, 0.3, 0.2]).unwrap();
        assert_eq!(p.median, 0.2);
        // Duplicates: three equal scores give a MAD of 0, so no z.
        let p = Population::of(&mut [0.7, 0.7, 0.7, 0.1]).unwrap();
        assert_eq!((p.median, p.mad, p.z(0.9)), (0.7, 0.0, None));
        assert_eq!(Population::of(&mut []), None);
        let p = Population::of(&mut [0.0, 0.5, 1.0]).unwrap();
        assert_eq!(p.z(1.0), Some(1.0));
    }

    #[test]
    fn a_population_does_not_depend_on_the_order_of_its_scores() {
        // Every permutation of 6 scores (720, with ties and a negative) gives bit-identical statistics.
        let base = [0.31f32, -0.05, 0.77, 0.31, 0.12, 0.64];
        let want = Population::of(&mut base.clone()).unwrap();
        let mut idx = [0usize, 1, 2, 3, 4, 5];
        let mut c = [0usize; 6];
        let mut i = 0;
        let check = |idx: &[usize; 6]| {
            let mut v: Vec<f32> = idx.iter().map(|&j| base[j]).collect();
            let p = Population::of(&mut v).unwrap();
            assert_eq!(
                (p.median.to_bits(), p.mad.to_bits()),
                (want.median.to_bits(), want.mad.to_bits())
            );
        };
        check(&idx);
        // Heap's algorithm: every permutation once.
        while i < 6 {
            if c[i] < i {
                idx.swap(if i % 2 == 0 { 0 } else { c[i] }, i);
                check(&idx);
                c[i] += 1;
                i = 0;
            } else {
                c[i] = 0;
                i += 1;
            }
        }
    }

    #[test]
    fn labels_need_both_cuts_and_a_big_enough_spread_population() {
        let cuts = StrengthCuts {
            strong_z: 2.0,
            strong_z_per_ln: 0.0,
            strong_score: 0.6,
            fair_z: 1.0,
            fair_z_per_ln: 0.0,
            fair_score: 0.5,
            min_population: 5,
            z_part: true,
            max_n_measured: 100,
            run: "test",
        };
        // median 0.4, mad 0.1.
        let p = Population::of(&mut [0.3, 0.4, 0.5, 0.4, 0.2, 0.6, 0.4]).unwrap();
        assert_eq!(p.median, 0.4);
        assert!((p.mad - 0.1).abs() < 1e-6);
        assert_eq!(strength(&p, &cuts, 0.65), Some(Strength::Strong));
        assert_eq!(strength(&p, &cuts, 0.55), Some(Strength::Fair), "z 1.5");
        assert_eq!(strength(&p, &cuts, 0.45), Some(Strength::Weak), "z 0.5");
        let high = StrengthCuts {
            strong_score: 0.9,
            ..cuts
        };
        assert_eq!(
            strength(&p, &high, 0.65),
            Some(Strength::Fair),
            "a high z alone is not strong: the score cut holds it back"
        );
        let grows = StrengthCuts {
            strong_z_per_ln: 1.0,
            ..cuts
        };
        assert!(grows.strong_z_at(7) > cuts.strong_z_at(7));
        assert_eq!(
            strength(&p, &grows, 0.65),
            Some(Strength::Fair),
            "z 2.5 is below 2 + ln 7"
        );
        let small = StrengthCuts {
            min_population: 8,
            ..cuts
        };
        assert_eq!(strength(&p, &small, 0.65), None, "below min_population");
        let flat = Population::of(&mut [0.4; 9]).unwrap();
        assert_eq!(strength(&flat, &cuts, 0.9), None, "a MAD of 0");
        let cosine_only = StrengthCuts {
            z_part: false,
            ..grows
        };
        assert_eq!(
            strength(&p, &cosine_only, 0.65),
            Some(Strength::Strong),
            "without the z part only the cosine cuts apply"
        );
        assert_eq!(strength(&flat, &cosine_only, 0.55), Some(Strength::Fair));
        // The cosine cuts hold at equality (>=), strong and fair alike.
        assert_eq!(strength(&p, &cosine_only, 0.6), Some(Strength::Strong));
        assert_eq!(strength(&p, &cosine_only, 0.5), Some(Strength::Fair));
        assert_eq!(strength(&p, &cosine_only, 0.4999), Some(Strength::Weak));
        // Above twice the largest population measured nothing is labelled; at twice it, still labelled.
        let measured = StrengthCuts {
            max_n_measured: 3,
            ..cosine_only
        };
        assert!(labels(&Population { n: 6, ..p }, &measured));
        assert_eq!(
            strength(&Population { n: 6, ..p }, &measured, 0.65),
            Some(Strength::Strong)
        );
        assert_eq!(strength(&Population { n: 7, ..p }, &measured, 0.65), None);
        assert!(!labels(&Population { n: 7, ..p }, &measured));
        assert_eq!(
            strength(
                &p,
                &StrengthCuts {
                    min_population: 8,
                    ..cosine_only
                },
                0.65
            ),
            None
        );
        assert!(Strength::Exact > Strength::Strong && Strength::Strong > Strength::Fair);
    }

    #[test]
    fn a_cosine_is_one_with_itself_and_minus_one_with_its_opposite() {
        let a: Vec<i8> = (0..256).map(|i| ((i * 37) % 255 - 127) as i8).collect();
        let neg: Vec<i8> = a.iter().map(|x| -x).collect();
        let l = inverse_length(&a);
        assert!((dot(&a, &a) as f32 * l * l - 1.0).abs() < 1e-6);
        assert!((dot(&a, &neg) as f32 * l * inverse_length(&neg) + 1.0).abs() < 1e-6);
        assert_eq!(inverse_length(&[0, 0, 0]), 0.0);
        assert_eq!(dot(&[127; 256], &[-127; 256]), -256 * 127 * 127);
    }

    #[test]
    fn top_k_is_best_first_ties_by_row_and_skips_what_keep_refuses() {
        // 2 dims. Rows: same as query, orthogonal, same again, opposite, half-way.
        let rows: Vec<i8> = vec![10, 0, 0, 10, 20, 0, -10, 0, 10, 10];
        let inv: Vec<f32> = rows.chunks(2).map(inverse_length).collect();
        let hits = top_k(&[1, 0], &rows, &inv, 3, |_| true);
        let order: Vec<u32> = hits.iter().map(|h| h.row).collect();
        assert_eq!(
            order,
            [0, 2, 4],
            "rows 0 and 2 tie at 1.0; the lower row first"
        );
        assert!((hits[2].score - core::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        let kept = top_k(&[1, 0], &rows, &inv, 10, |i| i != 0 && i != 4);
        let order: Vec<u32> = kept.iter().map(|h| h.row).collect();
        assert_eq!(order, [2, 1, 3], "every kept row, fewer than k");
        assert!(top_k(&[1, 0], &rows, &inv, 0, |_| true).is_empty());
    }

    #[test]
    fn a_zero_vector_scores_zero_never_nan() {
        // Rows: zero, the query, zero. A zero query scores 0 against every row too.
        let rows: Vec<i8> = vec![0, 0, 3, 4, 0, 0];
        let inv: Vec<f32> = rows.chunks(2).map(inverse_length).collect();
        assert_eq!(inv[0], 0.0);
        let hits = top_k(&[3, 4], &rows, &inv, 3, |_| true);
        assert_eq!(hits.iter().map(|h| h.row).collect::<Vec<_>>(), [1, 0, 2]);
        assert!((hits[0].score - 1.0).abs() < 1e-6);
        assert_eq!(
            (hits[1].score, hits[2].score),
            (0.0, 0.0),
            "ties to the lower row"
        );
        assert!(
            top_k(&[0, 0], &rows, &inv, 3, |_| true).is_empty(),
            "a zero query matches nothing"
        );
        assert!((MAX_EXACT_DIMS as f64 * 127.0 * 127.0) < (1u64 << 24) as f64);
    }

    #[test]
    fn top_k_matches_a_full_sort() {
        let mut state = 7u64;
        let mut next = || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            (state >> 56) as u8 as i8
        };
        let dims = 16;
        let rows: Vec<i8> = (0..300 * dims).map(|_| next()).collect();
        let inv: Vec<f32> = rows.chunks(dims).map(inverse_length).collect();
        let query: Vec<i8> = (0..dims).map(|_| next()).collect();
        let q = inverse_length(&query);
        let mut all: Vec<Hit> = rows
            .chunks(dims)
            .enumerate()
            .filter(|(i, _)| i % 3 != 0)
            .map(|(i, r)| Hit {
                row: i as u32,
                score: dot(&query, r) as f32 * q * inv[i],
            })
            .collect();
        all.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.row.cmp(&b.row)));
        let mut every = Vec::new();
        let scored = top_k_scored(
            &query,
            &rows,
            &inv,
            25,
            |i| i % 3 != 0,
            |i, s| {
                every.push(Hit {
                    row: i as u32,
                    score: s,
                })
            },
        );
        every.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.row.cmp(&b.row)));
        assert_eq!(every, all, "every kept row scored once, from the same loop");
        all.truncate(25);
        assert_eq!(scored, all);
        assert_eq!(top_k(&query, &rows, &inv, 25, |i| i % 3 != 0), all);
    }
}

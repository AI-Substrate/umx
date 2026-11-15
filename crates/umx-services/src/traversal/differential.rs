//! Differential tests for the array search (workstream 003 s17). The oracle is the search `walk` and `path` ran
//! before it: breadth-first loops over [`ReadSpace::steps`] keyed by [`At`], kept here as they were on main. On
//! seeded random stores the new search must give the same output, order and ties included.

use super::*;
use crate::{NodeAddRequest, Ports};
use alloc::format;
use alloc::string::ToString;
use alloc::sync::Arc;
use alloc::vec;
use umx_core::codec::GraphFile;
use umx_core::ports::{GraphDir, GraphFiles};
use umx_fakes::{FakeClock, FakeGraphFiles, FakeIds, FakeLocalState};

// The oracle: main's walk and path, unchanged but for the names and the path search split out.

fn walk_oracle(store: &GraphStore, ctx: &Ctx, req: WalkRequest) -> Outcome<Page<WalkRow>> {
    let refs = [Some(req.node.as_str()), req.within.as_deref()];
    let (mut space, scope) = ReadSpace::new(store, ctx, &refs, "walk")?;
    for pass in 0..2 {
        let start = space.start(&req.node)?;
        let within = req.within.as_ref().map(|w| space.resolve(w)).transpose()?;
        if within.is_some_and(|root| space.state(root).is_some() || root.node == root.graph) {
            return Err(problems::usage_invalid(
                "--within needs a loaded hierarchy node",
                "walk",
            ));
        }
        let within_nodes = within.map(|root| {
            space
                .store
                .loaded(&space.aliases[root.part.expect("within loaded")])
                .expect("selected")
                .graph
                .within(root.node)
        });
        let mut seen = BTreeSet::from([start]);
        let mut queue = VecDeque::from([(start, 0usize)]);
        let mut ready = VecDeque::new();
        let mut missing =
            req.relation.as_deref() == Some("similar_to") && space.lacks_vector(start);
        let rows = core::iter::from_fn(|| {
            loop {
                if let Some(row) = ready.pop_front() {
                    return Some(row);
                }
                let (at, depth) = queue.pop_front()?;
                if depth == req.depth {
                    continue;
                }
                for hop in space.steps(at, req.direction, req.relation.as_deref()) {
                    if req.relation.as_deref() == Some("similar_to") {
                        missing |= space.lacks_vector(hop.to);
                    }
                    if !seen.insert(hop.to) {
                        continue;
                    }
                    if space.state(hop.to).is_none() {
                        queue.push_back((hop.to, depth + 1));
                    }
                    if within.is_some_and(|root| {
                        root.part != hop.to.part
                            || !within_nodes
                                .as_ref()
                                .expect("within set")
                                .contains(hop.to.node)
                    }) {
                        continue;
                    }
                    ready.push_back(WalkRow {
                        depth: depth + 1,
                        node: space.cell(hop.to),
                        from: at.node,
                        edge: hop.edge,
                        graph: space.graph_name(hop.to),
                        graph_id: hop.to.graph,
                        address: space.address(hop.to),
                        from_graph: at.graph,
                        edge_graph: space
                            .store
                            .loaded(&space.aliases[hop.owner])
                            .expect("owner")
                            .meta
                            .id,
                        state: space.state(hop.to),
                        candidates: space.candidates(hop.to),
                    });
                }
            }
        });
        let mut value = lazy_page(&space.graph_name(start), rows, req.limit, req.offset);
        let changed = space.finish();
        if changed.is_none() {
            let mut warnings = scope
                .saved
                .as_ref()
                .map(|s| store.saved_warnings(s, &space.aliases))
                .transpose()?
                .unwrap_or_default();
            warnings.extend(space.similar_warnings(start, req.relation.as_deref(), missing));
            let (names, notices) =
                placeholder_names(store, value.rows.iter().any(|r| unavailable(r.state)));
            for row in &mut value.rows {
                if unavailable(row.state)
                    && let Some(alias) = names.get(&row.graph_id)
                {
                    row.graph = alias.clone();
                    row.address = address(alias, row.node.id);
                }
            }
            warnings.extend(notices);
            warnings.extend(store.stale_notes(&space.aliases));
            return Ok(Done {
                value,
                notes: warnings,
            });
        }
        if pass == 1 {
            break;
        }
        space.exclude(&changed.expect("changed"));
    }
    Err(problems::usage_invalid(
        "graph policy changed during the read (retry)",
        "walk",
    ))
}

fn path_search_oracle(
    space: &ReadSpace<'_>,
    from: At,
    to: At,
    max_depth: usize,
    direction: Direction,
    relation: Option<&str>,
) -> Vec<(At, Option<EdgeCell>)> {
    let mut seen = BTreeSet::from([from]);
    let mut prev = BTreeMap::new();
    let mut queue = VecDeque::from([(from, 0usize)]);
    let mut found = from == to;
    while let Some((at, depth)) = queue.pop_front() {
        if found {
            break;
        }
        if depth == max_depth {
            continue;
        }
        for hop in space.steps(at, direction, relation) {
            if !seen.insert(hop.to) {
                continue;
            }
            let other = hop.to;
            prev.insert(other, (at, hop.edge));
            if other == to {
                found = true;
                break;
            }
            if space.state(other).is_none() {
                queue.push_back((other, depth + 1));
            }
        }
    }
    let mut rows = Vec::new();
    if found {
        let mut at = to;
        while let Some((before, edge)) = prev.remove(&at) {
            rows.push((at, Some(edge)));
            at = before;
        }
        rows.push((from, None));
        rows.reverse();
    }
    rows
}

fn path_oracle(store: &GraphStore, ctx: &Ctx, req: PathRequest) -> Outcome<Page<PathRow>> {
    let refs = [Some(req.from.as_str()), Some(req.to.as_str())];
    let (mut space, scope) = ReadSpace::new(store, ctx, &refs, "path")?;
    for pass in 0..2 {
        let from = space.start(&req.from)?;
        let to = space.resolve(&req.to)?;
        let rows = path_search_oracle(
            &space,
            from,
            to,
            req.max_depth,
            req.direction,
            req.relation.as_deref(),
        );
        let missing = req.relation.as_deref() == Some("similar_to")
            && (space.lacks_vector(from)
                || (to.part == from.part && space.lacks_vector(to))
                || rows.iter().any(|(at, _)| space.lacks_vector(*at)));
        let rows: Vec<_> = rows
            .into_iter()
            .enumerate()
            .map(|(step, (at, edge))| PathRow {
                step,
                node: space.cell(at),
                edge,
                graph: space.graph_name(at),
                graph_id: at.graph,
                address: space.address(at),
                state: space.state(at),
                candidates: space.candidates(at),
            })
            .collect();
        let count = rows.len();
        let mut value = page(&space.graph_name(from), rows, count, 0);
        let changed = space.finish();
        if changed.is_none() {
            let mut warnings = space.similar_warnings(from, req.relation.as_deref(), missing);
            if let Some(saved) = &scope.saved {
                warnings.extend(store.saved_warnings(saved, &space.aliases)?);
            }
            let (names, notices) =
                placeholder_names(store, value.rows.iter().any(|r| unavailable(r.state)));
            for row in &mut value.rows {
                if unavailable(row.state)
                    && let Some(alias) = names.get(&row.graph_id)
                {
                    row.graph = alias.clone();
                    row.address = address(alias, row.node.id);
                }
            }
            warnings.extend(notices);
            warnings.extend(store.stale_notes(&space.aliases));
            return Ok(Done {
                value,
                notes: warnings,
            });
        }
        if pass == 1 {
            break;
        }
        space.exclude(&changed.expect("changed"));
    }
    Err(problems::usage_invalid(
        "graph policy changed during the read (retry)",
        "path",
    ))
}

// Seeded stores.

/// splitmix64: small, seeded, the same on every machine.
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

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

/// Stored relations: directed, symmetric (alternative_to, conflicts_with) and the hierarchy.
const RELATIONS: [&str; 5] = [
    "supports",
    "blocks",
    "part_of",
    "conflicts_with",
    "alternative_to",
];

fn empty_store() -> (GraphStore, Arc<FakeGraphFiles>) {
    let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let store = GraphStore::new(Ports {
        files: files.clone(),
        vector_cache: files.clone(),
        vector_cache_root: GraphDir::parse("/work/cache").unwrap(),
        clock: Arc::new(FakeClock::default()),
        ids: Arc::new(FakeIds::new()),
        state: Arc::new(FakeLocalState::new()),
        allows: Arc::new(FakeLocalState::new()),
        lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
        scopes: Arc::new(FakeLocalState::new()),
        registry: Arc::new(FakeLocalState::new()),
        sources: Arc::new(umx_fakes::FakeSourceReader::open(
            GraphDir::parse("/").unwrap(),
        )),
    });
    (store, files)
}

fn new_graph(store: &mut GraphStore, alias: &str, secured: bool) -> Id {
    store
        .graph_new(
            &Ctx::new("test"),
            &format!("/work/{alias}"),
            alias,
            None,
            false,
            secured.then(|| umx_core::GraphEmbedding::new(None, umx_core::Policy::SECURED)),
        )
        .unwrap()
        .value
        .graph
        .id
}

fn add_nodes(store: &mut GraphStore, alias: &str, count: usize) -> Vec<Id> {
    (0..count)
        .map(|i| {
            store
                .node_add(
                    &Ctx::new("test").graph(alias),
                    NodeAddRequest {
                        kind: "problem".into(),
                        title: format!("{alias} synthetic {i}"),
                        ..Default::default()
                    },
                )
                .unwrap()
                .value
                .node
                .id
        })
        .collect()
}

/// Adds an edge as `author`; refusals (duplicates, policy) are part of the draw. True when it was stored.
fn link(store: &mut GraphStore, author: &str, from: &str, relation: &str, to: &str) -> bool {
    store
        .edge_add(&Ctx::new("test").graph(author), from, relation, to, &[])
        .is_ok()
}

/// Random local edges in `alias`: about `per_node` per node, with self-loops and parallel edges of other kinds.
fn local_edges(store: &mut GraphStore, rng: &mut Rng, alias: &str, nodes: &[Id], per_node: usize) {
    for _ in 0..nodes.len() * per_node {
        let from = *rng.pick(nodes);
        let to = if rng.chance(8) {
            from
        } else {
            *rng.pick(nodes)
        };
        let relation = *rng.pick(&RELATIONS);
        link(store, alias, &from.to_text(), relation, &to.to_text());
    }
}

struct World {
    store: GraphStore,
    /// Node references the requests draw from: plain ids, addresses, graph nodes, provenance nodes and misses.
    refs: Vec<String>,
    /// External edges stored, and of them links a third graph (g2) stores between g1 and g0.
    external: usize,
    third: usize,
}

/// Open graphs g0, g1 and g2 (g2 also stores third-graph links from g1 into g0), secured g3, gb (reloaded after
/// links to nodes it lacks were made, so those ends are edge_broken), gone (unloaded, so its ends are
/// graph_not_loaded) and sometimes c1, a second copy of g1.
fn world(seed: u64) -> World {
    let mut rng = Rng(seed);
    let (mut store, files) = empty_store();
    let mut nodes: Vec<(&str, Id, Vec<Id>)> = Vec::new();
    for (alias, secured) in [
        ("g0", false),
        ("g1", false),
        ("g2", false),
        ("g3", true),
        ("gb", false),
        ("gone", false),
    ] {
        let id = new_graph(&mut store, alias, secured);
        let count = 3 + rng.below(6);
        let ids = add_nodes(&mut store, alias, count);
        nodes.push((alias, id, ids));
    }
    for (alias, _, ids) in &nodes {
        local_edges(&mut store, &mut rng, alias, ids, 2);
    }
    let (mut external, mut third) = (0, 0);
    for _ in 0..12 {
        let (author, _, from) = &nodes[rng.below(4)];
        let (far, _, to) = &nodes[rng.below(nodes.len())];
        let from = rng.pick(from).to_text();
        let to = if rng.chance(15) {
            format!("umx:{far}")
        } else {
            format!("umx:{far}/{}", rng.pick(to))
        };
        let relation = *rng.pick(&RELATIONS);
        external += usize::from(link(&mut store, author, &from, relation, &to));
    }
    // Third-graph links: stored in g2, both ends elsewhere.
    for _ in 0..4 {
        let from = format!("umx:g1/{}", rng.pick(&nodes[1].2));
        let to = format!("umx:g0/{}", rng.pick(&nodes[0].2));
        let relation = *rng.pick(&RELATIONS);
        third += usize::from(link(&mut store, "g2", &from, relation, &to));
    }
    let copy = |store: &mut GraphStore, of: &str, alias: &str| {
        let target = GraphDir::parse(&format!("/work/{alias}")).unwrap();
        let source = GraphDir::parse(&format!("/work/{of}")).unwrap();
        for file in GraphFile::ALL {
            if let Some(bytes) = files.read(&source, file).unwrap() {
                files.tamper_for_tests(&target, file, &bytes);
            }
        }
        store
            .graph_load(&Ctx::new("test"), &format!("/work/{alias}"), Some(alias))
            .unwrap();
    };
    // c2 and c3 copy `gone` while it is the primary; once it unloads, its ends are copy_ambiguous.
    if rng.chance(40) {
        copy(&mut store, "gone", "c2");
        copy(&mut store, "gone", "c3");
    }
    store.graph_unload(&Ctx::new("test"), "gone").unwrap();
    let gb = nodes[4].1;
    store.graph_unload(&Ctx::new("test"), "gb").unwrap();
    for _ in 0..3 {
        let from = rng.pick(&nodes[0].2).to_text();
        let missing = Id(u128::from(rng.next()) << 20);
        external += usize::from(link(
            &mut store,
            "g0",
            &from,
            "supports",
            &format!("umx:{gb}/{missing}"),
        ));
    }
    store
        .graph_load(&Ctx::new("test"), "/work/gb", None)
        .unwrap();
    // c1 copies g1, whose primary stays loaded.
    if rng.chance(40) {
        copy(&mut store, "g1", "c1");
    }
    let mut refs = vec!["umx:g1".to_string(), "umx:g0".to_string(), Id(7).to_text()];
    for alias in store.aliases().map(ToString::to_string).collect::<Vec<_>>() {
        for node in store.loaded(&alias).unwrap().graph.nodes() {
            refs.push(node.id().to_text());
            refs.push(format!("umx:{alias}/{}", node.id()));
        }
    }
    World {
        store,
        refs,
        external,
        third,
    }
}

fn contexts() -> [Ctx; 7] {
    let c = || Ctx::new("reader");
    [
        c(),
        c().with_provenance(true),
        c().graph("g0"),
        c().graph("g0,g3").with_provenance(true),
        c().graph("g1,g2"),
        c().graph("g1,c1").with_provenance(true),
        c().graph("g3"),
    ]
}

fn direction(rng: &mut Rng) -> Direction {
    *rng.pick(&[Direction::Out, Direction::In, Direction::Both])
}

fn relation(rng: &mut Rng) -> Option<String> {
    let all = [
        None,
        None,
        Some("supports"),
        Some("conflicts_with"),
        Some("part_of"),
        Some("authored_by"),
        Some("edited_by"),
    ];
    rng.pick(&all).map(Into::into)
}

/// Every walk and path on 40 seeded stores, in every context, equals the oracle's, warnings and errors included.
#[test]
fn walk_and_path_match_the_oracle_on_mixed_stores() {
    let (mut walked, mut found, mut long, mut external, mut third) = (0, 0, 0, 0, 0);
    let (mut states, mut kinds) = (BTreeMap::new(), BTreeMap::new());
    for seed in 0..40 {
        let w = world(seed);
        external += w.external;
        third += w.third;
        let mut rng = Rng(seed ^ 0x5eed);
        for (c, ctx) in contexts().iter().enumerate() {
            for _ in 0..60 {
                let req = WalkRequest {
                    node: rng.pick(&w.refs).clone(),
                    depth: 1 + rng.below(4),
                    relation: relation(&mut rng),
                    direction: direction(&mut rng),
                    within: rng.chance(10).then(|| rng.pick(&w.refs).clone()),
                    limit: if rng.chance(30) {
                        rng.below(4)
                    } else {
                        usize::MAX
                    },
                    offset: if rng.chance(20) { rng.below(3) } else { 0 },
                };
                let new = walk(&w.store, ctx, req.clone());
                assert_eq!(
                    new,
                    walk_oracle(&w.store, ctx, req.clone()),
                    "seed {seed} context {c} {req:?}"
                );
                if let Ok(done) = &new {
                    walked += done.value.rows.len();
                    for row in &done.value.rows {
                        *states.entry(row.state).or_insert(0) += 1;
                        *kinds.entry(row.node.kind.clone()).or_insert(0) += 1;
                    }
                }
                let req = PathRequest {
                    from: rng.pick(&w.refs).clone(),
                    to: rng.pick(&w.refs).clone(),
                    max_depth: 1 + rng.below(5),
                    relation: relation(&mut rng),
                    direction: direction(&mut rng),
                };
                let new = path(&w.store, ctx, req.clone());
                assert_eq!(
                    new,
                    path_oracle(&w.store, ctx, req.clone()),
                    "seed {seed} context {c} {req:?}"
                );
                if let Ok(done) = &new {
                    found += usize::from(!done.value.rows.is_empty());
                    long += usize::from(done.value.rows.len() > 3);
                    for row in &done.value.rows {
                        *states.entry(row.state).or_insert(0) += 1;
                    }
                }
            }
        }
    }
    // The draw reaches what it is meant to: rows, found and long paths, external edges, unavailable ends.
    let state = |s| states.get(&Some(s)).copied().unwrap_or(0);
    let kind = |k: &str| kinds.get(k).copied().unwrap_or(0);
    assert!(
        walked > 4_000 && found > 200 && long > 40,
        "{walked} {found} {long}"
    );
    assert!(external > 300 && third > 80, "{external} {third}");
    assert!(
        state("copy_ambiguous") > 5
            && state("edge_broken") > 100
            && state("graph_not_loaded") > 400,
        "{states:?}"
    );
    assert!(kind("session") > 200 && kind("graph") > 30, "{kinds:?}");
}

/// A small dense graph: cycles, symmetric relations, self-loops and parallel edges; nodes, then provenance nodes.
fn small(seed: u64) -> (GraphStore, Vec<Id>) {
    let mut rng = Rng(seed);
    let (mut store, _) = empty_store();
    new_graph(&mut store, "g", false);
    let ids = add_nodes(&mut store, "g", 4 + rng.below(6));
    let per_node = 1 + rng.below(3);
    local_edges(&mut store, &mut rng, "g", &ids, per_node);
    let all = store
        .loaded("g")
        .unwrap()
        .graph
        .nodes()
        .map(|n| n.id())
        .collect();
    (store, all)
}

/// Every pair of 2,000 small graphs: the array search (two-sided where it can be) returns the oracle's path, with
/// provenance hidden and shown.
#[test]
fn two_sided_paths_match_the_oracle_on_every_pair() {
    let (mut found, mut two_steps) = (0, 0);
    for seed in 0..2_000 {
        let (store, ids) = small(seed);
        let mut rng = Rng(seed ^ 0x7a1);
        let ctx = Ctx::new("reader")
            .graph("g")
            .with_provenance(rng.chance(50));
        let (space, _) = ReadSpace::new(&store, &ctx, &[], "path").unwrap();
        let (dir, rel) = (direction(&mut rng), relation(&mut rng));
        let max_depth = 1 + rng.below(6);
        for &a in &ids {
            for &b in &ids {
                let (from, to) = (space.at(0, a), space.at(0, b));
                let old = path_search_oracle(&space, from, to, max_depth, dir, rel.as_deref());
                let mut handles = bfs::Handles::new(&space);
                let (f, t) = (handles.node(from), handles.node(to));
                let new: Vec<_> = handles
                    .path(f, t, max_depth, dir, rel.as_deref())
                    .into_iter()
                    .map(|(n, s)| (handles.at(n), s.map(|s| s.cell())))
                    .collect();
                assert_eq!(
                    new, old,
                    "seed {seed} {a} -> {b} {dir:?} {rel:?} {max_depth}"
                );
                found += usize::from(!old.is_empty());
                two_steps += usize::from(old.len() > 2);
            }
        }
    }
    assert!(found > 30_000 && two_steps > 8_000, "{found} {two_steps}");
}

/// Every pair of loaded nodes and graph nodes across the parts of 100 mixed stores (external edges, third-graph
/// links, copies, a secured graph, unloaded and broken ends), in every context: the array search, two-sided across
/// graphs, returns the oracle's path.
#[test]
fn cross_graph_paths_match_the_oracle_on_every_pair() {
    let (mut found, mut across) = (0, 0);
    for seed in 0..100 {
        let w = world(seed);
        let mut rng = Rng(seed ^ 0xc405);
        for (c, ctx) in contexts().iter().enumerate() {
            // A context naming a copy this store lacks is refused before any search.
            let Ok((space, _)) = ReadSpace::new(&w.store, ctx, &[], "path") else {
                continue;
            };
            let mut ends = Vec::new();
            for (part, alias) in space.aliases.iter().enumerate() {
                let loaded = w.store.loaded(alias).unwrap();
                ends.push(space.at(part, loaded.meta.id));
                ends.extend(loaded.graph.nodes().map(|n| space.at(part, n.id())));
            }
            let (dir, rel) = (direction(&mut rng), relation(&mut rng));
            let max_depth = 1 + rng.below(6);
            for &from in &ends {
                for &to in &ends {
                    let old = path_search_oracle(&space, from, to, max_depth, dir, rel.as_deref());
                    let mut handles = bfs::Handles::new(&space);
                    let (f, t) = (handles.node(from), handles.node(to));
                    let new: Vec<_> = handles
                        .path(f, t, max_depth, dir, rel.as_deref())
                        .into_iter()
                        .map(|(n, s)| (handles.at(n), s.map(|s| s.cell())))
                        .collect();
                    assert_eq!(
                        new, old,
                        "seed {seed} context {c} {from:?} -> {to:?} {dir:?} {rel:?}"
                    );
                    found += usize::from(old.len() > 1);
                    across += usize::from(old.iter().any(|(at, _)| at.part != from.part));
                }
            }
        }
    }
    assert!(found > 10_000 && across > 4_000, "{found} {across}");
}

/// A chain longer than any small counter: the two-sided search finds its 299-step path at max depth 299 and none
/// at 298, as the oracle does.
#[test]
fn a_path_longer_than_255_steps_matches_the_oracle() {
    let (mut store, _) = empty_store();
    new_graph(&mut store, "g", false);
    let ids = add_nodes(&mut store, "g", 300);
    for pair in ids.windows(2) {
        assert!(link(
            &mut store,
            "g",
            &pair[0].to_text(),
            "supports",
            &pair[1].to_text()
        ));
    }
    let ctx = Ctx::new("reader").graph("g");
    for max_depth in [298, 299, 300] {
        let req = PathRequest {
            from: ids[0].to_text(),
            to: ids[299].to_text(),
            max_depth,
            relation: None,
            direction: Direction::Both,
        };
        let new = path(&store, &ctx, req.clone());
        assert_eq!(new, path_oracle(&store, &ctx, req), "{max_depth}");
        let rows = new.unwrap().value.rows.len();
        assert_eq!(rows, if max_depth >= 299 { 300 } else { 0 }, "{max_depth}");
    }
}

/// Two nodes joined only through their session (both authored_by it): with provenance shown the path runs through
/// the session node, hidden there is none; walks agree too. Both match the oracle, in every direction.
#[test]
fn a_provenance_node_on_the_only_path_matches_the_oracle_shown_and_hidden() {
    let (mut store, _) = empty_store();
    new_graph(&mut store, "g", false);
    let ids = add_nodes(&mut store, "g", 2);
    for shown in [false, true] {
        let ctx = Ctx::new("reader").graph("g").with_provenance(shown);
        for dir in [Direction::Out, Direction::In, Direction::Both] {
            for relation in [None, Some("authored_by".into())] {
                let req = PathRequest {
                    from: ids[0].to_text(),
                    to: ids[1].to_text(),
                    max_depth: 3,
                    relation: relation.clone(),
                    direction: dir,
                };
                let new = path(&store, &ctx, req.clone());
                assert_eq!(new, path_oracle(&store, &ctx, req), "{shown} {dir:?}");
                let rows = new.unwrap().value.rows;
                let through = rows.get(1).map(|r| r.node.kind.as_str());
                let expected = (shown && dir == Direction::Both).then_some("session");
                assert_eq!(through, expected, "{shown} {dir:?} {relation:?}");
                let req = WalkRequest {
                    node: ids[0].to_text(),
                    depth: 2,
                    relation,
                    direction: dir,
                    within: None,
                    limit: usize::MAX,
                    offset: 0,
                };
                assert_eq!(
                    walk(&store, &ctx, req.clone()),
                    walk_oracle(&store, &ctx, req),
                    "{shown} {dir:?}"
                );
            }
        }
    }
}

/// The only link between g0 and g1 is stored in a third graph, g2. With g2 selected, path and walk follow it; with
/// g2 left out, neither does. Both match the oracle. Neither g0 nor g1 stores an external edge, so this guards the
/// rule that lets a read skip external steps: it must count every selected graph that stores one.
#[test]
fn a_link_stored_in_a_third_graph_is_followed_only_when_that_graph_is_selected() {
    let (mut store, _) = empty_store();
    let mut ids = Vec::new();
    for alias in ["g0", "g1", "g2"] {
        new_graph(&mut store, alias, false);
        ids.push(add_nodes(&mut store, alias, 2));
    }
    let (a, b) = (ids[0][0], ids[1][0]);
    assert!(link(
        &mut store,
        "g2",
        &format!("umx:g0/{a}"),
        "supports",
        &format!("umx:g1/{b}")
    ));
    for (graphs, linked) in [
        (None, true),
        (Some("g0,g1,g2"), true),
        (Some("g0,g1"), false),
    ] {
        let ctx = match graphs {
            Some(g) => Ctx::new("reader").graph(g),
            None => Ctx::new("reader"),
        };
        let req = PathRequest {
            from: format!("umx:g0/{a}"),
            to: format!("umx:g1/{b}"),
            max_depth: 3,
            relation: None,
            direction: Direction::Out,
        };
        let new = path(&store, &ctx, req.clone());
        assert_eq!(new, path_oracle(&store, &ctx, req), "{graphs:?}");
        assert_eq!(
            new.unwrap().value.rows.len(),
            if linked { 2 } else { 0 },
            "{graphs:?}"
        );
        let req = WalkRequest {
            node: format!("umx:g0/{a}"),
            depth: 2,
            relation: None,
            direction: Direction::Both,
            within: None,
            limit: usize::MAX,
            offset: 0,
        };
        let new = walk(&store, &ctx, req.clone());
        assert_eq!(new, walk_oracle(&store, &ctx, req), "{graphs:?}");
        let reached = new.unwrap().value.rows.iter().any(|r| r.node.id == b);
        assert_eq!(reached, linked, "{graphs:?}");
    }
}

/// The invariant the external-step precheck rests on: a loaded graph's edges never name its own graph id. Loading a
/// file whose edges name it (the own-graph-end fixture) and adding an edge to an address in the graph itself both
/// leave every end unqualified, so the graph stores no external edge.
#[test]
fn a_graphs_own_edges_never_name_its_own_id() {
    let (mut store, files) = empty_store();
    let dir = GraphDir::parse("/work/own").unwrap();
    for (file, bytes) in [
        (
            GraphFile::Graph,
            include_bytes!("../../tests/fixtures/own-graph-end/graph.json").as_slice(),
        ),
        (
            GraphFile::Nodes,
            include_bytes!("../../tests/fixtures/own-graph-end/nodes.jsonl").as_slice(),
        ),
        (
            GraphFile::Edges,
            include_bytes!("../../tests/fixtures/own-graph-end/edges.jsonl").as_slice(),
        ),
    ] {
        files.tamper_for_tests(&dir, file, bytes);
    }
    let alias = store
        .graph_load(&Ctx::new("test"), "/work/own", None)
        .unwrap()
        .value
        .graph
        .alias;
    let own = store.loaded(&alias).unwrap().meta.id;
    let nodes: Vec<Id> = store
        .loaded(&alias)
        .unwrap()
        .graph
        .nodes()
        .map(|n| n.id())
        .collect();
    assert!(link(
        &mut store,
        &alias,
        &nodes[0].to_text(),
        "supports",
        &format!("umx:{own}/{}", nodes[1])
    ));
    let graph = &store.loaded(&alias).unwrap().graph;
    assert!(graph.edges().count() >= 2);
    for edge in graph.edges() {
        assert_eq!(
            (edge.from_graph(), edge.to_graph()),
            (None, None),
            "{}",
            edge.id()
        );
    }
    assert!(!store.has_external_edges(&alias));
}

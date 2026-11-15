use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use umx_core::Id;
use umx_graph::{EdgeRef, Graph};

type SourceEdges = Box<[(u64, Id)]>;

/// External ends across loaded source copies. Target ids survive node renumbering; the source instance
/// distinguishes copies and is retired on unload. Both-external edges are indexed at both ends. `sources` counts
/// each source's entries, so a remove knows when a source is empty without scanning the index.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IncomingIndex {
    entries: BTreeMap<(Id, Id), SourceEdges>,
    sources: BTreeMap<u64, usize>,
}

impl IncomingIndex {
    pub(crate) fn has_source(&self, source: u64) -> bool {
        self.sources.contains_key(&source)
    }

    pub fn entries(&self, graph: Id, node: Id) -> &[(u64, Id)] {
        self.entries
            .get(&(graph, node))
            .map_or(&[], |entries| &**entries)
    }

    pub fn len(&self) -> usize {
        self.entries.values().map(|entries| entries.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn add_edge(&mut self, source: u64, edge: EdgeRef<'_>) {
        let ends = [
            (edge.from_graph(), edge.from()),
            (edge.to_graph(), edge.to()),
        ];
        for (i, (graph, node)) in ends.into_iter().enumerate() {
            let Some(graph) = graph else { continue };
            if i == 1 && ends[0] == (Some(graph), node) {
                continue;
            }
            let entries = self.entries.entry((graph, node)).or_default();
            let value = (source, edge.id());
            if let Err(at) = entries.binary_search(&value) {
                let mut values = core::mem::take(entries).into_vec();
                values.insert(at, value);
                *entries = values.into_boxed_slice();
                *self.sources.entry(source).or_default() += 1;
            }
        }
    }

    /// The mirror of `add_edge` for one removed edge record (ws008 E2 `entity unlink`, F122 edit and delete), after
    /// its graph's write committed: the same keys, a binary-search remove, and the source's count down by one per
    /// entry taken. A source left with no entries is dropped, so the index equals one rebuilt from the graphs.
    pub(crate) fn remove_edge(&mut self, source: u64, edge: &umx_core::Edge) {
        let ends = [(edge.from_graph, edge.from), (edge.to_graph, edge.to)];
        for (i, (graph, node)) in ends.into_iter().enumerate() {
            let Some(graph) = graph else { continue };
            if i == 1 && ends[0] == (Some(graph), node) {
                continue;
            }
            let Some(entries) = self.entries.get_mut(&(graph, node)) else {
                continue;
            };
            let Ok(at) = entries.binary_search(&(source, edge.id)) else {
                continue;
            };
            let mut values = core::mem::take(entries).into_vec();
            values.remove(at);
            if values.is_empty() {
                self.entries.remove(&(graph, node));
            } else {
                *entries = values.into_boxed_slice();
            }
            if let Some(count) = self.sources.get_mut(&source) {
                *count -= 1;
                if *count == 0 {
                    self.sources.remove(&source);
                }
            }
        }
    }

    pub fn add_graph(&mut self, source: u64, graph: &Graph) {
        let mut grouped: BTreeMap<(Id, Id), Vec<(u64, Id)>> = BTreeMap::new();
        for edge in graph.edges() {
            let ends = [
                (edge.from_graph(), edge.from()),
                (edge.to_graph(), edge.to()),
            ];
            for (i, (graph, node)) in ends.into_iter().enumerate() {
                if i == 1 && ends[0] == (graph, node) {
                    continue;
                }
                if let Some(graph) = graph {
                    grouped
                        .entry((graph, node))
                        .or_default()
                        .push((source, edge.id()));
                }
            }
        }
        let mut added = 0;
        for (key, mut values) in grouped {
            let mut before = 0;
            if let Some(old) = self.entries.remove(&key) {
                before = old.len();
                values.extend_from_slice(&old);
            }
            values.sort_unstable();
            values.dedup();
            added += values.len() - before;
            self.entries.insert(key, values.into_boxed_slice());
        }
        if added > 0 {
            *self.sources.entry(source).or_default() += added;
        }
        // Bulk packing avoids mostly half-full B-tree nodes after an ordered graph load.
        self.entries = core::mem::take(&mut self.entries).into_iter().collect();
    }

    pub fn remove_source(&mut self, source: u64) {
        self.sources.remove(&source);
        self.entries.retain(|_, entries| {
            if entries.iter().any(|(s, _)| *s == source) {
                let values: Vec<_> = entries
                    .iter()
                    .copied()
                    .filter(|(s, _)| *s != source)
                    .collect();
                *entries = values.into_boxed_slice();
            }
            !entries.is_empty()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BatchLine, BatchOp, BatchRequest, Ctx, GraphStore, NodeAddRequest, Ports};
    use alloc::sync::Arc;
    use alloc::vec;
    use umx_core::codec::GraphFile;
    use umx_core::ports::{GraphDir, GraphFiles};
    use umx_core::setup::GraphFilesSetup;
    use umx_fakes::{FakeClock, FakeGraphFiles, FakeIds, FakeLocalState};

    fn setup() -> (GraphStore, Arc<FakeGraphFiles>, Id, Id) {
        setup_with(false)
    }

    /// Graphs `author` and `target` (secured when `secured`), one node in each: (store, files, author node, target node).
    fn setup_with(secured: bool) -> (GraphStore, Arc<FakeGraphFiles>, Id, Id) {
        let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
        let mut store = GraphStore::new(Ports {
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
        let mut ids = Vec::new();
        for alias in ["author", "target"] {
            store
                .graph_new(
                    &Ctx::new("test"),
                    &alloc::format!("/work/{alias}"),
                    alias,
                    None,
                    false,
                    (secured && alias == "target")
                        .then(|| umx_core::GraphEmbedding::new(None, umx_core::Policy::SECURED)),
                )
                .unwrap();
            ids.push(
                store
                    .node_add(
                        &Ctx::new("test").graph(alias),
                        NodeAddRequest {
                            kind: "problem".into(),
                            title: alias.into(),
                            ..Default::default()
                        },
                    )
                    .unwrap()
                    .value
                    .node
                    .id,
            );
        }
        (store, files, ids[0], ids[1])
    }

    fn external(store: &mut GraphStore, from: Id, to: Id) -> crate::Outcome<crate::EdgeAdded> {
        store.edge_add(
            &Ctx::new("test").graph("author"),
            &from.to_text(),
            "blocks",
            &alloc::format!("umx:target/{to}"),
            &[],
        )
    }

    fn rebuilt(store: &GraphStore) -> IncomingIndex {
        let mut index = IncomingIndex::default();
        for loaded in store.graphs.values() {
            index.add_graph(loaded.instance, &loaded.graph);
        }
        index
    }

    fn author_ctx() -> Ctx {
        Ctx::new("test").graph("author")
    }

    fn edit(relation: Option<&str>, reverse: bool) -> crate::EdgeEditRequest {
        crate::EdgeEditRequest {
            relation: relation.map(Into::into),
            reverse,
            ..Default::default()
        }
    }

    /// graph.json and edges.jsonl of `alias`, as on disk.
    fn files_of(files: &FakeGraphFiles, alias: &str) -> [Option<Vec<u8>>; 2] {
        let dir = GraphDir::parse(&alloc::format!("/work/{alias}")).unwrap();
        [GraphFile::Graph, GraphFile::Edges].map(|file| files.read(&dir, file).unwrap())
    }

    /// F122: the storing graph deletes an external edge; the far end's entry goes once the write commits, the index
    /// equals a rebuild, and the edge is gone from the author's file and stays gone after a reload.
    #[test]
    fn an_external_edge_is_deleted_and_the_index_follows() {
        let (mut store, _files, from, to) = setup();
        let edge = external(&mut store, from, to).unwrap().value.edge.id;
        let target = store.loaded("target").unwrap().meta.id;
        assert_eq!(store.incoming.entries(target, to).len(), 1);
        let done = store.edge_delete(&author_ctx(), &edge.to_text()).unwrap();
        assert_eq!(done.value.edge.to_graph, Some(target));
        assert!(store.incoming.entries(target, to).is_empty());
        assert_eq!(store.incoming, rebuilt(&store));
        assert!(
            !store
                .incoming
                .has_source(store.loaded("author").unwrap().instance)
        );
        store.graph_unload(&Ctx::new("test"), "author").unwrap();
        store
            .graph_load(&Ctx::new("test"), "/work/author", None)
            .unwrap();
        assert!(store.loaded("author").unwrap().graph.edge(edge).is_none());
        assert_eq!(store.incoming, rebuilt(&store));
    }

    /// F122: a relation edit on an external edge keeps its id and both stored ends byte for byte, stamps it, and the
    /// index equals a rebuild after it (old entries out, new in); with the far graph loaded, unloaded, and loaded twice.
    #[test]
    fn an_external_edge_is_retyped_with_its_ends_kept() {
        for far in ["loaded", "unloaded", "copies"] {
            let (mut store, files, from, to) = setup();
            let added = external(&mut store, from, to).unwrap().value.edge;
            match far {
                "unloaded" => {
                    store.graph_unload(&Ctx::new("test"), "target").unwrap();
                }
                "copies" => {
                    let copy = GraphDir::parse("/work/target2").unwrap();
                    for (file, bytes) in [GraphFile::Graph, GraphFile::Nodes, GraphFile::Edges]
                        .into_iter()
                        .zip(files_of_all(&files, "target"))
                    {
                        files
                            .write(&copy, file, &bytes.unwrap_or_default())
                            .unwrap();
                    }
                    store
                        .graph_load(&Ctx::new("test"), "/work/target2", Some("target2"))
                        .unwrap();
                }
                _ => {}
            }
            let done = store
                .edge_edit(
                    &author_ctx(),
                    &added.id.to_text(),
                    &edit(Some("supports"), false),
                )
                .unwrap();
            let e = &done.value.edge;
            assert_eq!(done.value.changed, ["type"], "{far}");
            assert_eq!(
                (
                    e.id,
                    e.from_graph,
                    e.from,
                    e.to_graph,
                    e.to,
                    e.kind.as_str()
                ),
                (
                    added.id,
                    added.from_graph,
                    added.from,
                    added.to_graph,
                    added.to,
                    "supports"
                ),
                "{far}"
            );
            assert!(
                e.fields.contains_key("updated") && e.fields.contains_key("updated_by"),
                "{far}"
            );
            assert!(
                e.edited_by_session.is_some(),
                "{far}: P2's typed session stamp"
            );
            assert_eq!(store.incoming, rebuilt(&store), "{far}");
            let stored = &store.loaded("author").unwrap().graph;
            assert_eq!(stored.edge(added.id).unwrap().kind(), "supports", "{far}");
        }
    }

    fn files_of_all(files: &FakeGraphFiles, alias: &str) -> [Option<Vec<u8>>; 3] {
        let dir = GraphDir::parse(&alloc::format!("/work/{alias}")).unwrap();
        [GraphFile::Graph, GraphFile::Nodes, GraphFile::Edges]
            .map(|file| files.read(&dir, file).unwrap())
    }

    /// F122 (graph-storage-expert): a failed write of an external edit or delete changes nothing: the index, the edge
    /// in memory, and graph.json and edges.jsonl byte for byte. graph.json failing proves it is in the write set.
    #[test]
    fn a_failed_external_edit_or_delete_changes_nothing() {
        for op in ["delete", "edit"] {
            let (mut store, files, from, to) = setup();
            let added = external(&mut store, from, to).unwrap().value.edge;
            let (index, disk) = (store.incoming.clone(), files_of(&files, "author"));
            files.fail_writes_to(Some(GraphFile::Graph));
            let failed = if op == "delete" {
                store
                    .edge_delete(&author_ctx(), &added.id.to_text())
                    .map(|_| ())
            } else {
                store
                    .edge_edit(
                        &author_ctx(),
                        &added.id.to_text(),
                        &edit(Some("supports"), false),
                    )
                    .map(|_| ())
            };
            assert!(failed.is_err(), "{op}: graph.json is in the write set");
            assert_eq!(store.incoming, index, "{op}");
            assert_eq!(files_of(&files, "author"), disk, "{op}");
            let kept = store
                .loaded("author")
                .unwrap()
                .graph
                .edge(added.id)
                .unwrap()
                .to_edge();
            assert_eq!(kept.kind, added.kind, "{op}");
        }
    }

    /// F122 for S5b (review R210-M1, graph-storage-expert's rule): an evidence append on an existing external edge puts
    /// graph.json in the write set, by a batch as by the verb, on a graph already at the external-ends format, with the
    /// writer's session already recorded. A failed graph.json write refuses and changes nothing: index, edge and both
    /// files.
    #[test]
    fn an_evidence_append_on_an_external_edge_writes_graph_json_by_batch_and_verb() {
        let append = |from: Id, to: Id, pointer: &str| BatchRequest {
            lines: vec![BatchLine {
                line: 1,
                op: BatchOp::EdgeAdd {
                    id: None,
                    from: Some(from.to_text()),
                    from_ref: None,
                    relation: "blocks".into(),
                    to: Some(alloc::format!("umx:target/{to}")),
                    to_ref: None,
                    sets: Vec::new(),
                    evidence: crate::EdgeEvidence {
                        pointers: vec![pointer.into()],
                        kind: None,
                    },
                },
            }],
            ..Default::default()
        };
        for by in ["batch", "verb"] {
            let (mut store, files, from, to) = setup();
            let added = external(&mut store, from, to).unwrap().value.edge;
            // The same writer again first: the format is current and the session exists, so only the line itself
            // could put graph.json in the set.
            store
                .batch(&author_ctx(), append(from, to, "a.md#L1"))
                .unwrap();
            assert_eq!(store.loaded("author").unwrap().meta.umx, 3);
            let (index, disk) = (store.incoming.clone(), files_of_all(&files, "author"));
            files.fail_writes_to(Some(GraphFile::Graph));
            let failed = if by == "batch" {
                store
                    .batch(&author_ctx(), append(from, to, "b.md#L2"))
                    .map(|_| ())
                    .map_err(|_| ())
            } else {
                store
                    .edge_add_with(
                        &author_ctx(),
                        &from.to_text(),
                        "blocks",
                        &alloc::format!("umx:target/{to}"),
                        &[],
                        &crate::EdgeEvidence {
                            pointers: vec!["b.md#L2".into()],
                            kind: None,
                        },
                    )
                    .map(|_| ())
                    .map_err(|_| ())
            };
            assert!(failed.is_err(), "{by}: graph.json is in the write set");
            assert_eq!(store.incoming, index, "{by}");
            assert_eq!(files_of_all(&files, "author"), disk, "{by}");
            let kept = store
                .loaded("author")
                .unwrap()
                .graph
                .edge(added.id)
                .unwrap();
            assert_eq!(kept.evidence().len(), 1, "{by}");
            files.fail_writes_to(None);
            store
                .batch(&author_ctx(), append(from, to, "b.md#L2"))
                .unwrap();
            assert_eq!(store.incoming, rebuilt(&store), "{by}");
        }
    }

    /// F122 (graph-storage-expert): a retype to a symmetric relation puts the lower (graph id, node id) end first, a
    /// local end counting as the author's graph id, as add does; the external end keeps its graph id.
    #[test]
    fn a_symmetric_retype_orders_an_external_edge_by_graph_and_node() {
        let (mut store, _files, from, to) = setup();
        let (author, target) = (
            store.loaded("author").unwrap().meta.id,
            store.loaded("target").unwrap().meta.id,
        );
        // Store the edge with its higher end first, so the retype must reorder it.
        let (local_first, address) = (
            (author, from) > (target, to),
            alloc::format!("umx:target/{to}"),
        );
        let (a, b) = if local_first {
            (from.to_text(), address.clone())
        } else {
            (address.clone(), from.to_text())
        };
        let added = store
            .edge_add(&author_ctx(), &a, "blocks", &b, &[])
            .unwrap()
            .value
            .edge;
        let done = store
            .edge_edit(
                &author_ctx(),
                &added.id.to_text(),
                &edit(Some("conflicts_with"), false),
            )
            .unwrap();
        let e = &done.value.edge;
        assert_eq!(done.value.changed, ["type", "direction"]);
        let low = core::cmp::min((author, from), (target, to));
        assert_eq!(
            (e.from_graph.unwrap_or(author), e.from),
            low,
            "the lower (graph id, node id) end is from"
        );
        assert_eq!(e.from_graph.or(e.to_graph), Some(target));
        assert_eq!(store.incoming, rebuilt(&store));
    }

    /// F122 (graph-storage-expert): the duplicate check compares graph-qualified ends, so an edit cannot twin an
    /// external edge.
    #[test]
    fn an_edit_cannot_make_an_external_edge_a_twin() {
        let (mut store, _files, from, to) = setup();
        let first = external(&mut store, from, to).unwrap().value.edge;
        let second = store
            .edge_add(
                &author_ctx(),
                &from.to_text(),
                "supports",
                &alloc::format!("umx:target/{to}"),
                &[],
            )
            .unwrap()
            .value
            .edge;
        let refused = store
            .edge_edit(
                &author_ctx(),
                &second.id.to_text(),
                &edit(Some("blocks"), false),
            )
            .unwrap_err();
        assert_eq!(refused.code, "edge_would_duplicate");
        let other = refused.slot("other").unwrap_or_default();
        assert!(
            other.contains(&first.id.to_text()),
            "names the twin: {refused:?}"
        );
    }

    fn run_batch(store: &mut GraphStore, echo: bool, lines: Vec<BatchLine>) -> crate::BatchOutcome {
        store.batch(
            &author_ctx(),
            BatchRequest {
                raised_in: None,
                reason: None,
                echo_sentences: echo,
                dry_run: false,
                lines,
                refs: Default::default(),
                refs_graph: None,
            },
        )
    }

    /// R287-1: a batch that deletes an external edge and then adds the same triple adds a new edge, echo on or off;
    /// the deleted edge is no twin of the new one. The index equals a rebuild after.
    #[test]
    fn a_batch_deletes_an_external_edge_then_adds_its_triple_again() {
        for echo in [false, true] {
            let (mut store, _files, from, to) = setup();
            let a = external(&mut store, from, to).unwrap().value.edge.id;
            let done = run_batch(
                &mut store,
                echo,
                vec![
                    BatchLine {
                        line: 1,
                        op: BatchOp::EdgeDelete { edge: a.to_text() },
                    },
                    BatchLine {
                        line: 2,
                        op: BatchOp::EdgeAdd {
                            id: None,
                            from: Some(from.to_text()),
                            from_ref: None,
                            relation: "blocks".into(),
                            to: Some(alloc::format!("umx:target/{to}")),
                            to_ref: None,
                            sets: Vec::new(),
                            evidence: Default::default(),
                        },
                    },
                ],
            )
            .unwrap();
            let row = &done.value.rows[1];
            assert!(!row.existed, "echo {echo}: {row:?}");
            let added = row.id.expect("a new edge");
            assert_ne!(added, a, "echo {echo}");
            let g = &store.loaded("author").unwrap().graph;
            assert!(g.edge(a).is_none(), "echo {echo}");
            assert_eq!(g.edge(added).unwrap().kind(), "blocks", "echo {echo}");
            let target = store.loaded("target").unwrap().meta.id;
            assert_eq!(store.incoming.entries(target, to).len(), 1, "echo {echo}");
            assert_eq!(store.incoming, rebuilt(&store), "echo {echo}");
        }
    }

    /// R287-1: a batch that deletes external edge A and then retypes edge B, with the same ends, to A's relation is
    /// no duplicate: A is gone by then.
    #[test]
    fn a_batch_retypes_onto_the_triple_of_an_external_edge_it_deleted() {
        let (mut store, _files, from, to) = setup();
        let a = external(&mut store, from, to).unwrap().value.edge.id;
        let b = store
            .edge_add(
                &author_ctx(),
                &from.to_text(),
                "supports",
                &alloc::format!("umx:target/{to}"),
                &[],
            )
            .unwrap()
            .value
            .edge
            .id;
        run_batch(
            &mut store,
            false,
            vec![
                BatchLine {
                    line: 1,
                    op: BatchOp::EdgeDelete { edge: a.to_text() },
                },
                BatchLine {
                    line: 2,
                    op: BatchOp::EdgeEdit {
                        edge: b.to_text(),
                        req: edit(Some("blocks"), false),
                    },
                },
            ],
        )
        .unwrap();
        let g = &store.loaded("author").unwrap().graph;
        assert!(g.edge(a).is_none());
        assert_eq!(g.edge(b).unwrap().kind(), "blocks");
        assert_eq!(store.incoming, rebuilt(&store));
    }

    /// F122: a delete while the far graph is unloaded leaves nothing stale when that graph loads later.
    #[test]
    fn a_delete_with_the_far_graph_unloaded_leaves_nothing_when_it_loads() {
        let (mut store, _files, from, to) = setup();
        let edge = external(&mut store, from, to).unwrap().value.edge.id;
        store.graph_unload(&Ctx::new("test"), "target").unwrap();
        store.edge_delete(&author_ctx(), &edge.to_text()).unwrap();
        store
            .graph_load(&Ctx::new("test"), "/work/target", None)
            .unwrap();
        let target = store.loaded("target").unwrap().meta.id;
        assert!(store.incoming.entries(target, to).is_empty());
        assert_eq!(store.incoming, rebuilt(&store));
    }

    /// F122: the repeated-line and derived refusals still fire on an external edge; nothing changes on disk.
    #[test]
    fn repeated_line_and_derived_refusals_fire_on_an_external_edge() {
        let (mut store, files, from, to) = setup();
        let edge = external(&mut store, from, to).unwrap().value.edge.id;
        let derived = store
            .edge_edit(
                &author_ctx(),
                &edge.to_text(),
                &edit(Some("authored_by"), false),
            )
            .unwrap_err();
        assert_eq!(derived.code, "derived_relation");
        let dir = GraphDir::parse("/work/author").unwrap();
        let text =
            alloc::string::String::from_utf8(files.read(&dir, GraphFile::Edges).unwrap().unwrap())
                .unwrap();
        let id = edge.to_text();
        let repeated: alloc::string::String = text
            .lines()
            .map(|line| {
                if line.contains(id.as_str()) {
                    alloc::format!("{}\n{line}\n", line.replace("\"blocks\"", "\"supports\""))
                } else {
                    alloc::format!("{line}\n")
                }
            })
            .collect();
        store.graph_unload(&Ctx::new("test"), "author").unwrap();
        files
            .write(&dir, GraphFile::Edges, repeated.as_bytes())
            .unwrap();
        store
            .graph_load(&Ctx::new("test"), "/work/author", None)
            .unwrap();
        let before = files_of(&files, "author");
        let refused = store
            .edge_delete(&author_ctx(), &edge.to_text())
            .unwrap_err();
        assert_eq!(refused.code, "edge_repeated");
        assert_eq!(files_of(&files, "author"), before);
        assert_eq!(store.incoming, rebuilt(&store));
    }

    /// F122 (graph-storage-expert): the twin check compares graph-qualified ends. Graph `other` holds a node with the
    /// same id as `target`'s; an edge to it is not a twin of an edge to `target`'s, so the retype goes through.
    #[test]
    fn the_twin_check_tells_the_same_node_id_in_two_graphs_apart() {
        let (mut store, files, from, to) = setup();
        store
            .graph_new(&Ctx::new("test"), "/work/other", "other", None, false, None)
            .unwrap();
        store.graph_unload(&Ctx::new("test"), "other").unwrap();
        let [_, nodes, _] = files_of_all(&files, "target");
        let other = GraphDir::parse("/work/other").unwrap();
        files
            .write(&other, GraphFile::Nodes, &nodes.unwrap())
            .unwrap();
        store
            .graph_load(&Ctx::new("test"), "/work/other", None)
            .unwrap();
        external(&mut store, from, to).unwrap();
        let second = store
            .edge_add(
                &author_ctx(),
                &from.to_text(),
                "supports",
                &alloc::format!("umx:other/{to}"),
                &[],
            )
            .unwrap()
            .value
            .edge;
        let done = store
            .edge_edit(
                &author_ctx(),
                &second.id.to_text(),
                &edit(Some("blocks"), false),
            )
            .unwrap();
        assert_eq!(done.value.changed, ["type"]);
        assert_eq!(
            done.value.edge.to_graph,
            Some(store.loaded("other").unwrap().meta.id)
        );
        assert_eq!(store.incoming, rebuilt(&store));
    }

    /// F122 (security): an edit's and a delete's replies and errors are the same whether the far node exists and its
    /// graph is open, secured, unloaded, or loaded without that node. Fresh stores with the same ids and clock.
    #[test]
    fn replies_are_the_same_whatever_the_far_graph_holds() {
        let run = |far: &str| -> alloc::string::String {
            let (mut store, files, from, to) = setup_with(far == "secured");
            let added = external(&mut store, from, to).unwrap().value.edge;
            match far {
                "unloaded" => {
                    store.graph_unload(&Ctx::new("test"), "target").unwrap();
                }
                "missing" => {
                    store.graph_unload(&Ctx::new("test"), "target").unwrap();
                    let dir = GraphDir::parse("/work/target").unwrap();
                    files.write(&dir, GraphFile::Nodes, b"").unwrap();
                    store
                        .graph_load(&Ctx::new("test"), "/work/target", None)
                        .unwrap();
                }
                _ => {}
            }
            let id = added.id.to_text();
            let ctx = author_ctx();
            let derived = store.edge_edit(&ctx, &id, &edit(Some("similar_to"), false));
            let edited = store.edge_edit(&ctx, &id, &edit(Some("supports"), true));
            let deleted = store.edge_delete(&ctx, &id);
            alloc::format!("{derived:?}\n{edited:?}\n{deleted:?}")
        };
        let open = run("open");
        assert!(
            !open.contains("target"),
            "the far title or alias never shows: {open}"
        );
        for far in ["secured", "unloaded", "missing"] {
            assert_eq!(run(far), open, "{far}");
        }
    }

    /// F122: a batch that deletes one external edge and edits another leaves the index equal to a rebuild; a dry run
    /// and a failed batch leave it, and both files, unchanged.
    #[test]
    fn a_batch_changes_external_edges_and_the_index_follows_after_commit() {
        let lines = |a: Id, b: Id| {
            vec![
                BatchLine {
                    line: 1,
                    op: BatchOp::EdgeDelete { edge: a.to_text() },
                },
                BatchLine {
                    line: 2,
                    op: BatchOp::EdgeEdit {
                        edge: b.to_text(),
                        req: edit(Some("undercuts"), false),
                    },
                },
            ]
        };
        for mode in ["dry", "failed", "real"] {
            let (mut store, files, from, to) = setup();
            let a = external(&mut store, from, to).unwrap().value.edge.id;
            let b = store
                .edge_add(
                    &author_ctx(),
                    &alloc::format!("umx:target/{to}"),
                    "supports",
                    &from.to_text(),
                    &[],
                )
                .unwrap()
                .value
                .edge
                .id;
            let (index, disk) = (store.incoming.clone(), files_of(&files, "author"));
            if mode == "failed" {
                files.fail_writes_to(Some(GraphFile::Graph));
            }
            let done = store.batch(
                &author_ctx(),
                BatchRequest {
                    raised_in: None,
                    reason: None,
                    echo_sentences: false,
                    dry_run: mode == "dry",
                    lines: lines(a, b),
                    refs: Default::default(),
                    refs_graph: None,
                },
            );
            if mode == "real" {
                done.unwrap();
                let g = &store.loaded("author").unwrap().graph;
                assert!(g.edge(a).is_none() && g.edge(b).unwrap().kind() == "undercuts");
                assert_eq!(store.incoming, rebuilt(&store));
                let target = store.loaded("target").unwrap().meta.id;
                assert_eq!(store.incoming.entries(target, to).len(), 1);
            } else {
                assert_eq!(done.is_err(), mode == "failed", "{mode}");
                assert_eq!(store.incoming, index, "{mode}");
                assert_eq!(files_of(&files, "author"), disk, "{mode}");
            }
        }
    }

    /// ws008 E2: `entity unlink` removes a cross-graph link and the index follows, equal to one rebuilt from the
    /// graphs; the link is gone from the author's file.
    #[test]
    fn entity_unlink_of_an_external_link_keeps_the_index_exact() {
        let (mut store, files, from, to) = setup();
        let ctx = Ctx::new("test").graph("author");
        let b = alloc::format!("umx:target/{to}");
        let linked = store.entity_link(&ctx, &from.to_text(), &b, false).unwrap();
        let target = store.loaded("target").unwrap().meta.id;
        assert_eq!(store.incoming.entries(target, to).len(), 1);
        let gone = store.entity_unlink(&ctx, &from.to_text(), &b).unwrap();
        assert_eq!(
            gone.value.removed.unwrap().1.id,
            linked.value.edge.unwrap().id
        );
        assert!(store.incoming.entries(target, to).is_empty());
        assert_eq!(store.incoming, rebuilt(&store));
        let dir = GraphDir::parse("/work/author").unwrap();
        let edges = files
            .read(&dir, GraphFile::Edges)
            .unwrap()
            .unwrap_or_default();
        assert!(
            !alloc::string::String::from_utf8_lossy(&edges).contains("same_as"),
            "the link is gone from the author's file"
        );
    }

    /// ws008 E2 (R250-3): a link stored in B to a node of a graph neither loaded nor remembered (named by its full id)
    /// is removed by `entity unlink A B` with no --graph: the unavailable first graph never blocks B's, and the index
    /// follows.
    #[test]
    fn entity_unlink_tries_the_second_graph_when_the_first_is_not_loaded() {
        let (mut store, _files, from, to) = setup();
        let target = store.loaded("target").unwrap().meta.id;
        let a = alloc::format!("umx:{}/{}", target.to_text(), to.to_text());
        let b = alloc::format!("umx:author/{}", from.to_text());
        store
            .entity_link(&Ctx::new("test").graph("author"), &b, &a, false)
            .unwrap();
        store.graph_unload(&Ctx::new("test"), "target").unwrap();
        let gone = store.entity_unlink(&Ctx::new("test"), &a, &b).unwrap();
        assert!(
            gone.value.removed.is_some(),
            "the link stored in B is removed"
        );
        assert_eq!(store.incoming, rebuilt(&store));
    }

    #[test]
    fn index_failed_write_is_unchanged_in_memory_and_on_disk() {
        let (mut store, files, from, to) = setup();
        let before = store.incoming.clone();
        let dir = GraphDir::parse("/work/author").unwrap();
        let meta = files.read(&dir, GraphFile::Graph).unwrap();
        let edges = files.read(&dir, GraphFile::Edges).unwrap();
        files.fail_writes_to(Some(GraphFile::Graph));
        assert!(external(&mut store, from, to).is_err());
        assert_eq!(store.incoming, before);
        assert_eq!(files.read(&dir, GraphFile::Graph).unwrap(), meta);
        assert_eq!(files.read(&dir, GraphFile::Edges).unwrap(), edges);
        assert_eq!(store.loaded("author").unwrap().meta.umx, 1);
        assert_eq!(store.incoming, rebuilt(&store));
    }

    #[test]
    fn index_dry_run_keeps_graph_files_and_index_unchanged() {
        let (mut store, files, from, to) = setup();
        let before = store.incoming.clone();
        let edge_count = store.loaded("author").unwrap().graph.edge_count();
        let dir = GraphDir::parse("/work/author").unwrap();
        let meta = files.read(&dir, GraphFile::Graph).unwrap();
        let edges = files.read(&dir, GraphFile::Edges).unwrap();
        store
            .batch(
                &Ctx::new("test").graph("author"),
                BatchRequest {
                    raised_in: None,
                    reason: None,
                    echo_sentences: false,
                    dry_run: true,
                    lines: vec![BatchLine {
                        line: 1,
                        op: BatchOp::EdgeAdd {
                            id: None,
                            from: Some(from.to_text()),
                            from_ref: None,
                            relation: "blocks".into(),
                            to: Some(alloc::format!("umx:target/{to}")),
                            to_ref: None,
                            sets: Vec::new(),
                            evidence: Default::default(),
                        },
                    }],
                    refs: Default::default(),
                    refs_graph: None,
                },
            )
            .unwrap();
        assert_eq!(store.incoming, before);
        assert_eq!(
            store.loaded("author").unwrap().graph.edge_count(),
            edge_count
        );
        assert_eq!(files.read(&dir, GraphFile::Graph).unwrap(), meta);
        assert_eq!(files.read(&dir, GraphFile::Edges).unwrap(), edges);
    }

    #[test]
    fn index_not_synced_write_is_committed_with_its_format_upgrade() {
        let (mut store, files, from, to) = setup();
        files.fail_sync(true);
        let done = external(&mut store, from, to).unwrap();
        assert!(done.notes.iter().any(|p| p.code == "write_not_durable"));
        assert_eq!(store.incoming.len(), 1);
        assert_eq!(store.incoming, rebuilt(&store));
        assert_eq!(store.loaded("author").unwrap().meta.umx, 3);
        let dir = GraphDir::parse("/work/author").unwrap();
        assert_eq!(
            umx_core::codec::decode_graph(&files.read(&dir, GraphFile::Graph).unwrap().unwrap())
                .unwrap()
                .umx,
            3
        );
    }

    #[test]
    fn index_keeps_target_unload_entries_and_drops_only_the_unloaded_source() {
        let (mut store, _, from, to) = setup();
        external(&mut store, from, to).unwrap();
        let before = store.incoming.clone();
        let target = store.loaded("target").unwrap().meta.id;
        store.graph_unload(&Ctx::new("test"), "target").unwrap();
        assert_eq!(store.incoming, before);
        assert_eq!(
            store.read_end(&Ctx::new("test"), &[], target, to).state,
            Some("graph_not_loaded")
        );
        assert_eq!(store.incoming, rebuilt(&store));
        store
            .graph_load(&Ctx::new("test"), "/work/target", None)
            .unwrap();
        assert_eq!(store.incoming, before);
        store.graph_unload(&Ctx::new("test"), "author").unwrap();
        assert!(store.incoming.is_empty());
        assert_eq!(store.incoming, rebuilt(&store));
    }

    #[test]
    fn index_survives_node_renumbering_reload_and_idle_unload() {
        let (mut store, _, from, to) = setup();
        external(&mut store, from, to).unwrap();
        let before = store.incoming.clone();
        for alias in ["author", "target"] {
            let graph = &mut store.graphs.get_mut(alias).unwrap().graph;
            let mut node = graph.nodes().next().unwrap().to_node();
            node.id = Id(0);
            graph.insert_node(node).unwrap();
            assert_eq!(store.incoming, before);
            assert_eq!(store.incoming, rebuilt(&store));
        }
        store
            .graph_load(&Ctx::new("test"), "/work/author", None)
            .unwrap();
        assert_eq!(store.incoming.len(), 1);
        assert_eq!(store.incoming, rebuilt(&store));
        store.set_idle_unload_ms(1);
        for loaded in store.graphs.values() {
            loaded
                .last_used
                .store(0, core::sync::atomic::Ordering::Relaxed);
        }
        assert_eq!(store.unload_idle().len(), 2);
        assert!(store.incoming.is_empty());
    }

    #[test]
    fn index_failed_batch_rolls_back_earlier_external_edges() {
        let (mut store, files, from, to) = setup();
        let before = store.incoming.clone();
        let edge_count = store.loaded("author").unwrap().graph.edge_count();
        let dir = GraphDir::parse("/work/author").unwrap();
        let meta = files.read(&dir, GraphFile::Graph).unwrap();
        let result = store.batch(
            &Ctx::new("test").graph("author"),
            BatchRequest {
                raised_in: None,
                reason: None,
                echo_sentences: false,
                dry_run: false,
                lines: vec![
                    BatchLine {
                        line: 1,
                        op: BatchOp::EdgeAdd {
                            id: None,
                            from: Some(from.to_text()),
                            from_ref: None,
                            relation: "blocks".into(),
                            to: Some(alloc::format!("umx:target/{to}")),
                            to_ref: None,
                            sets: Vec::new(),
                            evidence: Default::default(),
                        },
                    },
                    BatchLine {
                        line: 2,
                        op: BatchOp::EdgeAdd {
                            id: None,
                            from: None,
                            from_ref: Some("missing_ref".into()),
                            relation: "blocks".into(),
                            to: Some(from.to_text()),
                            to_ref: None,
                            sets: Vec::new(),
                            evidence: Default::default(),
                        },
                    },
                ],
                refs: Default::default(),
                refs_graph: None,
            },
        );
        assert!(result.is_err());
        assert_eq!(store.incoming, before);
        assert_eq!(
            store.loaded("author").unwrap().graph.edge_count(),
            edge_count
        );
        assert_eq!(files.read(&dir, GraphFile::Graph).unwrap(), meta);
    }

    fn reverse_pairs() -> (GraphStore, Arc<FakeGraphFiles>, Id, Id) {
        let (mut store, files, from, to) = setup();
        let ctx = Ctx::new("test").graph("author");
        let local = store
            .node_add(
                &ctx,
                NodeAddRequest {
                    kind: "problem".into(),
                    title: "Other local".into(),
                    ..Default::default()
                },
            )
            .unwrap()
            .value
            .node
            .id;
        for (a, b) in [
            (from.to_text(), local.to_text()),
            (local.to_text(), from.to_text()),
            (from.to_text(), alloc::format!("umx:target/{to}")),
            (alloc::format!("umx:target/{to}"), from.to_text()),
        ] {
            store.edge_add(&ctx, &a, "rivals", &b, &[]).unwrap();
        }
        (store, files, from, to)
    }

    #[test]
    fn symmetry_declaration_collapses_qualified_pairs_and_matches_a_rebuilt_index() {
        let (mut store, files, _, to) = reverse_pairs();
        let ctx = Ctx::new("test").graph("author");
        let done = store
            .ontology_add_relation(&ctx, "rivals", true, None)
            .unwrap();
        let loaded = store.loaded("author").unwrap();
        assert_eq!(
            loaded
                .graph
                .edges()
                .filter(|e| e.kind() == "rivals"
                    && e.from_graph().is_none()
                    && e.to_graph().is_none())
                .count(),
            1
        );
        assert_eq!(
            loaded
                .graph
                .edges()
                .filter(|e| e.kind() == "rivals")
                .count(),
            2,
            "local and external reverse pairs collapse alike"
        );
        assert_eq!(
            done.notes
                .iter()
                .filter(|p| p.code == "symmetric_duplicate")
                .count(),
            2
        );
        assert_eq!(loaded.graph.kept_line_count(), 2);
        assert_eq!(store.incoming.len(), 1);
        assert_eq!(store.incoming, rebuilt(&store));
        let target = store.loaded("target").unwrap().meta.id;
        assert_eq!(store.incoming.entries(target, to).len(), 1);
        let bytes = files
            .read(&GraphDir::parse("/work/author").unwrap(), GraphFile::Edges)
            .unwrap()
            .unwrap();
        assert_eq!(
            bytes.iter().filter(|byte| **byte == b'\n').count(),
            loaded.graph.edge_count() + loaded.graph.kept_line_count()
        );
        store
            .graph_load(&Ctx::new("test"), "/work/author", None)
            .unwrap();
        assert_eq!(
            store
                .loaded("author")
                .unwrap()
                .graph
                .edges()
                .filter(|e| e.kind() == "rivals")
                .count(),
            2
        );
        assert_eq!(store.incoming.len(), 1);
        assert_eq!(store.incoming, rebuilt(&store));
    }

    #[test]
    fn failed_symmetry_declaration_preserves_graph_files_and_index() {
        let (mut store, files, _, _) = reverse_pairs();
        let ctx = Ctx::new("test").graph("author");
        let before = store.incoming.clone();
        let loaded = store.loaded("author").unwrap();
        let ontology = loaded.meta.ontology.clone();
        let encoded = loaded.graph.encode_edges();
        let path = GraphDir::parse("/work/author").unwrap();
        let meta = files.read(&path, GraphFile::Graph).unwrap();
        files.fail_writes_to(Some(GraphFile::Graph));
        assert!(
            store
                .ontology_add_relation(&ctx, "rivals", true, None)
                .is_err()
        );
        assert_eq!(store.loaded("author").unwrap().meta.ontology, ontology);
        assert_eq!(
            store.loaded("author").unwrap().graph.encode_edges(),
            encoded
        );
        assert_eq!(files.read(&path, GraphFile::Graph).unwrap(), meta);
        assert_eq!(
            files.read(&path, GraphFile::Edges).unwrap().unwrap(),
            encoded
        );
        assert_eq!(store.incoming, before);
        assert_eq!(store.incoming, rebuilt(&store));
    }
}

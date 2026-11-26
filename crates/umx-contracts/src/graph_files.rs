//! GraphFiles contract. A factory returns a fresh, empty port, the root it may use, and a guard dropped after the
//! clause (for example a temp folder). The port also implements [`SetHarness`], the test inputs the set clauses
//! need: crash, restart and tamper.

use umx_core::codec::GraphFile;
use umx_core::ports::{
    Change, CrashPoint, EntryKind, FileRef, FileStamp, FilesError, GraphDir, GraphFiles,
    GraphStamps, ModelKey, RuleSetFile,
};
use umx_core::setup::GraphFilesSetup;
use umx_core::vectors::{ShardFile, ShardKind};

/// What the set clauses (gf_c14 to gf_c22) need beyond the port. Each adapter implements it in its own tests over
/// test-only inherent methods; umx-compose never calls them.
pub trait SetHarness: GraphFiles + Sized {
    /// Stops the next `write_set` at `point`, as if the process died there.
    fn crash_at(&self, point: Option<CrashPoint>);
    /// Makes writing this file's temp fail (a full disk), so a set fails before the point of no return.
    fn fail_temp_of(&self, file: Option<GraphFile>);
    /// A new instance over the same storage, as after a restart: the files stay, everything else is lost.
    fn restart(&self) -> Self;
    /// Writes a file behind the port's back (a pull, a checkout, a hand edit).
    fn tamper(&self, dir: &GraphDir, file: impl Into<FileRef>, bytes: &[u8]);
    /// A file's bytes straight from storage, with no recovery run.
    fn raw(&self, dir: &GraphDir, file: impl Into<FileRef>) -> Option<Vec<u8>>;
    /// The names in the folder that start with `.umx-tmp-` (temp files and the marker), sorted.
    fn leftovers(&self, dir: &GraphDir) -> Vec<String>;
    /// Every byte a read so far took out of storage, so a clause can prove a body was never read (gf_c26).
    fn bytes_read(&self) -> u64;
    /// Touches a file behind the port's back: the same bytes, a new modification time (F144).
    fn touch(&self, dir: &GraphDir, file: GraphFile);
    /// Replaces a file behind the port's back with a symlink to a copy of its bytes (F144). An adapter with no
    /// links (the fake) replaces it with the same bytes.
    fn swap_for_link(&self, dir: &GraphDir, file: GraphFile);
    /// Lands `bytes` in `file` while the next `read_stamped` of it is under way: after its open, before it returns.
    fn change_during_next_read(&self, dir: &GraphDir, file: GraphFile, bytes: &[u8]);
    /// Lands `bytes` in `file` once the next set's temps and marker are in place, before its first rename (F144).
    fn change_after_next_marker(&self, dir: &GraphDir, file: impl Into<FileRef>, bytes: &[u8]);
    /// Lands `bytes` in `file` once the next set's temps are written, before it takes the old stamps for its marker
    /// (F144).
    fn change_before_next_marker(&self, dir: &GraphDir, file: impl Into<FileRef>, bytes: &[u8]);
}

// gf_c12 retired 2026-11-10: remove was folded into write_set (bytes None); covered by gf_c14 and gf_c21. Clause
// ids are never reused.
pub const CLAUSES: &[&str] = &[
    "gf_c1_write_then_read_is_identical",
    "gf_c2_read_missing_is_none",
    "gf_c3_overwrite_replaces_the_whole_file",
    "gf_c4_list_is_in_graph_file_order",
    "gf_c5_list_of_missing_dir_is_empty",
    "gf_c6_dirs_are_separate",
    "gf_c7_outside_roots_is_refused",
    "gf_c8_empty_file_round_trips",
    "gf_c9_write_creates_missing_dirs",
    "gf_c10_read_returns_a_copy",
    "gf_c11_resolve_is_stable_and_confined",
    "gf_c13_an_error_means_nothing_changed",
    "gf_c14_set_applies_every_change",
    "gf_c15_set_error_changes_nothing",
    "gf_c16_crash_before_commit_keeps_the_old_state",
    "gf_c17_crash_after_commit_rolls_forward",
    "gf_c18_crash_mid_renames_rolls_forward",
    "gf_c19_changed_target_is_a_conflict",
    "gf_c20_set_never_leaves_the_roots",
    "gf_c21_invalid_set_is_refused",
    "gf_c22_one_file_write_is_a_set",
    "gf_c23_a_set_after_an_unfinished_set_finishes_it_first",
    "gf_c24_concurrent_sets_serialize",
    "gf_c25_resolve_never_recovers",
    "gf_c26_an_over_cap_file_is_refused_unread",
    "gf_c27_a_change_over_cap_is_refused_unwritten",
    "gf_c28_recovery_and_old_stamps_never_read_an_over_cap_body",
    "gf_c29_similar_files_round_trip_and_list_by_kind",
    "gf_c30_a_set_returns_the_stamps_a_fresh_look_sees",
    "gf_c31_a_change_since_the_stamps_refuses_the_set",
    "gf_c32_read_stamped_stamps_the_bytes_it_read",
    "gf_c33_a_change_after_the_marker_refuses_the_set",
    "gf_c34_receipts_round_trip_capped_and_are_no_graph_file",
    "gf_c35_a_set_with_receipts_is_all_or_nothing_across_crashes",
    "gf_c36_a_receipts_only_set_writes_and_keeps_the_content_stamps",
    "gf_c37_a_changed_also_file_refuses_the_set",
    "gf_c38_rule_sets_are_graph_local_sorted_and_capped",
    "gf_c39_rule_sets_commit_with_metadata_and_check_their_stamps",
    "gf_c40_rule_set_path_stamps_are_body_independent",
    "gf_c41_rule_set_links_never_follow_the_target",
];

fn dir(root: &GraphDir, child: &str) -> GraphDir {
    root.join(child).expect("child dir")
}

pub fn gf_c38_rule_sets_are_graph_local_sorted_and_capped<P: SetHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let g = dir(&root, "g");
    let set = |name: &str| FileRef::RuleSet(RuleSetFile::new(name).unwrap());
    assert_eq!(port.list_rule_sets(&g).unwrap().entries, []);
    assert_eq!(
        port.read_ref_stamped(&g, &set("alpha")).unwrap(),
        (None, FileStamp::ABSENT)
    );
    port.write_set(
        &g,
        &[
            put(set("zeta"), b"{}\n"),
            put(set("alpha"), b"{ \"rules\": [] }\n"),
        ],
    )
    .unwrap();
    let before = port.bytes_read();
    let listing = port.list_rule_sets(&g).unwrap();
    assert!(!listing.limited);
    assert_eq!(
        listing.entries,
        [
            ("alpha.json".into(), EntryKind::File),
            ("zeta.json".into(), EntryKind::File),
        ]
    );
    assert_eq!(port.bytes_read(), before, "listing reads no bodies");
    assert_eq!(port.list(&g).unwrap(), []);
    assert_eq!(
        port.list_rule_sets(&dir(&root, "another")).unwrap().entries,
        []
    );
    let file = set("alpha");
    let cap = file.cap();
    port.tamper(&g, file.clone(), &vec![b' '; cap as usize + 1]);
    let before = port.bytes_read();
    assert_eq!(
        port.read_ref_stamped(&g, &file),
        Err(FilesError::TooLarge {
            file: file.clone(),
            size: cap + 1,
            cap,
        })
    );
    assert_eq!(port.bytes_read(), before, "an over-cap set body is unread");
    for i in 0..umx_core::ports::graph_files::MAX_RULE_SET_ENTRIES {
        let name = format!("set_{i}");
        port.tamper(&g, set(&name), b"{}");
    }
    let listing = port.list_rule_sets(&g).unwrap();
    assert!(listing.limited);
    assert!(
        listing.entries.is_empty(),
        "no OS-order partial folder at the directory cap"
    );
}

pub fn gf_c39_rule_sets_commit_with_metadata_and_check_their_stamps<P: SetHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let set = FileRef::RuleSet(RuleSetFile::new("kit").unwrap());
    for (i, point) in [
        CrashPoint::AfterTemps,
        CrashPoint::AfterMarker,
        CrashPoint::AfterFirstRename,
    ]
    .into_iter()
    .enumerate()
    {
        let g = dir(&root, &format!("g{i}"));
        port.write(&g, GraphFile::Graph, b"old\n").unwrap();
        let stamps = loaded_stamps(&port, &g);
        let (_, stamp) = port.read_ref_stamped(&g, &set).unwrap();
        port.crash_at(Some(point));
        assert!(
            port.write_set_checked_also(
                &g,
                &[put(GraphFile::Graph, b"new\n"), put(set.clone(), b"{}\n"),],
                Some(&stamps),
                &[(set.clone(), stamp)]
            )
            .is_err()
        );
        let after = port.restart();
        let got = (
            read(&after, &g, GraphFile::Graph),
            after.read_ref(&g, &set).unwrap(),
        );
        if point == CrashPoint::AfterTemps {
            assert_eq!(got, (Some(b"old\n".to_vec()), None));
        } else {
            assert_eq!(got, (Some(b"new\n".to_vec()), Some(b"{}\n".to_vec())));
        }
    }
    let g = dir(&root, "changed");
    port.write(&g, GraphFile::Graph, b"old\n").unwrap();
    port.write_set(&g, &[put(set.clone(), b"{}\n")]).unwrap();
    let stamps = loaded_stamps(&port, &g);
    let (_, stamp) = port.read_ref_stamped(&g, &set).unwrap();
    port.tamper(&g, set.clone(), b"{ \"rules_off\": [] }\n");
    assert_eq!(
        port.write_set_checked_also(
            &g,
            &[
                put(GraphFile::Graph, b"new\n"),
                Change {
                    file: set.clone(),
                    bytes: None
                },
            ],
            Some(&stamps),
            &[(set.clone(), stamp)]
        ),
        Err(FilesError::Changed(vec![set.clone()]))
    );
    assert_eq!(
        read(&port, &g, GraphFile::Graph).as_deref(),
        Some(&b"old\n"[..])
    );
    let (_, stamp) = port.read_ref_stamped(&g, &set).unwrap();
    port.write_set_checked_also(
        &g,
        &[Change {
            file: set.clone(),
            bytes: None,
        }],
        Some(&stamps),
        &[(set.clone(), stamp)],
    )
    .unwrap();
    assert_eq!(port.read_ref(&g, &set).unwrap(), None);
}

pub fn gf_c40_rule_set_path_stamps_are_body_independent<P: SetHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let g = dir(&root, "g");
    let file = FileRef::RuleSet(RuleSetFile::new("kit").unwrap());
    assert_eq!(port.stamp_ref(&g, &file).unwrap(), FileStamp::ABSENT);
    port.write_set(&g, &[put(file.clone(), b"{}\n")]).unwrap();
    let (_, read) = port.read_ref_stamped(&g, &file).unwrap();
    let before = port.bytes_read();
    assert_eq!(port.stamp_ref(&g, &file).unwrap(), read);
    assert_eq!(port.bytes_read(), before, "stamping reads no body");
    port.tamper(&g, file.clone(), &vec![b' '; file.cap() as usize + 1]);
    let before = port.bytes_read();
    let changed = port.stamp_ref(&g, &file).unwrap();
    assert_ne!(changed, read);
    assert_ne!(changed, FileStamp::ABSENT);
    assert_eq!(port.stamp_ref(&g, &file).unwrap(), changed);
    assert_eq!(
        port.bytes_read(),
        before,
        "even an over-cap body stays unread"
    );
    let stamps = port.stamps(&g).unwrap();
    port.write_set_checked_also(
        &g,
        &[Change {
            file: file.clone(),
            bytes: None,
        }],
        Some(&stamps),
        &[(file.clone(), changed)],
    )
    .unwrap();
    assert_eq!(port.stamp_ref(&g, &file).unwrap(), FileStamp::ABSENT);
    port.write(&g, GraphFile::Graph, b"old\n").unwrap();
    port.write_set(&g, &[put(file.clone(), b"{}\n")]).unwrap();
    let old = port.stamp_ref(&g, &file).unwrap();
    port.crash_at(Some(CrashPoint::AfterMarker));
    assert!(
        port.write_set(
            &g,
            &[
                put(GraphFile::Graph, b"new\n"),
                put(file.clone(), b"{ \"rules\": [] }\n"),
            ]
        )
        .is_err()
    );
    let pending = port.leftovers(&g);
    let before = port.bytes_read();
    assert_eq!(port.stamp_ref(&g, &file).unwrap(), old);
    assert_eq!(
        port.bytes_read(),
        before,
        "a physical stamp never runs body-reading recovery"
    );
    assert_eq!(
        port.leftovers(&g),
        pending,
        "the pending write stays pending"
    );
}

pub fn gf_c41_rule_set_links_never_follow_the_target<P: SetHarness + FindHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let (g, outside) = (dir(&root, "g"), dir(&root, "outside"));
    let file = FileRef::RuleSet(RuleSetFile::new("kit").unwrap());
    port.write_set(&outside, &[put(file.clone(), b"keep")])
        .unwrap();
    port.write_set(&g, &[put(file.clone(), b"seed")]).unwrap();
    port.write_set(
        &g,
        &[Change {
            file: file.clone(),
            bytes: None,
        }],
    )
    .unwrap();
    port.make_link(
        &g.join(&file.path()).unwrap(),
        &outside.join(&file.path()).unwrap(),
    );
    let before = port.bytes_read();
    assert!(matches!(
        port.read_ref(&g, &file),
        Err(FilesError::OutsideRoots(_))
    ));
    assert!(matches!(
        port.read_ref_stamped(&g, &file),
        Err(FilesError::OutsideRoots(_))
    ));
    assert!(matches!(
        port.write_set(&g, &[put(file.clone(), b"replacement")]),
        Err(FilesError::OutsideRoots(_))
    ));
    let stamp = port.stamp_ref(&g, &file).unwrap();
    assert_ne!(stamp, FileStamp::ABSENT);
    assert_eq!(port.bytes_read(), before, "the link's target is never read");
    let expect = port.stamps(&g).unwrap();
    port.write_set_checked_also(
        &g,
        &[Change {
            file: file.clone(),
            bytes: None,
        }],
        Some(&expect),
        &[(file.clone(), stamp)],
    )
    .unwrap();
    assert_eq!(port.read_ref(&g, &file).unwrap(), None);
    assert_eq!(port.raw(&outside, file.clone()), Some(b"keep".to_vec()));

    let h = dir(&root, "h");
    port.make_folder(&h);
    port.make_link(&h.join("rules").unwrap(), &outside.join("rules").unwrap());
    assert!(matches!(
        port.list_rule_sets(&h),
        Err(FilesError::OutsideRoots(_))
    ));
    assert!(matches!(
        port.read_ref(&h, &file),
        Err(FilesError::OutsideRoots(_))
    ));
    assert!(matches!(
        port.stamp_ref(&h, &file),
        Err(FilesError::OutsideRoots(_))
    ));
    assert!(matches!(
        port.write_set(&h, &[put(file.clone(), b"replacement")]),
        Err(FilesError::OutsideRoots(_))
    ));
    assert!(matches!(
        port.write_set(
            &h,
            &[Change {
                file: file.clone(),
                bytes: None
            }]
        ),
        Err(FilesError::OutsideRoots(_))
    ));
    assert_eq!(port.raw(&outside, file), Some(b"keep".to_vec()));
}

pub fn gf_c1_write_then_read_is_identical<P: GraphFiles, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let g = dir(&root, "g");
    let bytes = b"{\"id\":\"x\"}\n\xe2\x9c\x93 bytes, not text\n".to_vec();
    port.write(&g, GraphFile::Nodes, &bytes).unwrap();
    assert_eq!(port.read(&g, GraphFile::Nodes).unwrap(), Some(bytes));
}

pub fn gf_c2_read_missing_is_none<P: GraphFiles, G>(factory: impl Fn() -> (P, GraphDir, G)) {
    let (port, root, _guard) = factory();
    let g = dir(&root, "g");
    assert_eq!(port.read(&g, GraphFile::Graph).unwrap(), None);
    port.write(&g, GraphFile::Graph, b"{}").unwrap();
    assert_eq!(port.read(&g, GraphFile::Edges).unwrap(), None);
}

pub fn gf_c3_overwrite_replaces_the_whole_file<P: GraphFiles, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let g = dir(&root, "g");
    port.write(&g, GraphFile::Edges, &[b'a'; 4096]).unwrap();
    port.write(&g, GraphFile::Edges, b"short\n").unwrap();
    assert_eq!(
        port.read(&g, GraphFile::Edges).unwrap().as_deref(),
        Some(&b"short\n"[..])
    );
}

pub fn gf_c4_list_is_in_graph_file_order<P: GraphFiles, G>(factory: impl Fn() -> (P, GraphDir, G)) {
    let (port, root, _guard) = factory();
    let g = dir(&root, "g");
    port.write(&g, GraphFile::Edges, b"").unwrap();
    port.write(&g, GraphFile::Graph, b"{}").unwrap();
    assert_eq!(
        port.list(&g).unwrap(),
        vec![GraphFile::Graph, GraphFile::Edges]
    );
    port.write(&g, GraphFile::Nodes, b"").unwrap();
    assert_eq!(port.list(&g).unwrap(), GraphFile::ALL.to_vec());
}

pub fn gf_c5_list_of_missing_dir_is_empty<P: GraphFiles, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    assert!(port.list(&dir(&root, "nothing/here")).unwrap().is_empty());
}

pub fn gf_c6_dirs_are_separate<P: GraphFiles, G>(factory: impl Fn() -> (P, GraphDir, G)) {
    let (port, root, _guard) = factory();
    let (a, b, ab) = (dir(&root, "a"), dir(&root, "b"), dir(&root, "a/b"));
    port.write(&a, GraphFile::Nodes, b"a\n").unwrap();
    assert_eq!(port.read(&b, GraphFile::Nodes).unwrap(), None);
    assert_eq!(port.read(&ab, GraphFile::Nodes).unwrap(), None);
    assert!(port.list(&b).unwrap().is_empty());
}

pub fn gf_c7_outside_roots_is_refused<P: GraphFiles, G>(factory: impl Fn() -> (P, GraphDir, G)) {
    let (port, root, _guard) = factory();
    // A sibling whose name extends the root's last part must not count as inside it.
    let sibling = GraphDir::parse(&format!("{}-sibling/g", root.as_str())).unwrap();
    let elsewhere = GraphDir::parse("/umx-contract-outside-roots/g").unwrap();
    for outside in [sibling, elsewhere] {
        let refused = |r: Result<(), FilesError>| matches!(r, Err(FilesError::OutsideRoots(_)));
        assert!(
            refused(port.write(&outside, GraphFile::Graph, b"{}").map(|_| ())),
            "write {outside}"
        );
        assert!(
            refused(port.read(&outside, GraphFile::Graph).map(|_| ())),
            "read {outside}"
        );
        assert!(refused(port.list(&outside).map(|_| ())), "list {outside}");
    }
    // `..` never reaches the port: GraphDir refuses it.
    assert!(GraphDir::parse(&format!("{}/../etc", root.as_str())).is_err());
}

pub fn gf_c8_empty_file_round_trips<P: GraphFiles, G>(factory: impl Fn() -> (P, GraphDir, G)) {
    let (port, root, _guard) = factory();
    let g = dir(&root, "g");
    port.write(&g, GraphFile::Nodes, b"").unwrap();
    assert_eq!(port.read(&g, GraphFile::Nodes).unwrap(), Some(Vec::new()));
    assert_eq!(port.list(&g).unwrap(), vec![GraphFile::Nodes]);
}

pub fn gf_c9_write_creates_missing_dirs<P: GraphFiles, G>(factory: impl Fn() -> (P, GraphDir, G)) {
    let (port, root, _guard) = factory();
    let deep = dir(&root, "x/y/z");
    port.write(&deep, GraphFile::Graph, b"{}").unwrap();
    assert_eq!(
        port.read(&deep, GraphFile::Graph).unwrap().as_deref(),
        Some(&b"{}"[..])
    );
}

pub fn gf_c10_read_returns_a_copy<P: GraphFiles, G>(factory: impl Fn() -> (P, GraphDir, G)) {
    let (port, root, _guard) = factory();
    let g = dir(&root, "g");
    port.write(&g, GraphFile::Nodes, b"one\n").unwrap();
    let mut first = port.read(&g, GraphFile::Nodes).unwrap().unwrap();
    first.clear();
    assert_eq!(
        port.read(&g, GraphFile::Nodes).unwrap().as_deref(),
        Some(&b"one\n"[..])
    );
}

pub fn gf_c11_resolve_is_stable_and_confined<P: GraphFiles, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let g = dir(&root, "not/yet/made");
    let once = port.resolve(&g).unwrap();
    assert_eq!(
        port.resolve(&once).unwrap(),
        once,
        "resolving twice changes nothing"
    );
    port.write(&g, GraphFile::Graph, b"{}").unwrap();
    assert_eq!(
        port.resolve(&g).unwrap(),
        once,
        "creating the folder does not change its identity"
    );
    assert_eq!(
        port.read(&once, GraphFile::Graph).unwrap().as_deref(),
        Some(&b"{}"[..])
    );
    assert_ne!(port.resolve(&dir(&root, "other")).unwrap(), once);
    let outside = GraphDir::parse("/umx-contract-outside-roots/g").unwrap();
    assert!(matches!(
        port.resolve(&outside),
        Err(FilesError::OutsideRoots(_))
    ));
}

/// `write` returns an error only when nothing changed (ap-13). Once the new bytes are in place it returns `Ok`, and
/// a later step that failed is `Written::NotSynced`, so the caller never undoes a change that happened.
pub fn gf_c13_an_error_means_nothing_changed<P: GraphFiles, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let g = dir(&root, "g");
    port.write(&g, GraphFile::Nodes, b"keep\n").unwrap();
    // A folder cannot be made inside a file, so this write fails before anything is written.
    let through = dir(&root, "g/nodes.jsonl/h");
    assert!(matches!(
        port.write(&through, GraphFile::Graph, b"{}"),
        Err(FilesError::Io(_))
    ));
    assert_eq!(
        port.read(&g, GraphFile::Nodes).unwrap().as_deref(),
        Some(&b"keep\n"[..])
    );
    assert_eq!(port.list(&g).unwrap(), vec![GraphFile::Nodes]);
    // Ok, durable or not, means the new bytes are what a read returns.
    port.write(&g, GraphFile::Nodes, b"new\n").unwrap();
    assert_eq!(
        port.read(&g, GraphFile::Nodes).unwrap().as_deref(),
        Some(&b"new\n"[..])
    );
}

fn put(file: impl Into<FileRef>, bytes: &[u8]) -> Change<'_> {
    Change {
        file: file.into(),
        bytes: Some(bytes),
    }
}

fn drop_file(file: impl Into<FileRef>) -> Change<'static> {
    Change {
        file: file.into(),
        bytes: None,
    }
}

/// The vector shard the set clauses carry beside the graph files, so a node edit and its shard are all or nothing
/// (workshop 008 section 4.1).
fn shard() -> ShardFile {
    ShardFile::new(ModelKey::new("m", 4).expect("a key"), "b7").expect("a shard")
}

/// graph.json "g0", nodes.jsonl "n0", edges.jsonl "e0", the shard "s0": the old state the set clauses change.
fn old_state<P: GraphFiles>(port: &P, g: &GraphDir) {
    port.write(g, GraphFile::Graph, b"g0\n").unwrap();
    port.write(g, GraphFile::Nodes, b"n0\n").unwrap();
    port.write(g, GraphFile::Edges, b"e0\n").unwrap();
    port.write_set(g, &[put(shard(), b"s0\n")]).unwrap();
}

/// Three writes (nodes.jsonl, a shard in its own folder, graph.json) and one remove.
fn new_set() -> [Change<'static>; 4] {
    [
        put(GraphFile::Nodes, b"n1\n"),
        put(shard(), b"s1\n"),
        put(GraphFile::Graph, b"g1\n"),
        drop_file(GraphFile::Edges),
    ]
}

fn read_shard<P: GraphFiles>(port: &P, g: &GraphDir) -> Option<Vec<u8>> {
    port.read_ref(g, &shard().into()).unwrap()
}

fn read<P: GraphFiles>(port: &P, g: &GraphDir, file: GraphFile) -> Option<Vec<u8>> {
    port.read(g, file).unwrap()
}

fn assert_old<P: GraphFiles>(port: &P, g: &GraphDir) {
    assert_eq!(
        read(port, g, GraphFile::Graph).as_deref(),
        Some(&b"g0\n"[..])
    );
    assert_eq!(
        read(port, g, GraphFile::Nodes).as_deref(),
        Some(&b"n0\n"[..])
    );
    assert_eq!(
        read(port, g, GraphFile::Edges).as_deref(),
        Some(&b"e0\n"[..])
    );
    assert_eq!(read_shard(port, g).as_deref(), Some(&b"s0\n"[..]));
}

fn assert_new<P: GraphFiles>(port: &P, g: &GraphDir) {
    assert_eq!(
        read(port, g, GraphFile::Graph).as_deref(),
        Some(&b"g1\n"[..])
    );
    assert_eq!(
        read(port, g, GraphFile::Nodes).as_deref(),
        Some(&b"n1\n"[..])
    );
    assert_eq!(read(port, g, GraphFile::Edges), None);
    assert_eq!(read_shard(port, g).as_deref(), Some(&b"s1\n"[..]));
    assert_eq!(
        port.list(g).unwrap(),
        vec![GraphFile::Graph, GraphFile::Nodes],
        "a shard is not a graph file"
    );
    assert_eq!(
        port.list_shards(g, shard().key(), ShardKind::Vectors)
            .unwrap(),
        vec![shard()]
    );
    assert_eq!(
        port.list_vector_spaces(g).unwrap(),
        vec![shard().key().clone()]
    );
}

fn crashed(r: Result<umx_core::ports::Written, FilesError>) {
    assert_eq!(r, Err(FilesError::io("crash")));
}

pub fn gf_c14_set_applies_every_change<P: SetHarness, G>(factory: impl Fn() -> (P, GraphDir, G)) {
    let (port, root, _guard) = factory();
    let g = dir(&root, "g");
    old_state(&port, &g);
    port.write_set(&g, &new_set()).unwrap();
    assert_new(&port, &g);
    assert!(port.leftovers(&g).is_empty(), "{:?}", port.leftovers(&g));
    // Removing a file that is already absent is fine.
    port.write_set(&g, &[drop_file(GraphFile::Edges)]).unwrap();
    assert_new(&port, &g);
    // A set also makes a missing folder, like `write`.
    let fresh = dir(&root, "fresh/g");
    port.write_set(
        &fresh,
        &[put(GraphFile::Graph, b"{}"), put(GraphFile::Nodes, b"")],
    )
    .unwrap();
    assert_eq!(
        port.list(&fresh).unwrap(),
        vec![GraphFile::Graph, GraphFile::Nodes]
    );
}

pub fn gf_c15_set_error_changes_nothing<P: SetHarness, G>(factory: impl Fn() -> (P, GraphDir, G)) {
    let (port, root, _guard) = factory();
    let g = dir(&root, "g");
    old_state(&port, &g);
    // The second change's temp cannot be written (a full disk).
    port.fail_temp_of(Some(GraphFile::Graph));
    assert!(matches!(
        port.write_set(&g, &new_set()),
        Err(FilesError::Io(_))
    ));
    port.fail_temp_of(None);
    assert_old(&port, &g);
    assert!(port.leftovers(&g).is_empty(), "{:?}", port.leftovers(&g));
    // The same port still works.
    port.write_set(&g, &new_set()).unwrap();
    assert_new(&port, &g);
}

pub fn gf_c16_crash_before_commit_keeps_the_old_state<P: SetHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let g = dir(&root, "g");
    old_state(&port, &g);
    port.crash_at(Some(CrashPoint::AfterTemps));
    crashed(port.write_set(&g, &new_set()));
    assert!(
        !port.leftovers(&g).is_empty(),
        "the crash leaves its temp files"
    );
    let after = port.restart();
    assert_old(&after, &g);
    assert!(after.leftovers(&g).is_empty(), "{:?}", after.leftovers(&g));
}

pub fn gf_c17_crash_after_commit_rolls_forward<P: SetHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let g = dir(&root, "g");
    old_state(&port, &g);
    port.crash_at(Some(CrashPoint::AfterMarker));
    crashed(port.write_set(&g, &new_set()));
    assert_eq!(
        port.raw(&g, GraphFile::Nodes).as_deref(),
        Some(&b"n0\n"[..])
    );
    let after = port.restart();
    // Recovery runs before a list too, not only before a read or a write.
    assert_eq!(
        after.list(&g).unwrap(),
        vec![GraphFile::Graph, GraphFile::Nodes]
    );
    assert_new(&after, &g);
    assert!(after.leftovers(&g).is_empty(), "{:?}", after.leftovers(&g));
}

/// `resolve` is read only: after a crash past the marker it names the folder and moves nothing, so `umx doctor`
/// can check a graph's folder without finishing (or touching) an unfinished set. The next read still recovers.
pub fn gf_c25_resolve_never_recovers<P: SetHarness, G>(factory: impl Fn() -> (P, GraphDir, G)) {
    let (port, root, _guard) = factory();
    let g = dir(&root, "g");
    old_state(&port, &g);
    port.crash_at(Some(CrashPoint::AfterMarker));
    crashed(port.write_set(&g, &new_set()));
    let after = port.restart();
    let leftovers = after.leftovers(&g);
    assert!(
        !leftovers.is_empty(),
        "the crash left a marker and temp files"
    );
    after.resolve(&g).expect("resolve");
    assert_eq!(
        after.leftovers(&g),
        leftovers,
        "resolve moved the marker or a temp file"
    );
    assert_eq!(
        after.raw(&g, GraphFile::Nodes).as_deref(),
        Some(&b"n0\n"[..]),
        "resolve rolled the set forward"
    );
    assert_new(&after, &g);
}

pub fn gf_c18_crash_mid_renames_rolls_forward<P: SetHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let g = dir(&root, "g");
    old_state(&port, &g);
    port.crash_at(Some(CrashPoint::AfterFirstRename));
    crashed(port.write_set(&g, &new_set()));
    assert_eq!(
        port.raw(&g, GraphFile::Nodes).as_deref(),
        Some(&b"n1\n"[..])
    );
    assert_eq!(
        port.raw(&g, GraphFile::Graph).as_deref(),
        Some(&b"g0\n"[..])
    );
    let after = port.restart();
    assert_new(&after, &g);
    assert!(after.leftovers(&g).is_empty(), "{:?}", after.leftovers(&g));
}

pub fn gf_c19_changed_target_is_a_conflict<P: SetHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let g = dir(&root, "g");
    old_state(&port, &g);
    port.crash_at(Some(CrashPoint::AfterMarker));
    crashed(port.write_set(&g, &new_set()));
    let kept = port.leftovers(&g);
    port.tamper(&g, GraphFile::Graph, b"pulled\n");
    let after = port.restart();
    let conflict = FilesError::RecoveryConflict(vec![GraphFile::Graph.into()]);
    assert_eq!(after.read(&g, GraphFile::Nodes).unwrap_err(), conflict);
    assert_eq!(after.list(&g).unwrap_err(), conflict);
    assert_eq!(
        after.write(&g, GraphFile::Nodes, b"n2\n").unwrap_err(),
        conflict
    );
    assert_eq!(after.write_set(&g, &new_set()).unwrap_err(), conflict);
    assert_eq!(
        after
            .write_set(&g, &[drop_file(GraphFile::Edges)])
            .unwrap_err(),
        conflict
    );
    // Nothing moved: the tampered file, the other targets, the temp files and the marker.
    assert_eq!(
        after.raw(&g, GraphFile::Graph).as_deref(),
        Some(&b"pulled\n"[..])
    );
    assert_eq!(
        after.raw(&g, GraphFile::Nodes).as_deref(),
        Some(&b"n0\n"[..])
    );
    assert_eq!(
        after.raw(&g, GraphFile::Edges).as_deref(),
        Some(&b"e0\n"[..])
    );
    assert_eq!(after.leftovers(&g), kept);
    // Another folder is not affected.
    let other = dir(&root, "other");
    after.write(&other, GraphFile::Graph, b"{}").unwrap();
}

pub fn gf_c20_set_never_leaves_the_roots<P: SetHarness, G>(factory: impl Fn() -> (P, GraphDir, G)) {
    let (port, root, _guard) = factory();
    let sibling = GraphDir::parse(&format!("{}-sibling/g", root.as_str())).unwrap();
    let elsewhere = GraphDir::parse("/umx-contract-outside-roots/g").unwrap();
    for outside in [sibling, elsewhere] {
        port.crash_at(Some(CrashPoint::AfterMarker));
        assert!(
            matches!(
                port.write_set(&outside, &new_set()),
                Err(FilesError::OutsideRoots(_))
            ),
            "{outside}"
        );
        port.crash_at(None);
    }
    // The refused set left nothing anywhere the port can see.
    assert!(port.leftovers(&root).is_empty());
}

pub fn gf_c21_invalid_set_is_refused<P: SetHarness, G>(factory: impl Fn() -> (P, GraphDir, G)) {
    let (port, root, _guard) = factory();
    let g = dir(&root, "g");
    old_state(&port, &g);
    port.write(&g, GraphFile::GitIgnore, b"keep\n").unwrap();
    let twice = [
        put(GraphFile::Nodes, b"a"),
        put(GraphFile::Graph, b"b"),
        drop_file(GraphFile::Nodes),
    ];
    for set in [&[][..], &twice[..], &[drop_file(GraphFile::GitIgnore)][..]] {
        assert!(
            matches!(port.write_set(&g, set), Err(FilesError::Invalid(_))),
            "{set:?}"
        );
    }
    assert_old(&port, &g);
    assert_eq!(
        read(&port, &g, GraphFile::GitIgnore).as_deref(),
        Some(&b"keep\n"[..])
    );
    assert!(port.leftovers(&g).is_empty(), "{:?}", port.leftovers(&g));
}

pub fn gf_c22_one_file_write_is_a_set<P: SetHarness, G>(factory: impl Fn() -> (P, GraphDir, G)) {
    let (port, root, _guard) = factory();
    let g = dir(&root, "g");
    old_state(&port, &g);
    port.write_set(&g, &[put(GraphFile::Nodes, b"n1\n")])
        .unwrap();
    assert_eq!(
        read(&port, &g, GraphFile::Nodes).as_deref(),
        Some(&b"n1\n"[..])
    );
    // A one-file write crashed before its change stands leaves the old bytes and, after a restart, no temp file.
    port.crash_at(Some(CrashPoint::AfterTemps));
    crashed(port.write(&g, GraphFile::Nodes, b"n2\n"));
    let after = port.restart();
    assert_eq!(
        read(&after, &g, GraphFile::Nodes).as_deref(),
        Some(&b"n1\n"[..])
    );
    assert!(after.leftovers(&g).is_empty(), "{:?}", after.leftovers(&g));
    // `.gitignore` is not a graph file: list never shows it.
    after
        .write(&g, GraphFile::GitIgnore, b".umx-tmp-*\n")
        .unwrap();
    assert_eq!(after.list(&g).unwrap(), GraphFile::ALL.to_vec());
}

/// On one instance, no restart: a set that stopped after its point of no return (here a crash after its first
/// rename; in a running daemon, a NotSynced set whose rename failed) is finished by the next call before that call
/// writes anything, so a second set never writes over the first one's marker (review R54-1).
pub fn gf_c23_a_set_after_an_unfinished_set_finishes_it_first<P: SetHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let g = dir(&root, "g");
    old_state(&port, &g);
    port.crash_at(Some(CrashPoint::AfterFirstRename));
    crashed(port.write_set(&g, &new_set()));
    // The second set touches graph.json only; nodes.jsonl and edges.jsonl must still end as the first set left them.
    port.write_set(&g, &[put(GraphFile::Graph, b"g2\n")])
        .unwrap();
    assert_eq!(
        read(&port, &g, GraphFile::Graph).as_deref(),
        Some(&b"g2\n"[..])
    );
    assert_eq!(
        read(&port, &g, GraphFile::Nodes).as_deref(),
        Some(&b"n1\n"[..])
    );
    assert_eq!(read(&port, &g, GraphFile::Edges), None);
    assert!(port.leftovers(&g).is_empty(), "{:?}", port.leftovers(&g));
    // The same with a two-file second set, which writes a marker of its own.
    let h = dir(&root, "h");
    old_state(&port, &h);
    port.crash_at(Some(CrashPoint::AfterMarker));
    crashed(port.write_set(&h, &new_set()));
    port.write_set(
        &h,
        &[
            put(GraphFile::Graph, b"g2\n"),
            put(GraphFile::Edges, b"e2\n"),
        ],
    )
    .unwrap();
    assert_eq!(
        read(&port, &h, GraphFile::Graph).as_deref(),
        Some(&b"g2\n"[..])
    );
    assert_eq!(
        read(&port, &h, GraphFile::Nodes).as_deref(),
        Some(&b"n1\n"[..])
    );
    assert_eq!(
        read(&port, &h, GraphFile::Edges).as_deref(),
        Some(&b"e2\n"[..])
    );
    assert!(port.leftovers(&h).is_empty(), "{:?}", port.leftovers(&h));
}

/// Sets on one folder from several threads run one at a time: the two files of every set land together, so the
/// folder always ends with both files from the same set, and nothing is left behind. Small, because the disk is
/// shared.
pub fn gf_c24_concurrent_sets_serialize<P: SetHarness, G>(factory: impl Fn() -> (P, GraphDir, G)) {
    let (port, root, _guard) = factory();
    let g = dir(&root, "g");
    old_state(&port, &g);
    std::thread::scope(|scope| {
        for t in 0..4u8 {
            let (port, g) = (&port, &g);
            scope.spawn(move || {
                for round in 0..3u8 {
                    let bytes = [b's', b'0' + t, b'0' + round, b'\n'];
                    port.write_set(
                        g,
                        &[put(GraphFile::Graph, &bytes), put(GraphFile::Nodes, &bytes)],
                    )
                    .unwrap();
                }
            });
        }
    });
    let graph = read(&port, &g, GraphFile::Graph).unwrap();
    assert_eq!(
        read(&port, &g, GraphFile::Nodes),
        Some(graph.clone()),
        "mixed sets"
    );
    assert_eq!(graph.len(), 4);
    assert!(port.leftovers(&g).is_empty(), "{:?}", port.leftovers(&g));
}

/// One `#[test]` per GraphFiles clause, named in the call so r-fake-per-port finds each name in the caller's source,
/// plus a test that fails if the list is not exactly `CLAUSES`. Usage: `graph_files_contract!(factory; <prefix>_c1_<what>(), ...)`, each clause written as a call so umx-arch's r-fake-per-port token scan sees it.
#[macro_export]
macro_rules! graph_files_contract {
    ($factory:expr; $($clause:ident()),+ $(,)?) => {
        mod graph_files_contract {
            #[allow(unused_imports)]
            use super::*;
            $(
                #[test]
                fn $clause() {
                    $crate::graph_files::$clause($factory)
                }
            )+
            #[test]
            fn every_clause_is_listed() {
                $crate::assert_listed($crate::graph_files::CLAUSES, &[$(stringify!($clause)),+]);
            }
        }
    };
}

/// A file over its kind's cap (put there behind the port's back, as by a pull) is `TooLarge`, naming the file, its
/// size and the cap, and not one byte of its body is read. A file of exactly the cap reads whole, which also shows the
/// byte count works.
pub fn gf_c26_an_over_cap_file_is_refused_unread<P: SetHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let g = dir(&root, "g");
    port.write(&g, GraphFile::Nodes, b"").unwrap();
    for file in [GraphFile::Graph, GraphFile::GitIgnore] {
        let cap = FileRef::from(file).cap();
        port.tamper(&g, file, &vec![b' '; cap as usize]);
        let before = port.bytes_read();
        assert_eq!(read(&port, &g, file).map(|b| b.len() as u64), Some(cap));
        assert_eq!(
            port.bytes_read() - before,
            cap,
            "{file:?}: the counter counts"
        );
        port.tamper(&g, file, &vec![b' '; cap as usize + 1]);
        let before = port.bytes_read();
        assert_eq!(
            port.read(&g, file),
            Err(FilesError::TooLarge {
                file: file.into(),
                size: cap + 1,
                cap
            })
        );
        assert_eq!(
            port.bytes_read(),
            before,
            "{file:?}: an over-cap body was read"
        );
    }
}

/// A set holding a change over its file's cap is `TooLarge` and changes nothing: umx never writes a file it would
/// refuse to read. A change of exactly the cap is written.
pub fn gf_c27_a_change_over_cap_is_refused_unwritten<P: SetHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let g = dir(&root, "g");
    old_state(&port, &g);
    let shard_cap = FileRef::from(shard()).cap() as usize;
    let graph_cap = FileRef::from(GraphFile::Graph).cap() as usize;
    let meta = FileRef::SimilarMeta(shard().key().clone());
    let meta_cap = meta.cap() as usize;
    let big_shard = vec![b's'; shard_cap + 1];
    let big_graph = vec![b'g'; graph_cap + 1];
    let big_meta = vec![b'm'; meta_cap + 1];
    for (over, size, cap) in [
        (put(shard(), &big_shard), shard_cap + 1, shard_cap),
        (put(GraphFile::Graph, &big_graph), graph_cap + 1, graph_cap),
        (
            Change {
                file: meta.clone(),
                bytes: Some(&big_meta),
            },
            meta_cap + 1,
            meta_cap,
        ),
    ] {
        let file = over.file.clone();
        let set = [put(GraphFile::Nodes, b"n1\n"), over];
        assert_eq!(
            port.write_set(&g, &set),
            Err(FilesError::TooLarge {
                file,
                size: size as u64,
                cap: cap as u64
            })
        );
        assert_old(&port, &g);
        assert!(port.leftovers(&g).is_empty(), "{:?}", port.leftovers(&g));
    }
    port.write_set(&g, &[put(GraphFile::Graph, &big_graph[..graph_cap])])
        .unwrap();
    assert_eq!(
        read(&port, &g, GraphFile::Graph).map(|b| b.len()),
        Some(graph_cap)
    );
}

/// No path reads an over-cap body (review F1). Recovery: a target that grew over its cap after a crash matches
/// no stamp, so it is a conflict, found without reading it. A set of several changes stamps the files it replaces:
/// one over its cap is `TooLarge` and nothing changes.
pub fn gf_c28_recovery_and_old_stamps_never_read_an_over_cap_body<P: SetHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let cap = FileRef::from(GraphFile::Graph).cap();
    let big = vec![b' '; cap as usize + 1];

    let g = dir(&root, "g");
    old_state(&port, &g);
    port.tamper(&g, GraphFile::Graph, &big);
    let before = port.bytes_read();
    assert_eq!(
        port.write_set(
            &g,
            &[
                put(GraphFile::Graph, b"g1\n"),
                put(GraphFile::Nodes, b"n1\n")
            ]
        ),
        Err(FilesError::TooLarge {
            file: GraphFile::Graph.into(),
            size: cap + 1,
            cap
        })
    );
    assert!(
        port.bytes_read() - before < cap,
        "a set read an over-cap old file"
    );
    assert_eq!(port.raw(&g, GraphFile::Graph), Some(big.clone()));
    assert_eq!(
        read(&port, &g, GraphFile::Nodes).as_deref(),
        Some(&b"n0\n"[..])
    );
    assert!(port.leftovers(&g).is_empty(), "{:?}", port.leftovers(&g));

    let h = dir(&root, "h");
    old_state(&port, &h);
    port.crash_at(Some(CrashPoint::AfterMarker));
    crashed(port.write_set(&h, &new_set()));
    port.tamper(&h, GraphFile::Graph, &big);
    let after = port.restart();
    let before = after.bytes_read();
    assert_eq!(
        after.read(&h, GraphFile::Nodes),
        Err(FilesError::RecoveryConflict(vec![GraphFile::Graph.into()]))
    );
    assert!(
        after.bytes_read() - before < cap,
        "recovery read an over-cap body"
    );
    assert_eq!(after.raw(&h, GraphFile::Graph), Some(big));
}

/// similar/ files (ws004 s09): a similarity shard and its folder's `_meta.tsv` write and read back like any file, in
/// one set with a vector shard; `list_shards` lists one kind's shards only (never `_meta.tsv`), and
/// `list_vector_spaces` lists only `embeddings/` folders.
pub fn gf_c29_similar_files_round_trip_and_list_by_kind<P: GraphFiles, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let g = dir(&root, "g");
    let key = shard().key().clone();
    let similar = ShardFile::of_kind(ShardKind::Similar, key.clone(), "b7").unwrap();
    let other = ModelKey::new("other", 4).unwrap();
    let other_similar = ShardFile::of_kind(ShardKind::Similar, other, "c1").unwrap();
    let meta = FileRef::SimilarMeta(key.clone());
    port.write_set(
        &g,
        &[
            put(shard(), b"s\n"),
            put(similar.clone(), b"l\n"),
            Change {
                file: meta.clone(),
                bytes: Some(b"m\n"),
            },
            put(other_similar, b"o\n"),
        ],
    )
    .unwrap();
    assert_eq!(
        port.read_ref(&g, &FileRef::Shard(similar.clone()))
            .unwrap()
            .as_deref(),
        Some(&b"l\n"[..])
    );
    assert_eq!(
        port.read_ref(&g, &meta).unwrap().as_deref(),
        Some(&b"m\n"[..])
    );
    assert_eq!(read_shard(&port, &g).as_deref(), Some(&b"s\n"[..]));
    assert_eq!(
        port.list_shards(&g, &key, ShardKind::Similar).unwrap(),
        vec![similar]
    );
    assert_eq!(
        port.list_shards(&g, &key, ShardKind::Vectors).unwrap(),
        vec![shard()]
    );
    assert_eq!(
        port.list_vector_spaces(&g).unwrap(),
        vec![key],
        "a similar/ folder is not a vector space"
    );
}

/// The three content files' stamps as a load sees them: each from `read_stamped`, as the services load a graph.
fn loaded_stamps<P: GraphFiles>(port: &P, g: &GraphDir) -> GraphStamps {
    let mut out = GraphStamps::ABSENT;
    for file in GraphStamps::FILES {
        out.set(file, port.read_stamped(g, file).unwrap().1);
    }
    out
}

/// F144: the stamps a set returns are the stamps a fresh look sees, for the files it wrote, removed and left alone,
/// with a check or without one (`None` writes and still returns them). Absent is a stamp. `read_stamped` of a file
/// nobody changed agrees.
pub fn gf_c30_a_set_returns_the_stamps_a_fresh_look_sees<P: SetHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let g = dir(&root, "g");
    assert_eq!(
        port.stamps(&g).unwrap(),
        GraphStamps::ABSENT,
        "no folder yet"
    );
    let first = port
        .write_set_checked(
            &g,
            &[
                put(GraphFile::Graph, b"g0\n"),
                put(GraphFile::Nodes, b"n0\n"),
            ],
            None,
        )
        .unwrap();
    assert_eq!(first.stamps, port.stamps(&g).unwrap());
    assert_eq!(first.stamps, loaded_stamps(&port, &g));
    assert_eq!(first.stamps.edges, FileStamp::ABSENT);
    assert_ne!(first.stamps.graph, FileStamp::ABSENT);
    assert_ne!(first.stamps.graph, first.stamps.nodes);
    // One file (the one-rename path): the files it left alone keep their stamps.
    let one = port
        .write_set_checked(&g, &[put(GraphFile::Edges, b"e0\n")], Some(&first.stamps))
        .unwrap();
    assert_eq!(one.stamps, port.stamps(&g).unwrap());
    assert_eq!(
        (one.stamps.graph, one.stamps.nodes),
        (first.stamps.graph, first.stamps.nodes)
    );
    assert_ne!(one.stamps.edges, FileStamp::ABSENT);
    // Several, with a shard and a removal.
    let set = port
        .write_set_checked(
            &g,
            &[
                put(GraphFile::Nodes, b"n1\n"),
                put(shard(), b"s1\n"),
                drop_file(GraphFile::Edges),
            ],
            Some(&one.stamps),
        )
        .unwrap();
    assert_eq!(set.stamps, port.stamps(&g).unwrap());
    assert_eq!(set.stamps, loaded_stamps(&port, &g));
    assert_eq!(set.stamps.edges, FileStamp::ABSENT);
    assert_eq!(set.stamps.graph, one.stamps.graph);
    assert_ne!(set.stamps.nodes, one.stamps.nodes);
    // Neither a shard nor .gitignore is stamped: writing them moves no stamp.
    let other = port
        .write_set_checked(
            &g,
            &[put(shard(), b"s2\n"), put(GraphFile::GitIgnore, b"x\n")],
            Some(&set.stamps),
        )
        .unwrap();
    assert_eq!(other.stamps, set.stamps);
    assert_eq!(port.stamps(&g).unwrap(), set.stamps);
}

/// F144: a content file changed since the stamps were taken (other bytes, only a touch, a symlink swapped in, or
/// a file where there was none) refuses a checked set with `Changed` naming it, even when the set does not write
/// that file: nothing is written and no temp file is left. A set without a check is not refused.
pub fn gf_c31_a_change_since_the_stamps_refuses_the_set<P: SetHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    type Outside<P> = fn(&P, &GraphDir, GraphFile);
    let outside: [(&str, Outside<P>); 3] = [
        ("other bytes", |p, g, f| p.tamper(g, f, b"pulled\n")),
        ("a touch", |p, g, f| p.touch(g, f)),
        ("a symlink", |p, g, f| p.swap_for_link(g, f)),
    ];
    let one: &[Change<'_>] = &[put(GraphFile::Nodes, b"n1\n")];
    let several: &[Change<'_>] = &[
        put(GraphFile::Nodes, b"n1\n"),
        put(GraphFile::Graph, b"g1\n"),
        put(shard(), b"s1\n"),
    ];
    for (i, (what, change)) in outside.iter().enumerate() {
        for (j, set) in [one, several].into_iter().enumerate() {
            let g = dir(&root, &format!("g{i}-{j}"));
            old_state(&port, &g);
            let stamps = loaded_stamps(&port, &g);
            change(&port, &g, GraphFile::Edges);
            let disk = GraphStamps::FILES.map(|f| port.raw(&g, f));
            assert_eq!(
                port.write_set_checked(&g, set, Some(&stamps)),
                Err(FilesError::Changed(vec![GraphFile::Edges.into()])),
                "{what}, {} change(s)",
                set.len()
            );
            assert_eq!(GraphStamps::FILES.map(|f| port.raw(&g, f)), disk, "{what}");
            assert_eq!(read_shard(&port, &g).as_deref(), Some(&b"s0\n"[..]));
            assert!(port.leftovers(&g).is_empty(), "{:?}", port.leftovers(&g));
            port.write_set_checked(&g, set, None).unwrap();
            assert_eq!(
                read(&port, &g, GraphFile::Nodes).as_deref(),
                Some(&b"n1\n"[..])
            );
        }
    }
    let g = dir(&root, "appeared");
    port.write(&g, GraphFile::Graph, b"g0\n").unwrap();
    let stamps = loaded_stamps(&port, &g);
    port.tamper(&g, GraphFile::Edges, b"e\n");
    assert_eq!(
        port.write_set_checked(&g, one, Some(&stamps)),
        Err(FilesError::Changed(vec![GraphFile::Edges.into()])),
        "a file where there was none"
    );
    assert_eq!(read(&port, &g, GraphFile::Nodes), None);
}

/// F144: a set checks after its temps and marker are in place and before the first rename, so a change landing
/// after the marker still refuses the set: nothing is renamed, the change stays, and neither a temp nor the marker
/// is left for recovery to finish. And a change to one of its targets landing before the marker takes its old
/// stamps, with a crash once the marker is published, is never rolled forward over (review R1): the set is refused
/// before it publishes the marker, or recovery reports a conflict.
pub fn gf_c33_a_change_after_the_marker_refuses_the_set<P: SetHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let several: &[Change<'_>] = &[
        put(GraphFile::Nodes, b"n1\n"),
        put(GraphFile::Graph, b"g1\n"),
        put(shard(), b"s1\n"),
    ];
    let g = dir(&root, "late");
    old_state(&port, &g);
    let stamps = loaded_stamps(&port, &g);
    port.change_after_next_marker(&g, GraphFile::Edges, b"checked out after the marker\n");
    let before = [GraphFile::Graph, GraphFile::Nodes].map(|f| port.raw(&g, f));
    assert_eq!(
        port.write_set_checked(&g, several, Some(&stamps)),
        Err(FilesError::Changed(vec![GraphFile::Edges.into()]))
    );
    assert!(port.leftovers(&g).is_empty(), "{:?}", port.leftovers(&g));
    let restarted = port.restart();
    assert_eq!(
        [GraphFile::Graph, GraphFile::Nodes].map(|f| restarted.raw(&g, f)),
        before
    );
    assert_eq!(
        read(&restarted, &g, GraphFile::Edges).as_deref(),
        Some(&b"checked out after the marker\n"[..])
    );
    assert_eq!(read_shard(&restarted, &g).as_deref(), Some(&b"s0\n"[..]));
    // Control: with nothing landing, the same set from fresh stamps goes through.
    let stamps = loaded_stamps(&restarted, &g);
    restarted
        .write_set_checked(&g, several, Some(&stamps))
        .unwrap();
    assert_eq!(
        read(&restarted, &g, GraphFile::Nodes).as_deref(),
        Some(&b"n1\n"[..])
    );
    // A target changed while the temps are written, before the marker's old stamps; a crash after publication.
    let port = restarted.restart();
    let g = dir(&root, "early");
    old_state(&port, &g);
    let stamps = loaded_stamps(&port, &g);
    let checked_out = b"checked out before the marker\n";
    port.change_before_next_marker(&g, GraphFile::Nodes, checked_out);
    port.crash_at(Some(CrashPoint::AfterMarker));
    assert!(port.write_set_checked(&g, several, Some(&stamps)).is_err());
    port.crash_at(None);
    let restarted = port.restart();
    match restarted.read(&g, GraphFile::Nodes) {
        Err(FilesError::RecoveryConflict(_)) => {}
        got => assert_eq!(got, Ok(Some(checked_out.to_vec())), "rolled forward over"),
    }
}

/// F144: `read_stamped` stamps the file its bytes came from (one open), so a write that lands while it reads shows
/// as changed at the next check rather than being taken into the stamp. An absent file is stamped absent.
pub fn gf_c32_read_stamped_stamps_the_bytes_it_read<P: SetHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let g = dir(&root, "g");
    port.write(&g, GraphFile::Graph, b"g0\n").unwrap();
    port.write(&g, GraphFile::Nodes, b"n0\n").unwrap();
    let mut stamps = loaded_stamps(&port, &g);
    port.change_during_next_read(&g, GraphFile::Nodes, b"a longer line, written mid-read\n");
    let (bytes, stamp) = port.read_stamped(&g, GraphFile::Nodes).unwrap();
    assert!(bytes.is_some());
    stamps.nodes = stamp;
    assert_eq!(
        port.write_set_checked(&g, &[put(GraphFile::Graph, b"g1\n")], Some(&stamps)),
        Err(FilesError::Changed(vec![GraphFile::Nodes.into()]))
    );
    assert_eq!(
        port.raw(&g, GraphFile::Graph).as_deref(),
        Some(&b"g0\n"[..])
    );
    assert_eq!(
        port.read_stamped(&g, GraphFile::Edges).unwrap(),
        (None, FileStamp::ABSENT)
    );
}

/// ws003 s09: `receipts.jsonl` is a file the port reads and writes in sets, at the folder's top, under its own cap, and
/// never a graph file: `list` leaves it out. A stamped read of it is capped as any read is: over the cap is
/// `TooLarge` with the body unread, and absent is stamped absent.
pub fn gf_c34_receipts_round_trip_capped_and_are_no_graph_file<P: SetHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let g = dir(&root, "g");
    assert_eq!(
        port.read_ref_stamped(&g, &FileRef::Receipts).unwrap(),
        (None, FileStamp::ABSENT)
    );
    port.write(&g, GraphFile::Graph, b"g0\n").unwrap();
    port.write_set(&g, &[put(FileRef::Receipts, b"r0\n")])
        .unwrap();
    assert_eq!(
        port.read_ref(&g, &FileRef::Receipts).unwrap().as_deref(),
        Some(&b"r0\n"[..])
    );
    let (bytes, stamp) = port.read_ref_stamped(&g, &FileRef::Receipts).unwrap();
    assert_eq!(bytes.as_deref(), Some(&b"r0\n"[..]));
    assert_ne!(stamp, FileStamp::ABSENT);
    assert_eq!(
        port.raw(&g, FileRef::Receipts).as_deref(),
        Some(&b"r0\n"[..])
    );
    assert_eq!(
        port.list(&g).unwrap(),
        vec![GraphFile::Graph],
        "not a graph file"
    );
    let cap = FileRef::Receipts.cap();
    port.tamper(&g, FileRef::Receipts, &vec![b' '; cap as usize + 1]);
    let before = port.bytes_read();
    for read in [
        port.read_ref(&g, &FileRef::Receipts).map(|_| ()),
        port.read_ref_stamped(&g, &FileRef::Receipts).map(|_| ()),
    ] {
        assert_eq!(
            read,
            Err(FilesError::TooLarge {
                file: FileRef::Receipts,
                size: cap + 1,
                cap
            })
        );
    }
    assert_eq!(port.bytes_read(), before, "an over-cap body is never read");
}

/// ws003 s09: a set of content files and `receipts.jsonl` is all or nothing at every crash point, so a keyed batch's
/// receipt never stands without its write, or its write without its receipt.
pub fn gf_c35_a_set_with_receipts_is_all_or_nothing_across_crashes<P: SetHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    for (i, point) in [
        CrashPoint::AfterTemps,
        CrashPoint::AfterMarker,
        CrashPoint::AfterFirstRename,
    ]
    .into_iter()
    .enumerate()
    {
        let g = dir(&root, &format!("g{i}"));
        old_state(&port, &g);
        port.write_set(&g, &[put(FileRef::Receipts, b"r0\n")])
            .unwrap();
        let stamps = loaded_stamps(&port, &g);
        let (_, seen) = port.read_ref_stamped(&g, &FileRef::Receipts).unwrap();
        port.crash_at(Some(point));
        let set = [
            put(GraphFile::Nodes, b"n1\n"),
            put(GraphFile::Edges, b"e1\n"),
            put(FileRef::Receipts, b"r1\n"),
        ];
        assert_eq!(
            port.write_set_checked_also(&g, &set, Some(&stamps), &[(FileRef::Receipts, seen)]),
            Err(FilesError::io("crash"))
        );
        let after = port.restart();
        let got = (
            read(&after, &g, GraphFile::Nodes),
            read(&after, &g, GraphFile::Edges),
            after.read_ref(&g, &FileRef::Receipts).unwrap(),
        );
        let old = (
            Some(b"n0\n".to_vec()),
            Some(b"e0\n".to_vec()),
            Some(b"r0\n".to_vec()),
        );
        let new = (
            Some(b"n1\n".to_vec()),
            Some(b"e1\n".to_vec()),
            Some(b"r1\n".to_vec()),
        );
        match point {
            CrashPoint::AfterTemps => assert_eq!(got, old, "{point:?}"),
            _ => assert_eq!(got, new, "{point:?}"),
        }
        assert!(after.leftovers(&g).is_empty(), "{:?}", after.leftovers(&g));
    }
}

/// ws003 s09 (graph-storage Q4): a keyed batch that changed nothing writes `receipts.jsonl` alone, checked against
/// the content stamps and its own; it writes, and the content stamps it returns are unchanged. A crash before its one
/// rename leaves the old receipts and no temp after recovery; a crash after it leaves the new.
pub fn gf_c36_a_receipts_only_set_writes_and_keeps_the_content_stamps<P: SetHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let g = dir(&root, "g");
    old_state(&port, &g);
    let stamps = loaded_stamps(&port, &g);
    let (bytes, absent) = port.read_ref_stamped(&g, &FileRef::Receipts).unwrap();
    assert_eq!((bytes, absent), (None, FileStamp::ABSENT));
    let committed = port
        .write_set_checked_also(
            &g,
            &[put(FileRef::Receipts, b"r1\n")],
            Some(&stamps),
            &[(FileRef::Receipts, absent)],
        )
        .unwrap();
    assert_eq!(committed.stamps, stamps, "only the receipts changed");
    assert_old(&port, &g);
    assert_eq!(
        port.read_ref(&g, &FileRef::Receipts).unwrap().as_deref(),
        Some(&b"r1\n"[..])
    );
    for (i, point) in [CrashPoint::AfterTemps, CrashPoint::AfterFirstRename]
        .into_iter()
        .enumerate()
    {
        let (_, seen) = port.read_ref_stamped(&g, &FileRef::Receipts).unwrap();
        let bytes = format!("r{}\n", i + 2);
        port.crash_at(Some(point));
        assert!(
            port.write_set_checked_also(
                &g,
                &[put(FileRef::Receipts, bytes.as_bytes())],
                Some(&stamps),
                &[(FileRef::Receipts, seen)],
            )
            .is_err()
        );
        port.crash_at(None);
        let after = port.restart();
        let got = after.read_ref(&g, &FileRef::Receipts).unwrap();
        match point {
            CrashPoint::AfterTemps => assert_eq!(got.as_deref(), Some(&b"r1\n"[..])),
            _ => assert_eq!(got.as_deref(), Some(bytes.as_bytes())),
        }
        assert!(after.leftovers(&g).is_empty(), "{:?}", after.leftovers(&g));
        assert_old(&after, &g);
    }
}

/// ws003 s09 (architect A3, A5): each `also` file is checked against its stamp at the points the content files are,
/// in the same pass. A changed one refuses the set with `Changed` naming it, after any changed content file, with
/// nothing written and no temp or marker left; an unchanged one writes. One change or several, a change before the
/// set or after its marker, alike.
pub fn gf_c37_a_changed_also_file_refuses_the_set<P: SetHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let one: &[Change<'_>] = &[put(FileRef::Receipts, b"r1\n")];
    let several: &[Change<'_>] = &[
        put(GraphFile::Nodes, b"n1\n"),
        put(FileRef::Receipts, b"r1\n"),
    ];
    for (j, set) in [one, several].into_iter().enumerate() {
        // Changed after the read: refused, nothing written.
        let g = dir(&root, &format!("before{j}"));
        old_state(&port, &g);
        port.write_set(&g, &[put(FileRef::Receipts, b"r0\n")])
            .unwrap();
        let stamps = loaded_stamps(&port, &g);
        let (_, seen) = port.read_ref_stamped(&g, &FileRef::Receipts).unwrap();
        port.tamper(&g, FileRef::Receipts, b"pulled\n");
        assert_eq!(
            port.write_set_checked_also(&g, set, Some(&stamps), &[(FileRef::Receipts, seen)]),
            Err(FilesError::Changed(vec![FileRef::Receipts])),
            "{} change(s)",
            set.len()
        );
        assert_eq!(
            port.raw(&g, FileRef::Receipts).as_deref(),
            Some(&b"pulled\n"[..])
        );
        assert_old(&port, &g);
        assert!(port.leftovers(&g).is_empty(), "{:?}", port.leftovers(&g));
        // Unchanged since a fresh read: writes.
        let (_, seen) = port.read_ref_stamped(&g, &FileRef::Receipts).unwrap();
        port.write_set_checked_also(&g, set, Some(&stamps), &[(FileRef::Receipts, seen)])
            .unwrap();
        assert_eq!(
            port.raw(&g, FileRef::Receipts).as_deref(),
            Some(&b"r1\n"[..])
        );
        // A content file and the also file both changed: both named, content first.
        let g = dir(&root, &format!("both{j}"));
        old_state(&port, &g);
        let stamps = loaded_stamps(&port, &g);
        let (_, seen) = port.read_ref_stamped(&g, &FileRef::Receipts).unwrap();
        port.tamper(&g, GraphFile::Edges, b"pulled\n");
        port.tamper(&g, FileRef::Receipts, b"pulled\n");
        assert_eq!(
            port.write_set_checked_also(&g, set, Some(&stamps), &[(FileRef::Receipts, seen)]),
            Err(FilesError::Changed(vec![
                GraphFile::Edges.into(),
                FileRef::Receipts
            ]))
        );
        assert_eq!(
            read(&port, &g, GraphFile::Nodes).as_deref(),
            Some(&b"n0\n"[..])
        );
        assert_eq!(
            port.raw(&g, FileRef::Receipts).as_deref(),
            Some(&b"pulled\n"[..])
        );
        assert!(port.leftovers(&g).is_empty(), "{:?}", port.leftovers(&g));
        // Changed after the marker (the late check).
        let g = dir(&root, &format!("late{j}"));
        old_state(&port, &g);
        port.write_set(&g, &[put(FileRef::Receipts, b"r0\n")])
            .unwrap();
        let stamps = loaded_stamps(&port, &g);
        let (_, seen) = port.read_ref_stamped(&g, &FileRef::Receipts).unwrap();
        port.change_after_next_marker(&g, FileRef::Receipts, b"pulled late\n");
        assert_eq!(
            port.write_set_checked_also(&g, set, Some(&stamps), &[(FileRef::Receipts, seen)]),
            Err(FilesError::Changed(vec![FileRef::Receipts]))
        );
        let restarted = port.restart();
        assert_eq!(
            restarted.raw(&g, FileRef::Receipts).as_deref(),
            Some(&b"pulled late\n"[..])
        );
        assert_old(&restarted, &g);
        assert!(
            restarted.leftovers(&g).is_empty(),
            "{:?}",
            restarted.leftovers(&g)
        );
    }
}

/// What the find clauses need beyond the port: build a tree under the root (ws002 s17). Paths are folders.
pub trait FindHarness: GraphFiles {
    /// A graph folder: `dir` holding `graph.json` with `bytes`.
    fn make_graph(&self, dir: &GraphDir, bytes: &[u8]);
    /// An empty folder.
    fn make_folder(&self, dir: &GraphDir);
    /// `dir` holds `.git`: a repository's top.
    fn make_repo(&self, dir: &GraphDir);
    /// `link` is a symlink to the folder `target`.
    fn make_link(&self, link: &GraphDir, target: &GraphDir);
    /// The graph folder `dir` has a write set cut short after its marker: temp files and the marker are there, and
    /// the next call that recovers would roll it forward.
    fn make_pending(&self, dir: &GraphDir);
    /// The names of the temp files and the marker in `dir`, sorted.
    fn leftovers(&self, dir: &GraphDir) -> Vec<String>;
    /// `dir` cannot be listed (`on`), or can be again (`!on`).
    fn make_unlistable(&self, dir: &GraphDir, on: bool);
    /// The next [`GraphFiles::relocate`] answers as a rename across file systems would (`EXDEV`).
    fn cross_device_next_relocate(&self);
}

/// The clauses for [`GraphFiles::find_graphs`] (ws002 s17, architect).
pub const FIND_CLAUSES: &[&str] = &[
    "gf_f1_finds_graphs_sorted_and_never_inside_one",
    "gf_f2_never_follows_a_symlinked_folder",
    "gf_f3_skips_dot_and_listed_folders",
    "gf_f4_stops_at_a_cap_with_truncated",
    "gf_f5_reports_a_nested_repo_and_does_not_enter_it",
    "gf_f6_a_root_outside_the_roots_is_refused",
    "gf_f7_a_graph_json_over_its_cap_is_found_unread",
    "gf_f8_peek_and_find_report_a_pending_write_and_never_recover_it",
    "gf_f9_a_creation_cut_short_is_a_pending_graph_with_no_json",
    "gf_f10_a_folder_the_walk_cannot_list_is_counted_and_peek_refuses",
    "gf_f11_relocate_moves_the_folder_whole",
    "gf_f12_relocate_refuses_a_target_that_exists",
    "gf_f13_relocate_never_leaves_the_roots",
    "gf_f14_relocate_refuses_a_symlinked_folder",
    "gf_f15_relocate_refuses_a_missing_parent",
    "gf_f16_relocate_across_file_systems_moves_nothing",
    "gf_f17_relocate_carries_everything_inside",
    "gf_f18_relocate_refuses_a_file_target_and_a_folder_inside_itself",
];

fn found_paths(found: &umx_core::ports::Found) -> Vec<&str> {
    found.graphs.iter().map(|g| g.path.as_str()).collect()
}

pub fn gf_f1_finds_graphs_sorted_and_never_inside_one<P: FindHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let w = dir(&root, "w");
    port.make_graph(&dir(&w, "c/d/e"), b"{\"e\":1}");
    port.make_graph(&dir(&w, "a"), b"{\"a\":1}");
    port.make_graph(&dir(&w, "a/inner"), b"{}");
    port.make_graph(&dir(&w, "b b"), b"{}");
    port.make_folder(&dir(&w, "empty"));
    let found = port.find_graphs(&w, Default::default()).unwrap();
    assert_eq!(found_paths(&found), ["a", "b b", "c/d/e"]);
    assert_eq!(
        found.graphs[0].graph_json.as_deref(),
        Some(&b"{\"a\":1}"[..])
    );
    assert!(!found.truncated && found.nested_repos.is_empty());
    let again = port.find_graphs(&w, Default::default()).unwrap();
    assert_eq!(again, found, "the same answer every time");
    let top = port.find_graphs(&dir(&w, "a"), Default::default()).unwrap();
    assert_eq!(found_paths(&top), ["."], "the root itself is a graph");
}

pub fn gf_f2_never_follows_a_symlinked_folder<P: FindHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let w = dir(&root, "w");
    port.make_graph(&dir(&w, "real/g"), b"{}");
    port.make_link(&dir(&w, "link"), &dir(&w, "real"));
    let found = port.find_graphs(&w, Default::default()).unwrap();
    assert_eq!(found_paths(&found), ["real/g"]);
}

pub fn gf_f3_skips_dot_and_listed_folders<P: FindHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let w = dir(&root, "w");
    for skipped in [".hidden", ".umx-tmp", "node_modules", "target", "x/.cache"] {
        port.make_graph(&dir(&w, &format!("{skipped}/g")), b"{}");
    }
    port.make_graph(&dir(&w, "ok/g"), b"{}");
    let found = port.find_graphs(&w, Default::default()).unwrap();
    assert_eq!(found_paths(&found), ["ok/g"]);
}

pub fn gf_f4_stops_at_a_cap_with_truncated<P: FindHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    use umx_core::ports::FindLimits;
    let (port, root, _guard) = factory();
    let w = dir(&root, "w");
    port.make_graph(&dir(&w, "1/2/3/g"), b"{}");
    let deep = port.find_graphs(&w, FindLimits::default()).unwrap();
    assert_eq!(
        (found_paths(&deep), deep.truncated),
        (vec!["1/2/3/g"], false),
        "positive control"
    );
    let shallow = port.find_graphs(&w, FindLimits::with_depth(2)).unwrap();
    assert!(
        shallow.graphs.is_empty() && shallow.truncated,
        "{shallow:?}"
    );
    let few = FindLimits {
        folders: 2,
        ..FindLimits::default()
    };
    let capped = port.find_graphs(&w, few).unwrap();
    assert!(capped.graphs.is_empty() && capped.truncated, "{capped:?}");
    assert_eq!(FindLimits::with_depth(99).depth, FindLimits::MAX_DEPTH);
}

pub fn gf_f5_reports_a_nested_repo_and_does_not_enter_it<P: FindHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let w = dir(&root, "w");
    port.make_repo(&w);
    port.make_graph(&dir(&w, "mine"), b"{}");
    port.make_repo(&dir(&w, "sub"));
    port.make_graph(&dir(&w, "sub/theirs"), b"{}");
    let found = port.find_graphs(&w, Default::default()).unwrap();
    assert_eq!(
        found_paths(&found),
        ["mine"],
        "the root's own .git is not a nested repo"
    );
    assert_eq!(found.nested_repos, ["sub"]);
}

pub fn gf_f6_a_root_outside_the_roots_is_refused<P: FindHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, _root, _guard) = factory();
    assert!(matches!(
        port.find_graphs(&GraphDir::parse("/").unwrap(), Default::default()),
        Err(FilesError::OutsideRoots(_))
    ));
}

/// graph.json is read only up to its cap (security, s17): a bigger one is still found, with no bytes.
pub fn gf_f7_a_graph_json_over_its_cap_is_found_unread<P: FindHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let w = dir(&root, "w");
    let cap = umx_core::scan::FOUND_GRAPH_JSON_CAP as usize;
    port.make_graph(&dir(&w, "at"), &vec![b' '; cap]);
    port.make_graph(&dir(&w, "over"), &vec![b' '; cap + 1]);
    let found = port.find_graphs(&w, Default::default()).unwrap();
    assert_eq!(found_paths(&found), ["at", "over"]);
    assert_eq!(found.graphs[0].graph_json.as_ref().map(Vec::len), Some(cap));
    assert_eq!(found.graphs[1].graph_json, None);
}

const MOVED: &[u8] = br#"{"umx":1}"#;

/// The graph at `from` is still there, as it was.
fn still_there<P: GraphFiles>(port: &P, from: &GraphDir) {
    assert_eq!(
        port.read(from, GraphFile::Graph).unwrap().as_deref(),
        Some(MOVED)
    );
}

/// ws006 S23 (architect): relocate moves the graph folder whole, in one rename: its files read the same at the new
/// place, their stamps read there, and nothing is left at the old.
pub fn gf_f11_relocate_moves_the_folder_whole<P: FindHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let (from, to) = (root.join("a").unwrap(), root.join("b").unwrap());
    port.make_graph(&from, MOVED);
    port.relocate(&from, &to).unwrap();
    still_there(&port, &to);
    assert!(port.stamps(&to).is_ok());
    assert_eq!(port.read(&from, GraphFile::Graph).unwrap(), None);
    assert!(port.peek_graph(&from).unwrap().is_none());
}

/// A new folder that exists in any form is refused, and nothing moves (rename(2) would replace an empty folder).
pub fn gf_f12_relocate_refuses_a_target_that_exists<P: FindHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let (from, to) = (root.join("a").unwrap(), root.join("b").unwrap());
    port.make_graph(&from, MOVED);
    port.make_folder(&to);
    assert!(matches!(
        port.relocate(&from, &to),
        Err(FilesError::Invalid(umx_core::ports::RELOCATE_TARGET_EXISTS))
    ));
    still_there(&port, &from);
}

/// Both folders must be inside the roots.
pub fn gf_f13_relocate_never_leaves_the_roots<P: FindHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let from = root.join("a").unwrap();
    port.make_graph(&from, MOVED);
    let outside = GraphDir::parse("/umx-contract-outside-the-roots").unwrap();
    assert!(matches!(
        port.relocate(&from, &outside),
        Err(FilesError::OutsideRoots(_))
    ));
    assert!(matches!(
        port.relocate(&outside, &root.join("b").unwrap()),
        Err(FilesError::OutsideRoots(_))
    ));
    still_there(&port, &from);
}

/// A symlinked folder is never moved, and a symlink at the new path counts as there.
pub fn gf_f14_relocate_refuses_a_symlinked_folder<P: FindHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let (real, link) = (root.join("real").unwrap(), root.join("link").unwrap());
    port.make_graph(&real, MOVED);
    port.make_link(&link, &real);
    assert!(matches!(
        port.relocate(&link, &root.join("b").unwrap()),
        Err(FilesError::OutsideRoots(_))
    ));
    let other = root.join("other").unwrap();
    port.make_graph(&other, MOVED);
    assert!(matches!(
        port.relocate(&other, &link),
        Err(FilesError::Invalid(umx_core::ports::RELOCATE_TARGET_EXISTS))
    ));
    still_there(&port, &real);
    still_there(&port, &other);
}

/// The new folder's parent must exist: relocate never creates folders.
pub fn gf_f15_relocate_refuses_a_missing_parent<P: FindHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let from = root.join("a").unwrap();
    port.make_graph(&from, MOVED);
    let to = root.join("no").unwrap().join("b").unwrap();
    assert!(matches!(
        port.relocate(&from, &to),
        Err(FilesError::Invalid(
            umx_core::ports::RELOCATE_PARENT_MISSING
        ))
    ));
    still_there(&port, &from);
}

/// A rename across file systems is `CrossDevice`, never a copy: nothing moves.
pub fn gf_f16_relocate_across_file_systems_moves_nothing<P: FindHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let (from, to) = (root.join("a").unwrap(), root.join("b").unwrap());
    port.make_graph(&from, MOVED);
    port.cross_device_next_relocate();
    assert!(matches!(
        port.relocate(&from, &to),
        Err(FilesError::CrossDevice)
    ));
    still_there(&port, &from);
    assert_eq!(port.read(&to, GraphFile::Graph).unwrap(), None);
}

/// Everything inside the folder moves with it, as one rename moves it (review R25): a sub-folder's write cut short
/// is still pending at the new place (its marker moved with its files), and a symlink inside is there.
pub fn gf_f17_relocate_carries_everything_inside<P: FindHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let (from, to) = (root.join("a").unwrap(), root.join("b").unwrap());
    port.make_graph(&from, MOVED);
    port.make_folder(&from.join("sub").unwrap());
    port.make_pending(&from.join("sub").unwrap());
    port.make_link(&from.join("l").unwrap(), &root.join("elsewhere").unwrap());
    port.relocate(&from, &to).unwrap();
    still_there(&port, &to);
    assert!(
        port.peek_graph(&to.join("sub").unwrap())
            .unwrap()
            .is_some_and(|found| found.pending),
        "the cut-short write moved with its marker"
    );
    let other = root.join("other").unwrap();
    port.make_graph(&other, MOVED);
    assert!(matches!(
        port.relocate(&other, &to.join("l").unwrap()),
        Err(FilesError::Invalid(umx_core::ports::RELOCATE_TARGET_EXISTS))
    ));
}

/// A file at the new path is there too, and a folder never moves inside itself (review R26): nothing moves.
pub fn gf_f18_relocate_refuses_a_file_target_and_a_folder_inside_itself<P: FindHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let (from, other) = (root.join("a").unwrap(), root.join("b").unwrap());
    port.make_graph(&from, MOVED);
    port.make_graph(&other, MOVED);
    assert!(matches!(
        port.relocate(&from, &other.join("graph.json").unwrap()),
        Err(FilesError::Invalid(umx_core::ports::RELOCATE_TARGET_EXISTS))
    ));
    assert!(matches!(
        port.relocate(&from, &from.join("inner").unwrap()),
        Err(FilesError::Invalid(umx_core::ports::RELOCATE_INSIDE_ITSELF))
    ));
    still_there(&port, &from);
    still_there(&port, &other);
}

/// One `#[test]` per [`FIND_CLAUSES`] clause plus a test that the list is exactly `FIND_CLAUSES`.
/// `peek_graph` reports the graph in one folder as the walk does, and neither it nor `find_graphs` runs recovery: a
/// folder whose write was cut short is reported `pending` and left byte for byte as it was (review R289-3,
/// graph-storage-expert).
pub fn gf_f8_peek_and_find_report_a_pending_write_and_never_recover_it<P: FindHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let w = dir(&root, "w");
    port.make_graph(&dir(&w, "plain"), b"{}");
    port.make_folder(&dir(&w, "empty"));
    let plain = port
        .peek_graph(&dir(&w, "plain"))
        .unwrap()
        .expect("a graph");
    assert_eq!(
        (
            plain.path.as_str(),
            plain.graph_json.as_deref(),
            plain.pending
        ),
        (".", Some(&b"{}"[..]), false)
    );
    assert_eq!(port.peek_graph(&dir(&w, "empty")).unwrap(), None);
    assert_eq!(port.peek_graph(&dir(&w, "nowhere")).unwrap(), None);
    let cut = dir(&w, "cut");
    port.make_graph(&cut, b"{}");
    port.make_pending(&cut);
    let before = port.leftovers(&cut);
    assert!(!before.is_empty(), "the harness left a pending set");
    let peeked = port.peek_graph(&cut).unwrap().expect("a graph");
    assert!(peeked.pending, "{peeked:?}");
    let found = port.find_graphs(&w, Default::default()).unwrap();
    let pending: Vec<(&str, bool)> = found
        .graphs
        .iter()
        .map(|g| (g.path.as_str(), g.pending))
        .collect();
    assert_eq!(pending, [("cut", true), ("plain", false)]);
    assert_eq!(port.leftovers(&cut), before, "never recovered");
}

/// F176: a creation cut short leaves the marker and temp files and no `graph.json` yet. It is a graph all the same,
/// pending with no `graph.json`, to the walk and to `peek_graph`; it is not entered, and nothing in it changes.
pub fn gf_f9_a_creation_cut_short_is_a_pending_graph_with_no_json<P: FindHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let w = dir(&root, "w");
    let fresh = dir(&w, "fresh");
    port.make_folder(&fresh);
    port.make_pending(&fresh);
    port.make_graph(&dir(&fresh, "inner"), b"{}");
    let before = port.leftovers(&fresh);
    assert!(!before.is_empty(), "the harness left a pending set");
    let peeked = port.peek_graph(&fresh).unwrap().expect("a pending graph");
    assert_eq!((peeked.graph_json, peeked.pending), (None, true));
    let found = port.find_graphs(&w, Default::default()).unwrap();
    let seen: Vec<(&str, bool, bool)> = found
        .graphs
        .iter()
        .map(|g| (g.path.as_str(), g.graph_json.is_some(), g.pending))
        .collect();
    assert_eq!(seen, [("fresh", false, true)], "never inside it");
    assert_eq!(port.leftovers(&fresh), before, "never recovered");
}

/// F177: a folder the walk reaches but cannot list is passed over and counted, so a scan knows it did not see
/// everything; `peek_graph` on it is an error, never "no graph here".
pub fn gf_f10_a_folder_the_walk_cannot_list_is_counted_and_peek_refuses<P: FindHarness, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let w = dir(&root, "w");
    port.make_graph(&dir(&w, "open"), b"{}");
    let shut = dir(&w, "shut");
    port.make_graph(&dir(&shut, "g"), b"{}");
    port.make_unlistable(&shut, true);
    let found = port.find_graphs(&w, Default::default());
    let peeked = port.peek_graph(&shut).map(|g| g.map(|g| g.path));
    port.make_unlistable(&shut, false);
    let found = found.unwrap();
    assert_eq!((found_paths(&found), found.unlisted), (vec!["open"], 1));
    assert!(peeked.is_err(), "{peeked:?}");
    let all = port.find_graphs(&w, Default::default()).unwrap();
    assert_eq!(
        (found_paths(&all), all.unlisted),
        (vec!["open", "shut/g"], 0)
    );
}

#[macro_export]
macro_rules! graph_files_find_contract {
    ($factory:expr; $($clause:ident()),+ $(,)?) => {
        mod graph_files_find_contract {
            #[allow(unused_imports)]
            use super::*;
            $(
                #[test]
                fn $clause() {
                    $crate::graph_files::$clause($factory)
                }
            )+
            #[test]
            fn every_clause_is_listed() {
                $crate::assert_listed($crate::graph_files::FIND_CLAUSES, &[$(stringify!($clause)),+]);
            }
        }
    };
}

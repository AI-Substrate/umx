//! Review F1: no path through fs.graph-files allocates the body of an over-cap file, recovery and a set's old
//! stamps included. The reviewer's reproducer, measured with a counting allocator (its own test binary, one test,
//! so no other thread allocates while it watches).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use umx_adapter_fs::FsGraphFiles;
use umx_core::codec::GraphFile;
use umx_core::ports::{Change, FileRef, FilesError, GraphDir, GraphFiles};
use umx_core::setup::GraphFilesSetup;

struct Allocator;
static WATCH: AtomicBool = AtomicBool::new(false);
static MAX_ALLOC: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards every call to the system allocator unchanged; only records sizes.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if WATCH.load(Ordering::Relaxed) {
            MAX_ALLOC.fetch_max(layout.size(), Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if WATCH.load(Ordering::Relaxed) {
            MAX_ALLOC.fetch_max(size, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

/// The largest single allocation `f` makes, and its result.
fn largest<T>(f: impl FnOnce() -> T) -> (usize, T) {
    MAX_ALLOC.store(0, Ordering::Relaxed);
    WATCH.store(true, Ordering::Relaxed);
    let out = f();
    WATCH.store(false, Ordering::Relaxed);
    (MAX_ALLOC.load(Ordering::Relaxed), out)
}

#[test]
fn no_path_allocates_the_body_of_an_over_cap_graph_file() {
    let root = std::env::temp_dir().join(format!("umx-recovery-cap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let graph = root.join("g");
    std::fs::create_dir_all(&graph).unwrap();
    let cap = FileRef::from(GraphFile::Graph).cap() as usize;
    std::fs::write(graph.join("graph.json"), vec![b' '; cap + 1]).unwrap();
    let root_dir = GraphDir::parse(root.to_str().unwrap()).unwrap();
    let dir = GraphDir::parse(graph.to_str().unwrap()).unwrap();

    // A plain read: refused from the size.
    let direct = FsGraphFiles::new(vec![root_dir.clone()]);
    let (allocated, result) = largest(|| direct.read(&dir, GraphFile::Graph));
    assert!(
        matches!(result, Err(FilesError::TooLarge { .. })),
        "{result:?}"
    );
    assert!(allocated <= cap, "a read allocated {allocated}");

    // A set that replaces it alongside another file stamps the old bytes: refused, nothing changed.
    let (allocated, result) = largest(|| {
        direct.write_set(
            &dir,
            &[
                Change {
                    file: GraphFile::Graph.into(),
                    bytes: Some(b"{}"),
                },
                Change {
                    file: GraphFile::Nodes.into(),
                    bytes: Some(b""),
                },
            ],
        )
    });
    assert!(
        matches!(result, Err(FilesError::TooLarge { .. })),
        "{result:?}"
    );
    assert!(allocated <= cap, "a set's old stamp allocated {allocated}");
    assert_eq!(
        std::fs::metadata(graph.join("graph.json")).unwrap().len(),
        cap as u64 + 1
    );
    assert!(!graph.join("nodes.jsonl").exists());

    // Recovery of a pending marker naming it: a conflict, found without reading the body.
    std::fs::write(
        graph.join(".umx-tmp-set.json"),
        concat!(
            "{\"hash\":\"fnv1a64-words-v1\"}\n",
            "{\"target\":\"graph.json\",\"temp\":\".umx-tmp-1-graph.json\",",
            "\"old\":\"absent\",\"new\":{\"size\":1,\"hash\":\"0000000000000000\"}}\n"
        ),
    )
    .unwrap();
    let files = FsGraphFiles::new(vec![root_dir.clone()]);
    let (allocated, result) = largest(|| files.read(&dir, GraphFile::Graph));
    assert!(
        allocated <= cap,
        "read allocated {allocated} bytes for a file capped at {cap}; result: {result:?}"
    );
    assert_eq!(
        result,
        Err(FilesError::RecoveryConflict(vec![GraphFile::Graph.into()]))
    );

    // An over-cap temp named by the marker is a conflict too, unread.
    std::fs::write(graph.join("graph.json"), b"{}").unwrap();
    std::fs::write(graph.join(".umx-tmp-1-graph.json"), vec![b' '; cap + 1]).unwrap();
    let marker = concat!(
        "{\"hash\":\"fnv1a64-words-v1\"}\n",
        "{\"target\":\"graph.json\",\"temp\":\".umx-tmp-1-graph.json\",",
        "\"old\":{\"size\":2,\"hash\":\"HASH\"},\"new\":{\"size\":1,\"hash\":\"0000000000000000\"}}\n"
    );
    let old = format!("{:016x}", fnv(b"{}"));
    std::fs::write(
        graph.join(".umx-tmp-set.json"),
        marker.replace("HASH", &old),
    )
    .unwrap();
    let files = FsGraphFiles::new(vec![root_dir]);
    let (allocated, result) = largest(|| files.read(&dir, GraphFile::Graph));
    assert!(
        allocated <= cap,
        "a temp allocated {allocated}; result: {result:?}"
    );
    assert_eq!(
        result,
        Err(FilesError::RecoveryConflict(vec![GraphFile::Graph.into()]))
    );
    std::fs::remove_dir_all(&root).unwrap();
}

/// The marker's hash (fnv1a64-words-v1), for a hand-made marker.
fn fnv(bytes: &[u8]) -> u64 {
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for chunk in bytes.chunks(8) {
        let mut w = [0u8; 8];
        w[..chunk.len()].copy_from_slice(chunk);
        h = (h ^ u64::from_le_bytes(w)).wrapping_mul(PRIME);
    }
    (h ^ bytes.len() as u64).wrapping_mul(PRIME)
}

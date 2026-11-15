//! R81-1 (ws004 s13, acceptance section 2): reading a graph's shards through the services path (the scan a load and
//! `umx embeddings repair` run) holds at most 10% over the final vector blocks at its peak, at k = 4 and 256 dims.
//! Its own test binary, so its counting allocator measures only this file's tests.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;
use umx_core::codec::{self, GraphFile};
use umx_core::ports::{Change, FileRef, GraphDir, ModelKey};
use umx_core::setup::GraphFilesSetup;
use umx_core::{Id, Node, Value};
use umx_fakes::{FakeClock, FakeEmbedder, FakeGraphFiles, FakeIds, FakeLocalState};
use umx_services::{Ctx, Pacer, Planned, Ports, Services, Step, batches, step};

struct Counting;

std::thread_local! {
    /// Net bytes this thread holds and the most it held since the last reset (tests run on their own threads).
    static NET: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
}

fn add(n: isize) {
    let _ = NET.try_with(|net| {
        net.set(net.get() + n);
        let _ = PEAK.try_with(|p| p.set(p.get().max(net.get())));
    });
}

// SAFETY: every call forwards to `System` unchanged; the counters only observe sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        add(l.size() as isize);
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        add(l.size() as isize);
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        add(-(l.size() as isize));
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        add(new as isize - l.size() as isize);
        unsafe { System.realloc(p, l, new) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

fn ctx() -> Ctx {
    Ctx::new("tester")
}

fn services(files: &Arc<FakeGraphFiles>, embedder: &Arc<FakeEmbedder>) -> Services {
    let ports = Ports {
        sources: std::sync::Arc::new(umx_fakes::FakeSourceReader::open(
            umx_core::ports::GraphDir::parse("/").unwrap(),
        )),
        files: files.clone(),
        vector_cache: files.clone(),
        vector_cache_root: GraphDir::parse("/work/cache").unwrap(),
        clock: Arc::new(FakeClock::default()),
        ids: Arc::new(FakeIds::new()),
        state: Arc::new(FakeLocalState::new()),
        scopes: Arc::new(FakeLocalState::new()),
        registry: Arc::new(FakeLocalState::new()),
        allows: Arc::new(FakeLocalState::new()),
        lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
    };
    Services::new(ports).with_embedder(embedder.clone())
}

fn embed_all(svc: &mut Services, embedder: &FakeEmbedder) {
    let mut pacer = Pacer::default();
    loop {
        let jobs = svc.take_embed_jobs();
        if jobs.is_empty() {
            return;
        }
        let planned: Vec<Planned> = jobs
            .iter()
            .filter_map(|j| svc.embed_plan(j).unwrap())
            .collect();
        for batch in batches(planned) {
            let Step::Done { vectors, .. } = step(embedder, &batch, &mut pacer, 0) else {
                panic!("the fake embeds");
            };
            svc.embed_apply(&vectors).unwrap();
        }
    }
}

#[test]
fn reading_the_shards_holds_at_most_a_tenth_over_the_blocks() {
    // Enough nodes to fill every one of the 1,024 shards a few times over, as a real graph does: the fixed cost per
    // shard (its name in the listing) is then a small part, as it is at 10k nodes.
    const NODES: u128 = 4000;
    let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let embedder = Arc::new(FakeEmbedder::new(ModelKey::new("fake", 256).unwrap()));
    let mut svc = services(&files, &embedder);
    svc.graph_new(&ctx(), "/work/g", "demo", None, false, None)
        .unwrap();
    svc.ontology_add_field(&ctx(), "problem", "impact:string", None)
        .unwrap();
    // Three texts and the whole node per node (k = 4), written straight into nodes.jsonl.
    let nodes: Vec<Node> = (1..=NODES)
        .map(|n| Node {
            id: Id((0x0194_0000_0000_u128 << 80) | n),
            kind: "problem".into(),
            title: format!("Problem number {n}"),
            status: "open".into(),
            note: format!("A note about problem {n}."),
            evidence: vec![],
            status_history: vec![],
            created: "2026-11-10T01:00:00Z".into(),
            updated: "2026-11-10T01:00:00Z".into(),
            created_by: "tester".into(),
            locator: None,
            authored_by_session: None,
            edited_by_session: None,
            fields: [("impact".to_owned(), Value::String(format!("Impact {n}")))].into(),
        })
        .collect();
    let bytes = codec::encode_nodes(&nodes);
    let dir = GraphDir::parse("/work/g").unwrap();
    files
        .write_set(
            &dir,
            &[Change {
                file: FileRef::Graph(GraphFile::Nodes),
                bytes: Some(&bytes),
            }],
        )
        .unwrap();
    let mut svc = services(&files, &embedder);
    svc.graph_load(&ctx(), "/work/g", None).unwrap();
    embed_all(&mut svc, &embedder);

    // A fresh daemon loads the committed shards, then repair reads them all again through the same scan.
    let mut svc = services(&files, &embedder);
    svc.graph_load(&ctx(), "/work/g", None).unwrap();
    let blocks = svc.loaded_graph("demo").unwrap().memory_bytes().vectors as isize;
    assert!(
        blocks >= (NODES as isize) * 4 * 256,
        "{blocks} bytes of blocks for {NODES} nodes at k = 4"
    );
    let before = NET.with(Cell::get);
    PEAK.with(|p| p.set(before));
    let repaired = svc.embeddings_repair(&ctx()).unwrap();
    assert_eq!((repaired.value.queued, repaired.value.tidied), (0, 0));
    let peak = PEAK.with(Cell::get) - before;
    let after = svc.loaded_graph("demo").unwrap().memory_bytes().vectors as isize;
    assert_eq!(after, blocks, "the same blocks again");
    eprintln!(
        "scan peak over the blocks: {peak} of {blocks} bytes ({:.2}%)",
        peak as f64 * 100.0 / blocks as f64
    );
    assert!(
        peak * 10 <= blocks,
        "the scan peaked {peak} bytes over the {blocks} bytes it started with: {:.1}% (band 10%)",
        peak as f64 * 100.0 / blocks as f64
    );
}

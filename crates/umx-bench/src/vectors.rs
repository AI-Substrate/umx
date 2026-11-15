//! Vector blocks (workstream 004 s13; s07 acceptance sections 1 and 2): seeded random int8 vectors, 256 dimensions,
//! one whole-node vector and k - 1 field vectors per node, written as committed shard files in memory (s12's line
//! codec), then decoded into the graph's blocks the way a graph load does. Reports memory against the allocator,
//! shard decode time and peak, and the exact scan kernel over the blocks.

use crate::alloc;
use crate::fixture::{Fixture, Rng};
use crate::storage::{build, per_op, round3};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::hint::black_box;
use std::time::Instant;
use umx_core::Id;
use umx_core::similarity::top_k;
use umx_core::vectors::{
    ShardLine, TextHash, VectorLine, decode_shard, encode_shard, shard_of, whole_node_field,
};
use umx_graph::Graph;

pub const DIMS: usize = 256;
const SPACE: &str = "bench-model@256";
const TOP: usize = 10;

/// The field names of a node's vectors at `k`: the whole node, then title, note, f3, f4, ...
fn fields(k: usize) -> Vec<String> {
    let mut out = vec![whole_node_field()];
    out.extend(
        ["title".to_owned(), "note".to_owned()]
            .into_iter()
            .chain((3..).map(|i| format!("f{i}")))
            .take(k.saturating_sub(1)),
    );
    out
}

/// Every shard file of the fixture's vectors, by shard name, as `encode_shard` writes them.
fn shards(ids: &[Id], k: usize, seed: u64) -> BTreeMap<String, Vec<u8>> {
    let mut rng = Rng(seed ^ 0x5EC7_0B10);
    let names = fields(k);
    let hash = TextHash::of("bench");
    let mut by_shard: BTreeMap<String, Vec<ShardLine>> = BTreeMap::new();
    for &id in ids {
        let lines = by_shard.entry(shard_of(id)).or_default();
        for field in &names {
            let values = (0..DIMS)
                .map(|_| (rng.below(255) as i64 - 127) as i8)
                .collect();
            lines.push(ShardLine::Vector(VectorLine {
                node: id,
                field: field.clone(),
                hash,
                values,
            }));
        }
    }
    by_shard
        .into_iter()
        .map(|(s, lines)| (s, encode_shard(&lines)))
        .collect()
}

/// The load path: each shard decoded on its own and put line by line, so only one shard's lines are held at once.
/// Returns how many vectors the blocks hold.
pub fn load(g: &mut Graph, files: &BTreeMap<String, Vec<u8>>) -> usize {
    let whole = whole_node_field();
    let mut load = g.load_vectors(SPACE, DIMS);
    for bytes in files.values() {
        for line in decode_shard(bytes) {
            let Some(v) = line.vector() else { continue };
            let field = if v.field == whole {
                None
            } else if v.field.starts_with('@') {
                continue;
            } else {
                Some(v.field.as_str())
            };
            load.put(v.node, field, &v.values, v.hash);
        }
    }
    load.finish()
}

/// A seeded tenth of the nodes, as a bit per handle (the shape `--within` gives a scan).
fn tenth(n: usize, seed: u64) -> Vec<u64> {
    let mut rng = Rng(seed ^ 0x7E47);
    let mut bits = vec![0u64; n.div_ceil(64)];
    for h in 0..n {
        if rng.below(10) == 0 {
            bits[h / 64] |= 1 << (h % 64);
        }
    }
    bits
}

pub fn measure(runs: usize, seed: u64, fx: &Fixture, k: usize) -> Value {
    let mut g = build(fx);
    let n = g.node_count();
    let ids: Vec<Id> = g.nodes().map(|r| r.id()).collect();
    let files = shards(&ids, k, seed);
    let shard_bytes: usize = files.values().map(Vec::len).sum();
    let lines = n * k;

    // Memory: the blocks against the allocator, and the peak while decoding over the final blocks.
    let base = alloc::live();
    alloc::reset_peak();
    let held = load(&mut g, &files);
    let peak = alloc::peak_since(base);
    let mem = alloc::live_since(base);
    assert_eq!(held, lines, "every vector line is held");
    let part = g.memory_bytes().vectors;
    let whole = g.whole_block(SPACE).expect("whole-node vectors");
    let fields: Vec<_> = g.field_blocks(SPACE).collect();
    let field_rows: usize = fields.iter().map(|f| f.rows()).sum();
    // Values, inverse length, presence bit and the whole-node text hash (s09) per row.
    let whole_bytes =
        whole.values().len() + whole.inverse_lengths().len() * (4 + 16) + n.div_ceil(64) * 8;
    let field_bytes: usize = fields.iter().map(|f| f.values().len() + f.rows() * 8).sum();

    // R81-3: the same vectors applied one by one through the writer path (a fresh graph embedded job by job), as
    // set_vector leaves the blocks until the next load compacts them.
    let mut applied = build(fx);
    let whole_name = whole_node_field();
    let before = alloc::live();
    for bytes in files.values() {
        for line in decode_shard(bytes) {
            let Some(v) = line.vector() else { continue };
            let field = (v.field != whole_name).then_some(v.field.as_str());
            applied.set_vector(SPACE, v.node, field, &v.values, v.hash);
        }
    }
    let apply_mem = alloc::live_since(before);
    let apply_part = applied.memory_bytes().vectors;
    drop(applied);

    // Decode time: committed shards into blocks, on a graph with no vectors yet each run.
    let mut fresh = build(fx);
    let (decode_ms, _) = per_op(runs, 1, 1e3, || {
        let held = load(&mut fresh, &files);
        assert_eq!(held, lines);
        held
    });
    drop(fresh);
    let (graph_load_ms, _) = per_op(runs, 1, 1e3, || build(fx).node_count());
    let (graph_and_vectors_ms, _) = per_op(runs, 1, 1e3, || {
        let mut g = build(fx);
        load(&mut g, &files)
    });

    // One node re-embedded (F1, verdict): its shard is written whole with that node's lines replaced, and only
    // that shard. An unchanged shard encodes to its own bytes, so it is not written at all.
    let target = ids[n / 2];
    let shard = &files[&shard_of(target)];
    let mut rewritten = decode_shard(shard);
    assert_eq!(
        encode_shard(&rewritten),
        *shard,
        "an unchanged shard writes nothing"
    );
    let mut rng = Rng(seed ^ 0xEE);
    for line in &mut rewritten {
        if let ShardLine::Vector(v) = line
            && v.node == target
        {
            v.values = (0..DIMS)
                .map(|_| (rng.below(255) as i64 - 127) as i8)
                .collect();
        }
    }
    let reembed_bytes = encode_shard(&rewritten).len();
    let mean_shard = shard_bytes as f64 / files.len() as f64;

    // Scans: top 10 against one node's whole-node vector; the check value is the hits' row sum.
    let query = g
        .vector(SPACE, g.node_ix(ids[n / 2]).unwrap(), None)
        .unwrap()
        .to_vec();
    let rows_sum =
        |hits: Vec<umx_core::similarity::Hit>| hits.iter().map(|h| u64::from(h.row)).sum::<u64>();
    let (scan_whole_ms, whole_check) = per_op(runs.max(9), 1, 1e3, || {
        rows_sum(top_k(
            &query,
            whole.values(),
            whole.inverse_lengths(),
            TOP,
            |r| whole.has(r),
        ))
    });
    let scope = tenth(n, seed);
    let (scan_whole_tenth_ms, tenth_check) = per_op(runs.max(9), 1, 1e3, || {
        rows_sum(top_k(
            &query,
            whole.values(),
            whole.inverse_lengths(),
            TOP,
            |r| scope[r / 64] & (1 << (r % 64)) != 0 && whole.has(r),
        ))
    });
    let (scan_every_block_ms, every_check) = per_op(runs.max(9), 1, 1e3, || {
        let mut sum = rows_sum(top_k(
            &query,
            whole.values(),
            whole.inverse_lengths(),
            TOP,
            |r| whole.has(r),
        ));
        for f in &fields {
            sum += top_k(&query, f.values(), f.inverse_lengths(), TOP, |_| true)
                .iter()
                .map(|h| f.node(h.row).index() as u64)
                .sum::<u64>();
        }
        sum
    });

    let mut v = json!({
        "per_node": k,
        "dims": DIMS,
        "count": held,
        "shard_files": files.len(),
        "shard_bytes": shard_bytes,
        "shard_bytes_per_line": round3(shard_bytes as f64 / lines as f64),
        "shard_mean_bytes": round3(mean_shard),
        "reembed_one_node": { "files_written": 1, "bytes_written": reembed_bytes, "over_mean_shard": round3(reembed_bytes as f64 / mean_shard) },
        "mem_bytes": mem,
        "mem_per_node_bytes": round3(mem as f64 / n as f64),
        "whole_bytes_per_vector": round3(whole_bytes as f64 / whole.count() as f64),
        "field_bytes_per_vector": if field_rows == 0 { Value::Null } else { json!(round3(field_bytes as f64 / field_rows as f64)) },
        "memory_bytes_over_alloc": round3(part as f64 / mem as f64),
        "apply_mem_per_node_bytes": round3(apply_mem as f64 / n as f64),
        "apply_memory_bytes_over_alloc": round3(apply_part as f64 / apply_mem as f64),
        "decode_peak_over_blocks": round3((peak - mem) as f64 / mem as f64),
        "decode_ms": decode_ms,
        "graph_load_ms": graph_load_ms,
        "graph_and_vectors_load_ms": graph_and_vectors_ms,
        "scan_whole_ms": scan_whole_ms,
        "scan_whole_tenth_ms": scan_whole_tenth_ms,
        "scan_every_block_ms": scan_every_block_ms,
        "scan_checks": [whole_check, tenth_check, every_check],
    });
    v["similarity"] = similarity(&mut g, runs, seed);
    v
}

/// similar_to lists (ws004 s09) over the loaded whole-node block, k = 10 and floor -1 so every list fills (the
/// bench's vectors are random, so near-zero cosines would leave a real floor's lists nearly empty): the rebuild's
/// wall time and its two lock holds (the snapshot copy, read; the apply with the reverse list, writer), memory
/// against the allocator, the reverse rebuild, one node's update, and reading stored lines back.
fn similarity(g: &mut Graph, runs: usize, seed: u64) -> Value {
    const K: usize = 10;
    const FLOOR: i32 = -10_000;
    let n = g.node_count();
    // From before the snapshot: the compute allocates the slots the apply moves in; the snapshot is freed.
    let base = alloc::live();
    let t = Instant::now();
    let snapshot = g.similar_snapshot(SPACE).expect("whole-node vectors");
    let snapshot_ms = round3(t.elapsed().as_secs_f64() * 1e3);
    let t = Instant::now();
    let lists = snapshot.compute(K, FLOOR);
    let rebuild_ms = round3(t.elapsed().as_secs_f64() * 1e3);
    let edges = lists.edges();
    drop(snapshot);
    let t = Instant::now();
    assert!(g.apply_similar(lists), "nothing moved");
    let apply_ms = round3(t.elapsed().as_secs_f64() * 1e3);
    let mem = alloc::live_since(base);
    let part = g.memory_bytes().similarity;
    let slots = n * K * 8 + n;
    let (reverse_ms, _) = per_op(runs.max(5), 1, 1e3, || {
        g.finish_similar();
        0
    });
    // One node's update, two ways, for a node in the middle. Arrival: its first whole-node vector lands (one
    // scan, its list, the lists it enters, the reverse list): the graph-storage-expert's 5 ms case. Change: a
    // new vector replaces one it had, so every list that held it at a better score is recomputed in full too.
    let mut rng = Rng(seed ^ 0x51_41);
    let id = g.nodes().nth(n / 2).unwrap().id();
    let mut update = |g: &mut Graph, arrival: bool| -> Value {
        let mut times: Vec<f64> = (0..runs.max(5))
            .map(|_| {
                let ix = g.node_ix(id).unwrap();
                if arrival {
                    g.remove_vector(SPACE, id, None);
                    g.refresh_similar(ix);
                    g.finish_similar();
                }
                let values: Vec<i8> = (0..DIMS)
                    .map(|_| (rng.below(255) as i64 - 127) as i8)
                    .collect();
                g.set_vector(SPACE, id, None, &values, TextHash::of("bench"));
                let t = Instant::now();
                black_box(g.refresh_similar(ix));
                g.finish_similar();
                round3(t.elapsed().as_secs_f64() * 1e3)
            })
            .collect();
        times.sort_by(f64::total_cmp);
        json!({ "median": times[times.len() / 2], "min": times[0], "runs": times.len() })
    };
    let arrival_ms = update(g, true);
    let change_ms = update(g, false);
    // Stored lines back into lists, as a load whose _meta.tsv matches does.
    let lines: Vec<(Id, Id, i32)> = g
        .nodes()
        .flat_map(|node| {
            g.similar_out(node.ix())
                .iter()
                .map(|e| (node.id(), g.node_at(e.other).id(), e.score))
                .collect::<Vec<_>>()
        })
        .collect();
    let (load_ms, _) = per_op(runs.max(3), 1, 1e3, || {
        let mut load = g.load_similar(SPACE, K, FLOOR);
        for &(a, b, s) in &lines {
            load.put(a, b, s);
        }
        load.finish();
        0
    });
    json!({
        "k": K, "floor": -1.0, "edges": edges,
        "rebuild_ms": rebuild_ms,
        "lock_snapshot_ms": snapshot_ms,
        "lock_apply_ms": apply_ms,
        "mem_bytes": mem,
        "memory_bytes_over_alloc": round3(part as f64 / mem as f64),
        "slots_per_node_bytes": round3(slots as f64 / n as f64),
        "reverse_bytes_per_edge": round3((mem as f64 - slots as f64) / edges.max(1) as f64),
        "reverse_rebuild_ms": reverse_ms,
        "one_node_update_ms": arrival_ms,
        "one_node_change_ms": change_ms,
        "load_stored_lists_ms": load_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::generate;

    /// s13 acceptance section 1: `Graph::memory_bytes().vectors` within 5% of the allocator, and the per-vector
    /// sizes inside their bands, at 1k nodes.
    /// s09: `Graph::memory_bytes().similarity` against the allocator at 1k nodes, k = 10, every list full.
    #[test]
    fn similarity_memory_matches_the_allocator() {
        let fx = generate(1, 1000, 3, false);
        let mut g = build(&fx);
        let ids: Vec<Id> = g.nodes().map(|r| r.id()).collect();
        load(&mut g, &shards(&ids, 1, 1));
        // From before the compute: its slots are the ones the apply moves in.
        let before = alloc::thread_net();
        let lists = g.similar_snapshot(SPACE).unwrap().compute(10, -10_000);
        assert!(g.apply_similar(lists));
        let counted = (alloc::thread_net() - before) as f64;
        let part = g.memory_bytes().similarity as f64;
        assert!(
            (part / counted - 1.0).abs() <= 0.05,
            "{part} against {counted}"
        );
        assert!(
            (part - 1000.0 * 81.0) / 10_000.0 <= 9.0,
            "at most 9 bytes per reverse entry: {part}"
        );
    }

    #[test]
    fn vector_memory_matches_the_allocator_and_the_bands() {
        let fx = generate(1, 1000, 3, false);
        let mut g = build(&fx);
        let ids: Vec<Id> = g.nodes().map(|r| r.id()).collect();
        let files = shards(&ids, 4, 1);
        let before = alloc::thread_net();
        assert_eq!(load(&mut g, &files), 4000);
        let counted = (alloc::thread_net() - before) as f64;
        let part = g.memory_bytes().vectors as f64;
        assert!(
            (part / counted - 1.0).abs() <= 0.05,
            "{part} against {counted}"
        );
        assert!(
            counted / 1000.0 <= 1090.0,
            "{} bytes per node",
            counted / 1000.0
        );
        // The same files decode to the same blocks, so the scan answers the same.
        let mut again = build(&fx);
        load(&mut again, &files);
        let (a, b) = (
            g.whole_block(SPACE).unwrap(),
            again.whole_block(SPACE).unwrap(),
        );
        assert_eq!(
            (a.values(), a.inverse_lengths()),
            (b.values(), b.inverse_lengths())
        );
    }
}

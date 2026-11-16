//! Embedding over the fakes (ws004 s07a): which texts embed, that only changes re-embed, shard bytes, load, repair
//! and pacing. Counts come from the fake embedder's call counter and the fake files' shard-write counter.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use umx_core::Id;
use umx_core::codec::GraphFile;
use umx_core::ports::{
    Change, EmbedError, Embedded, Embedder, FileRef, GraphDir, GraphFiles, Locality, ModelKey,
    TextKind,
};
use umx_core::setup::GraphFilesSetup;
use umx_core::value::SetValue;
use umx_core::vectors::{self, ShardFile, ShardKind, ShardLine, TextHash};
use umx_fakes::{FakeClock, FakeEmbedder, FakeGraphFiles, FakeIds, FakeLocalState};
use umx_services::{
    Ctx, EmbedNote, NodeAddRequest, NodeEditRequest, NoteKind, Pacer, Planned, Ports, Services,
    Step, batches, step,
};

struct World {
    files: Arc<FakeGraphFiles>,
    embedder: Arc<FakeEmbedder>,
    svc: Services,
}

fn key() -> ModelKey {
    ModelKey::new("fake", 8).unwrap()
}

fn dir() -> GraphDir {
    GraphDir::parse("/work/g").unwrap()
}

fn services(files: &Arc<FakeGraphFiles>, embedder: &Arc<FakeEmbedder>) -> Services {
    let ports = Ports {
        sources: std::sync::Arc::new(umx_fakes::FakeSourceReader::open(
            umx_core::ports::GraphDir::parse("/").unwrap(),
        )),
        files: files.clone(),
        // The same fake disk, so the cache survives a "restart"; its root is outside every graph folder.
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

fn world() -> World {
    let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let embedder = Arc::new(FakeEmbedder::new(key()));
    let mut svc = services(&files, &embedder);
    svc.graph_new(&ctx(), "/work/g", "demo", None, false, None)
        .unwrap();
    svc.ontology_add_field(&ctx(), "problem", "impact:string", None)
        .unwrap();
    svc.ontology_add_field(&ctx(), "problem", "due:date", None)
        .unwrap();
    World {
        files,
        embedder,
        svc,
    }
}

fn ctx() -> Ctx {
    Ctx::new("tester")
}

fn add(w: &mut World, title: &str, note: &str, impact: &str) -> Id {
    w.svc
        .node_add(
            &ctx(),
            NodeAddRequest {
                kind: "problem".into(),
                title: title.into(),
                note: Some(note.into()),
                sets: vec![("impact".into(), SetValue::Str(impact.into()))],
                ..Default::default()
            },
        )
        .unwrap()
        .value
        .node
        .id
}

/// Runs the queue to empty the way the daemon's worker does: plan (shared), embed (no store), apply (writer).
fn run(w: &mut World) {
    let embedder = w.embedder.clone();
    run_with(w, &*embedder);
}

fn run_with(w: &mut World, embedder: &dyn Embedder) -> Vec<EmbedNote> {
    let mut pacer = Pacer::default();
    let mut all = Vec::new();
    loop {
        let jobs = w.svc.take_embed_jobs();
        if jobs.is_empty() {
            return all;
        }
        let planned: Vec<Planned> = jobs
            .iter()
            .filter_map(|j| w.svc.embed_plan(j).unwrap())
            .collect();
        for batch in batches(planned) {
            match step(embedder, &batch, &mut pacer, 0) {
                Step::Done { vectors: v, notes } => {
                    w.svc.embed_apply(&v).unwrap();
                    all.extend(notes);
                }
                other => panic!("{other:?}"),
            }
        }
    }
}

fn shard(id: Id) -> ShardFile {
    ShardFile::of(key(), id)
}

/// Writes a shard behind the services' back (a hand edit, a merge, an older umx).
fn put_shard(w: &World, id: Id, bytes: &[u8]) {
    w.files
        .write_set(
            &dir(),
            &[Change {
                file: FileRef::Shard(shard(id)),
                bytes: Some(bytes),
            }],
        )
        .unwrap();
}

fn lines(w: &World, id: Id) -> Vec<ShardLine> {
    w.files
        .read_ref(&dir(), &FileRef::Shard(shard(id)))
        .unwrap()
        .map(|b| vectors::decode_shard(&b))
        .unwrap_or_default()
}

fn fields_of(w: &World, id: Id) -> Vec<String> {
    lines(w, id)
        .iter()
        .filter_map(ShardLine::vector)
        .filter(|v| v.node == id)
        .map(|v| v.field.clone())
        .collect()
}

fn counts(w: &World) -> (usize, usize, usize) {
    (
        w.embedder.calls(),
        w.embedder.texts_embedded(),
        w.files.shard_writes(),
    )
}

#[test]
fn a_node_with_three_text_fields_gets_four_vectors_after_the_write_returns() {
    let mut w = world();
    let id = add(&mut w, "Slow builds", "Builds take an hour.", "Teams wait");
    assert!(
        lines(&w, id).is_empty(),
        "the add returned before any embedding"
    );
    assert_eq!(counts(&w), (0, 0, 0));
    run(&mut w);
    assert_eq!(fields_of(&w, id), ["@node.v2", "impact", "note", "title"]);
    assert_eq!(
        counts(&w),
        (1, 4, 1),
        "one call, four texts, one shard write"
    );
}

#[test]
fn an_unchanged_re_save_embeds_nothing_and_writes_no_shard() {
    let mut w = world();
    let id = add(&mut w, "Slow builds", "Builds take an hour.", "Teams wait");
    run(&mut w);
    let before = counts(&w);
    let same = |title: &str| NodeEditRequest {
        title: Some(title.into()),
        ..Default::default()
    };
    w.svc
        .node_edit(&ctx(), &id.to_text(), same("Slow builds"))
        .unwrap();
    // A field that is not embedded changes the node but none of its texts.
    w.svc
        .node_edit(
            &ctx(),
            &id.to_text(),
            NodeEditRequest {
                sets: vec![("due".into(), SetValue::Raw("2026-12-01".into()))],
                ..Default::default()
            },
        )
        .unwrap();
    w.svc
        .node_status(&ctx(), &id.to_text(), "agreed", Some("done"), None)
        .unwrap();
    run(&mut w);
    assert_eq!(counts(&w), before);
}

#[test]
fn editing_one_field_re_embeds_that_field_and_the_whole_node_in_one_shard_write() {
    let mut w = world();
    let id = add(&mut w, "Slow builds", "Builds take an hour.", "Teams wait");
    let other = add(&mut w, "Flaky tests", "Retries hide it.", "Trust drops");
    run(&mut w);
    let (calls, texts, writes) = counts(&w);
    assert_ne!(shard(other), shard(id));
    let other_before: Vec<ShardLine> = lines(&w, other);
    w.svc
        .node_edit(
            &ctx(),
            &id.to_text(),
            NodeEditRequest {
                sets: vec![("impact".into(), SetValue::Str("Teams wait a day".into()))],
                ..Default::default()
            },
        )
        .unwrap();
    run(&mut w);
    assert_eq!(counts(&w), (calls + 1, texts + 2, writes + 1));
    let impact = lines(&w, id)
        .iter()
        .filter_map(ShardLine::vector)
        .find(|v| v.node == id && v.field == "impact")
        .unwrap()
        .hash;
    assert_eq!(impact, TextHash::of("title: none | text: Teams wait a day"));
    assert_eq!(lines(&w, other), other_before);
}

#[test]
fn the_same_vector_writes_the_same_line_bytes() {
    let mut w = world();
    let id = add(&mut w, "Slow builds", "Builds take an hour.", "Teams wait");
    run(&mut w);
    let first = w
        .files
        .read_ref(&dir(), &FileRef::Shard(shard(id)))
        .unwrap()
        .unwrap();
    // Throw the shard away and embed again from scratch: byte-identical file.
    put_shard(&w, id, b"");
    let mut fresh = services(&w.files, &w.embedder);
    fresh.graph_load(&ctx(), "/work/g", None).unwrap();
    w.svc = fresh;
    run(&mut w);
    assert_eq!(
        w.files
            .read_ref(&dir(), &FileRef::Shard(shard(id)))
            .unwrap()
            .unwrap(),
        first
    );
    assert_eq!(vectors::encode_shard(&vectors::decode_shard(&first)), first);
}

#[test]
fn a_graph_whose_vectors_are_committed_loads_with_zero_embeds() {
    let mut w = world();
    for i in 0..5 {
        add(&mut w, &format!("Node {i}"), "A note.", "Some impact");
    }
    run(&mut w);
    let before = counts(&w);
    // A teammate's clone: a new daemon over the same files.
    let mut clone = services(&w.files, &w.embedder);
    let loaded = clone.graph_load(&ctx(), "/work/g", None).unwrap();
    assert!(loaded.notes.is_empty(), "{:?}", loaded.notes);
    w.svc = clone;
    run(&mut w);
    assert_eq!(counts(&w), before);
}

/// Rewrites `id`'s shard through `edit`, as a hand edit, a merge or an older umx would leave it.
fn edit_shard(w: &World, id: Id, edit: impl Fn(Vec<ShardLine>) -> Vec<ShardLine>) {
    let bytes = vectors::encode_shard(&edit(lines(w, id)));
    put_shard(w, id, &bytes);
}

fn reload(w: &mut World) -> Vec<&'static str> {
    let mut fresh = services(&w.files, &w.embedder);
    let done = fresh.graph_load(&ctx(), "/work/g", None).unwrap();
    w.svc = fresh;
    done.notes.iter().map(|p| p.code).collect()
}

#[test]
fn bumping_the_whole_node_version_re_embeds_only_whole_node_vectors() {
    let mut w = world();
    let ids: Vec<Id> = (0..3)
        .map(|i| add(&mut w, &format!("Node {i}"), "A note.", "Some impact"))
        .collect();
    run(&mut w);
    // As if an older umx (whole-node form v1) wrote these shards from text that is not today's.
    for id in &ids {
        edit_shard(&w, *id, |ls| {
            ls.into_iter()
                .map(|l| match l {
                    ShardLine::Vector(mut v) if v.field == vectors::whole_node_field() => {
                        v.field = "@node.v1".into();
                        v.hash = TextHash::of("an older form of the whole node");
                        ShardLine::Vector(v)
                    }
                    other => other,
                })
                .collect()
        });
    }
    let (_, texts, _) = counts(&w);
    reload(&mut w);
    run(&mut w);
    assert_eq!(
        w.embedder.texts_embedded() - texts,
        3,
        "one whole-node text per node"
    );
    for id in ids {
        assert_eq!(fields_of(&w, id), ["@node.v2", "impact", "note", "title"]);
    }
}

/// Renames each whole-node line of `ids` to `@node.v1`, keeping its hash and values: a graph embedded by a umx
/// before D56 (form 1), whose texts held no field that is now data.
fn as_form_one(w: &World, ids: &[Id]) {
    for id in ids {
        edit_shard(w, *id, |ls| {
            ls.into_iter()
                .map(|l| match l {
                    ShardLine::Vector(mut v) if v.field == vectors::whole_node_field() => {
                        v.field = "@node.v1".into();
                        ShardLine::Vector(v)
                    }
                    other => other,
                })
                .collect()
        });
    }
}

/// Sets graph.json's `embedding.text_form` (adding the section, as a graph embedded by a recorded model has).
fn set_graph_json_text_form(w: &World, form: u32) {
    let bytes = w
        .files
        .read_ref(&dir(), &FileRef::Graph(umx_core::codec::GraphFile::Graph))
        .unwrap()
        .unwrap();
    let mut v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    v["embedding"] =
        serde_json::json!({ "key": key().to_string(), "text_form": form, "policy": "open" });
    v["umx"] = serde_json::json!(if form >= 2 { 4 } else { 2 });
    let out = serde_json::to_vec_pretty(&v).unwrap();
    w.files
        .write_set(
            &dir(),
            &[Change {
                file: FileRef::Graph(umx_core::codec::GraphFile::Graph),
                bytes: Some(&out),
            }],
        )
        .unwrap();
}

fn graph_json(w: &World) -> serde_json::Value {
    let bytes = w
        .files
        .read_ref(&dir(), &FileRef::Graph(umx_core::codec::GraphFile::Graph))
        .unwrap()
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
fn a_graph_made_before_field_roles_with_no_data_field_loads_with_zero_pending_and_zero_embeds() {
    let mut w = world();
    let ids: Vec<Id> = (0..3)
        .map(|i| add(&mut w, &format!("Node {i}"), "A note.", "Some impact"))
        .collect();
    run(&mut w);
    as_form_one(&w, &ids);
    set_graph_json_text_form(&w, 1);
    let before = counts(&w);
    let shards: Vec<Vec<ShardLine>> = ids.iter().map(|id| lines(&w, *id)).collect();
    let graph_before = graph_json(&w);
    let warnings = reload(&mut w);
    assert!(warnings.is_empty(), "{warnings:?}");
    let loaded = w.svc.graph_list();
    assert_eq!(loaded[0].vectors_pending, Some(0));
    run(&mut w);
    assert_eq!(counts(&w), before, "no embed call, no shard write");
    let after: Vec<Vec<ShardLine>> = ids.iter().map(|id| lines(&w, *id)).collect();
    assert_eq!(
        after, shards,
        "load writes nothing: the v1 lines stay as they are"
    );
    assert_eq!(graph_json(&w), graph_before, "never written on load");
    // The carried v1 vectors are the nodes' whole-node rows.
    for id in &ids {
        assert!(blocks(&w, *id).iter().any(|(f, _)| f == "@node.v2"), "{id}");
    }
    // The next graph.json write records text form 2, and the format that goes with it.
    w.svc.ontology_add_status(&ctx(), "parked", None).unwrap();
    let v = graph_json(&w);
    assert_eq!(v["embedding"]["text_form"], 2);
    assert_eq!(v["umx"], umx_core::model::FORMAT_FIELD_ROLES);
    // A node's shard written for another reason takes the current name for its carried line, with no embed call.
    w.svc
        .node_edit(
            &ctx(),
            &ids[0].to_text(),
            NodeEditRequest {
                note: Some("A new note.".into()),
                ..Default::default()
            },
        )
        .unwrap();
    run(&mut w);
    assert_eq!(
        fields_of(&w, ids[0]),
        ["@node.v2", "impact", "note", "title"]
    );
}

/// A world whose `problem` type also has `file:string`, still content, and nodes with a file.
fn world_with_file() -> (World, Vec<Id>) {
    let mut w = world();
    w.svc
        .ontology_add_field(&ctx(), "problem", "file:string", None)
        .unwrap();
    let ids: Vec<Id> = (0..3)
        .map(|i| {
            let id = add(&mut w, &format!("Node {i}"), "A note.", "Some impact");
            w.svc
                .node_edit(
                    &ctx(),
                    &id.to_text(),
                    NodeEditRequest {
                        sets: vec![("file".into(), SetValue::Str(format!("src/n{i}.rs")))],
                        ..Default::default()
                    },
                )
                .unwrap();
            id
        })
        .collect();
    run(&mut w);
    (w, ids)
}

#[test]
fn a_v1_vector_made_with_a_now_data_field_is_pending_and_outdated_and_only_reembed_remakes_it() {
    let (mut w, ids) = world_with_file();
    // Committed by a umx before D56 (form 1), then the field is declared data in graph.json (a pull).
    as_form_one(&w, &ids);
    let plain = add(&mut w, "No file", "A note.", "Some impact");
    run(&mut w);
    as_form_one(&w, &[plain]);
    set_graph_json_text_form(&w, 1);
    let mut v = graph_json(&w);
    let fields = v["ontology"]["types"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|t| t["name"] == "problem")
        .unwrap()["fields"]
        .as_array_mut()
        .unwrap();
    for f in fields.iter_mut() {
        if f["name"] == "file" {
            f["role"] = serde_json::json!("data");
        }
    }
    v["umx"] = serde_json::json!(4);
    w.files
        .write_set(
            &dir(),
            &[Change {
                file: FileRef::Graph(umx_core::codec::GraphFile::Graph),
                bytes: Some(&serde_json::to_vec_pretty(&v).unwrap()),
            }],
        )
        .unwrap();
    let before = counts(&w);
    let shards: Vec<Vec<ShardLine>> = ids.iter().map(|id| lines(&w, *id)).collect();
    let warnings = reload(&mut w);
    assert_eq!(warnings, ["text_form_outdated"]);
    // Exactly the nodes whose v1 text held the file: the node with no file carries its v1 line.
    assert_eq!(w.svc.graph_list()[0].vectors_pending, Some(ids.len()));
    run(&mut w);
    assert_eq!(counts(&w), before, "nothing remakes them passively");
    let after: Vec<Vec<ShardLine>> = ids.iter().map(|id| lines(&w, *id)).collect();
    assert_eq!(after, shards, "kept as they are, and still searched");
    for id in &ids {
        assert!(blocks(&w, *id).iter().any(|(f, _)| f == "@node.v2"), "{id}");
    }
    // The graph stays at form 1 while nodes are outdated, whatever else is written.
    w.svc.ontology_add_status(&ctx(), "parked", None).unwrap();
    assert_eq!(graph_json(&w)["embedding"]["text_form"], 1);
    // reembed remakes only their whole-node vectors and drops the data lines.
    let plan = w.svc.embeddings_reembed(&ctx(), true, None).unwrap().value;
    assert_eq!((plan.nodes, plan.texts), (ids.len(), ids.len()));
    w.svc.embeddings_reembed(&ctx(), false, None).unwrap();
    assert_eq!(graph_json(&w)["embedding"]["text_form"], 2);
    let (_, texts, _) = counts(&w);
    run(&mut w);
    assert_eq!(w.embedder.texts_embedded() - texts, ids.len());
    for id in &ids {
        assert_eq!(fields_of(&w, *id), ["@node.v2", "impact", "note", "title"]);
    }
    assert_eq!(w.svc.graph_list()[0].vectors_pending, Some(0));
}

#[test]
fn a_pulled_data_role_on_current_form_vectors_is_outdated_and_never_remade_on_load() {
    // R189-2: vectors made by this build (@node.v2) while file was content; a pull then marks file data, with the
    // nodes and shards unchanged. Same rule as for v1: pending and outdated, nothing remade until a reembed.
    let (mut w, ids) = world_with_file();
    let plain = add(&mut w, "No file", "A note.", "Some impact");
    run(&mut w);
    for id in &ids {
        assert!(fields_of(&w, *id).contains(&"@node.v2".to_owned()));
    }
    let mut v = graph_json(&w);
    let fields = v["ontology"]["types"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|t| t["name"] == "problem")
        .unwrap()["fields"]
        .as_array_mut()
        .unwrap();
    for f in fields.iter_mut() {
        if f["name"] == "file" {
            f["role"] = serde_json::json!("data");
        }
    }
    v["umx"] = serde_json::json!(umx_core::model::FORMAT_FIELD_ROLES);
    w.files
        .write_set(
            &dir(),
            &[Change {
                file: FileRef::Graph(umx_core::codec::GraphFile::Graph),
                bytes: Some(&serde_json::to_vec_pretty(&v).unwrap()),
            }],
        )
        .unwrap();
    let before = counts(&w);
    let shards: Vec<Vec<ShardLine>> = ids.iter().map(|id| lines(&w, *id)).collect();
    let warnings = reload(&mut w);
    assert_eq!(warnings, ["text_form_outdated"]);
    assert_eq!(
        w.svc.graph_list()[0].vectors_pending,
        Some(ids.len()),
        "exactly the nodes with a file, not {}",
        plain.to_text()
    );
    assert!(
        w.svc.take_embed_jobs().is_empty(),
        "nothing queued by the load"
    );
    run(&mut w);
    assert_eq!(counts(&w), before, "zero provider calls, no shard write");
    let after: Vec<Vec<ShardLine>> = ids.iter().map(|id| lines(&w, *id)).collect();
    assert_eq!(after, shards);
    // reembed is the one way they are remade, without the file.
    w.svc.embeddings_reembed(&ctx(), false, None).unwrap();
    let (_, texts, _) = counts(&w);
    run(&mut w);
    assert_eq!(w.embedder.texts_embedded() - texts, ids.len());
    for id in &ids {
        assert_eq!(fields_of(&w, *id), ["@node.v2", "impact", "note", "title"]);
    }
    assert_eq!(w.svc.graph_list()[0].vectors_pending, Some(0));
}

#[test]
fn setting_a_field_to_data_makes_exactly_its_nodes_pending_and_the_worker_remakes_their_whole_node_vectors()
 {
    let (mut w, ids) = world_with_file();
    let other = add(&mut w, "No file", "A note.", "Some impact");
    run(&mut w);
    for id in &ids {
        assert!(fields_of(&w, *id).contains(&"file".to_owned()));
    }
    let done = w
        .svc
        .ontology_set_field(&ctx(), "problem", "file", "data")
        .unwrap();
    assert_eq!(done.value.affected.len(), ids.len());
    let codes: Vec<&str> = done.notes.iter().map(|p| p.code).collect();
    assert_eq!(codes, ["role_changed"]);
    assert_eq!(w.svc.graph_list()[0].vectors_pending, Some(ids.len()));
    let (calls, texts, _) = counts(&w);
    let other_before = lines(&w, other);
    run(&mut w);
    let (calls_after, texts_after, _) = counts(&w);
    assert_eq!(
        texts_after - texts,
        ids.len(),
        "only their whole-node vectors"
    );
    assert!(calls_after > calls);
    for id in &ids {
        assert_eq!(
            fields_of(&w, *id),
            ["@node.v2", "impact", "note", "title"],
            "data line dropped"
        );
    }
    assert_eq!(lines(&w, other), other_before);
    assert_eq!(w.svc.graph_list()[0].vectors_pending, Some(0));
    // Stating the same role again changes nothing; back to content remakes the field and whole-node vectors.
    let again = w
        .svc
        .ontology_set_field(&ctx(), "problem", "file", "data")
        .unwrap();
    assert!(!again.value.changed && again.notes.is_empty());
    w.svc
        .ontology_set_field(&ctx(), "problem", "file", "content")
        .unwrap();
    let (_, texts, _) = counts(&w);
    run(&mut w);
    assert_eq!(w.embedder.texts_embedded() - texts, 2 * ids.len());
    for id in &ids {
        assert!(fields_of(&w, *id).contains(&"file".to_owned()));
    }
}

#[test]
fn an_identical_field_add_compares_the_role_and_refuses_another_one() {
    // F71 with D56 (security N1): adding a field that is already there the same way is unchanged; the same kind with
    // another role is refused whole, never a silent role change (only ontology set field changes a role).
    let mut w = world();
    w.svc
        .ontology_add_field_as(&ctx(), "problem", "file:string", None, Some("data"))
        .unwrap();
    let again = w
        .svc
        .ontology_add_field_as(&ctx(), "problem", "file:string", None, Some("data"))
        .unwrap();
    assert!(again.value.unchanged);
    let before = graph_json(&w);
    let p = w
        .svc
        .ontology_add_field(&ctx(), "problem", "file:string", None)
        .unwrap_err();
    assert_eq!(
        (p.code, p.slot("role").as_deref()),
        ("field_role_conflict", Some("data"))
    );
    assert_eq!(graph_json(&w), before, "nothing written");
    // Several types: per type, all or nothing.
    w.svc
        .ontology_add_field(&ctx(), "issue", "file:string", None)
        .unwrap();
    let before_many = graph_json(&w);
    let types = vec!["problem".to_string(), "issue".to_string()];
    let errors = w
        .svc
        .ontology_add_field_many_as(&ctx(), &types, false, "file:string", None, Some("data"))
        .unwrap_err();
    let codes: Vec<(&str, Option<String>)> =
        errors.iter().map(|p| (p.code, p.slot("type"))).collect();
    assert_eq!(codes, [("field_role_conflict", Some("issue".into()))]);
    assert_eq!(graph_json(&w), before_many, "nothing written");
    let same = w
        .svc
        .ontology_add_field_many_as(
            &ctx(),
            &types[..1],
            false,
            "file:string",
            None,
            Some("data"),
        )
        .unwrap();
    assert_eq!(same.value.unchanged, ["problem"]);
}

#[test]
fn roles_follow_kind_defaults_and_refuse_content_for_kinds_never_embedded() {
    let mut w = world();
    let refused = w
        .svc
        .ontology_add_field_as(&ctx(), "problem", "size:int", None, Some("content"))
        .unwrap_err();
    assert_eq!(refused.code, "role_not_embeddable");
    let refused = w
        .svc
        .ontology_set_field(&ctx(), "problem", "due", "content")
        .unwrap_err();
    assert_eq!(refused.code, "role_not_embeddable");
    let missing = w
        .svc
        .ontology_set_field(&ctx(), "problem", "nope", "data")
        .unwrap_err();
    assert_eq!(missing.code, "field_not_declared");
    w.svc
        .ontology_add_field_as(&ctx(), "problem", "file:string", None, Some("data"))
        .unwrap();
    let (_, o) = w.svc.ontology_show(&ctx()).unwrap().value;
    let role = |f: &str| {
        o.field_decl("problem", f)
            .unwrap()
            .effective_role()
            .as_str()
    };
    assert_eq!(
        (role("impact"), role("due"), role("file")),
        ("content", "data", "data")
    );
}

#[test]
fn a_stale_vector_stays_until_its_fresh_one_is_written() {
    let mut w = world();
    let id = add(&mut w, "Slow builds", "Builds take an hour.", "Teams wait");
    run(&mut w);
    let stale = TextHash::of("an older title");
    edit_shard(&w, id, |ls| {
        ls.into_iter()
            .map(|l| match l {
                ShardLine::Vector(mut v) if v.field == "title" => {
                    v.hash = stale;
                    ShardLine::Vector(v)
                }
                other => other,
            })
            .collect()
    });
    let (_, texts, _) = counts(&w);
    assert_eq!(reload(&mut w), Vec::<&str>::new());
    let title = |w: &World| {
        lines(w, id)
            .iter()
            .filter_map(ShardLine::vector)
            .find(|v| v.field == "title")
            .unwrap()
            .hash
    };
    assert_eq!(
        title(&w),
        stale,
        "loaded and still on disk until re-embedded"
    );
    run(&mut w);
    assert_eq!(title(&w), TextHash::of("title: none | text: Slow builds"));
    assert_eq!(w.embedder.texts_embedded() - texts, 1);
}

/// What the graph's vector blocks hold for `id`, as (shard field, values): `@node.v1` for the whole-node row.
fn blocks(w: &World, id: Id) -> Vec<(String, Vec<i8>)> {
    let g = w.svc.loaded_graph("demo").unwrap();
    let (space, ix) = (key().to_string(), g.node_ix(id).unwrap());
    let mut out: Vec<(String, Vec<i8>)> = g
        .field_blocks(&space)
        .filter_map(|f| {
            Some((
                f.field().to_owned(),
                g.vector(&space, ix, Some(f.field()))?.to_vec(),
            ))
        })
        .collect();
    out.extend(
        g.vector(&space, ix, None)
            .map(|v| ("@node.v2".to_owned(), v.to_vec())),
    );
    out.sort();
    out
}

/// The current-version vector lines of `id` in its shard, as `blocks` gives them.
fn shard_vectors(w: &World, id: Id) -> Vec<(String, Vec<i8>)> {
    lines(w, id)
        .iter()
        .filter_map(ShardLine::vector)
        .filter(|v| v.node == id && v.field != "@node.v0")
        .map(|v| (v.field.clone(), v.values.clone()))
        .collect()
}

/// ws004 s13: the blocks hold what the shard holds, after a load (one read of each shard) and after every apply;
/// a stale vector is loaded (req-75), an older whole-node version and an orphan are not, and a repeat keeps the
/// first line as repair does.
#[test]
fn the_vector_blocks_follow_the_shards_through_load_and_apply() {
    let mut w = world();
    let id = add(&mut w, "Slow builds", "Builds take an hour.", "Teams wait");
    assert!(
        blocks(&w, id).is_empty(),
        "nothing until a vector is applied"
    );
    run(&mut w);
    assert_eq!(blocks(&w, id).len(), 4);
    assert_eq!(blocks(&w, id), shard_vectors(&w, id));
    let edit = |w: &mut World, impact: &str| {
        w.svc
            .node_edit(
                &ctx(),
                &id.to_text(),
                NodeEditRequest {
                    sets: vec![("impact".into(), SetValue::Str(impact.into()))],
                    ..Default::default()
                },
            )
            .unwrap();
    };
    edit(&mut w, "Teams wait a day");
    let before = blocks(&w, id);
    run(&mut w);
    assert_ne!(blocks(&w, id), before, "the fresh vectors replaced the old");
    assert_eq!(blocks(&w, id), shard_vectors(&w, id));

    // On disk: a stale title, an older whole-node version, an orphan, and a repeat of `note` after the first.
    let gone = Id(id.0 + (1 << 10));
    edit_shard(&w, id, |ls| {
        let mut out = Vec::new();
        for l in ls {
            if let ShardLine::Vector(v) = &l {
                if v.field == "title" {
                    let mut stale = v.clone();
                    stale.hash = TextHash::of("an older title");
                    stale.values[0] = stale.values[0].wrapping_add(1);
                    out.push(ShardLine::Vector(stale));
                    continue;
                }
                if v.field == "note" {
                    let mut repeat = v.clone();
                    repeat.hash = TextHash([0xff; 16]);
                    repeat.values = vec![1; v.values.len()];
                    out.push(ShardLine::Vector(repeat));
                }
                if v.field == "@node.v2" {
                    let mut old = v.clone();
                    old.field = "@node.v0".into();
                    old.values = vec![2; v.values.len()];
                    out.push(ShardLine::Vector(old));
                    let mut orphan = v.clone();
                    orphan.node = gone;
                    out.push(ShardLine::Vector(orphan));
                }
            }
            out.push(l);
        }
        out
    });
    reload(&mut w);
    let held = blocks(&w, id);
    let on_disk: Vec<(String, Vec<i8>)> = lines(&w, id)
        .iter()
        .filter_map(ShardLine::vector)
        .filter(|v| v.node == id && v.field != "@node.v0" && v.hash != TextHash([0xff; 16]))
        .map(|v| (v.field.clone(), v.values.clone()))
        .collect();
    assert_eq!(
        held, on_disk,
        "the stale title is held, the first note kept, no v0, no orphan"
    );
    assert_eq!(held.len(), 4);
    assert!(w.svc.loaded_graph("demo").unwrap().node_ix(gone).is_none());
    // The re-embed rewrites the title, tidies the repeat and the leftover; the blocks follow.
    run(&mut w);
    assert_eq!(blocks(&w, id), shard_vectors(&w, id));
}

#[test]
fn a_vector_for_an_unknown_node_is_kept_and_warned_by_id_and_line() {
    let mut w = world();
    let id = add(&mut w, "Slow builds", "Builds take an hour.", "Teams wait");
    run(&mut w);
    // A node another branch deleted, in the same shard (same last two characters).
    let gone = Id(id.0 + (1 << 10));
    assert_eq!(shard(gone), shard(id));
    let mut orphan = lines(&w, id)[0].vector().unwrap().clone();
    orphan.node = gone;
    edit_shard(&w, id, |mut ls| {
        ls.push(ShardLine::Vector(orphan.clone()));
        ls
    });
    let warnings = {
        let mut fresh = services(&w.files, &w.embedder);
        let done = fresh.graph_load(&ctx(), "/work/g", None).unwrap();
        w.svc = fresh;
        done.notes
    };
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].code, "vector_orphan");
    assert_eq!(warnings[0].slot("node"), Some(gone.to_text()));
    assert_eq!(
        warnings[0].slot("n"),
        Some("5".into()),
        "after the node's four lines"
    );
    // A later write of that shard keeps the line.
    w.svc
        .node_edit(
            &ctx(),
            &id.to_text(),
            NodeEditRequest {
                title: Some("Slower builds".into()),
                ..Default::default()
            },
        )
        .unwrap();
    run(&mut w);
    assert!(lines(&w, id).contains(&ShardLine::Vector(orphan)));
}

#[test]
fn repair_keeps_the_first_of_two_equal_keys_and_re_embeds_a_conflict() {
    let mut w = world();
    let a = add(&mut w, "Slow builds", "Builds take an hour.", "Teams wait");
    let b = add(&mut w, "Flaky tests", "Retries hide it.", "Trust drops");
    run(&mut w);
    // Branch 1 and branch 2 both embedded a's title (same text, different bytes): git keeps both lines.
    let title = |w: &World, id: Id| {
        lines(w, id)
            .iter()
            .filter_map(ShardLine::vector)
            .find(|v| v.node == id && v.field == "title")
            .unwrap()
            .clone()
    };
    let mut twin = title(&w, a);
    twin.values[0] = twin.values[0].wrapping_add(1);
    edit_shard(&w, a, |mut ls| {
        ls.push(ShardLine::Vector(twin.clone()));
        ls
    });
    // Both branches edited b's note and embedded it: neither hash is b's note now.
    let mut left = title(&w, b);
    left.field = "note".into();
    left.hash = TextHash::of("left");
    let mut right = left.clone();
    right.hash = TextHash::of("right");
    edit_shard(&w, b, |ls| {
        ls.into_iter()
            .filter(|l| !l.vector().is_some_and(|v| v.node == b && v.field == "note"))
            .chain([
                ShardLine::Vector(left.clone()),
                ShardLine::Vector(right.clone()),
            ])
            .collect()
    });
    let first = {
        let mut both = [title(&w, a).encode(), twin.encode()];
        both.sort();
        both[0].clone()
    };
    let (calls, texts, _) = counts(&w);
    let done = w.svc.embeddings_repair(&ctx()).unwrap().value;
    assert_eq!((done.tidied, done.queued, done.orphans), (1, 1, 0));
    run(&mut w);
    assert_eq!(
        title(&w, a).encode(),
        first,
        "the first in sort order is kept"
    );
    assert_eq!(fields_of(&w, a), ["@node.v2", "impact", "note", "title"]);
    assert_eq!(fields_of(&w, b), ["@node.v2", "impact", "note", "title"]);
    assert_eq!(
        (
            w.embedder.calls() - calls,
            w.embedder.texts_embedded() - texts
        ),
        (1, 1),
        "only b's note is embedded"
    );
}

#[test]
fn repair_without_an_embedder_says_so() {
    let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let mut svc = Services::new(Ports {
        sources: std::sync::Arc::new(umx_fakes::FakeSourceReader::open(
            umx_core::ports::GraphDir::parse("/").unwrap(),
        )),
        files: files.clone(),
        vector_cache: files,
        vector_cache_root: GraphDir::parse("/work/cache").unwrap(),
        clock: Arc::new(FakeClock::default()),
        ids: Arc::new(FakeIds::new()),
        state: Arc::new(FakeLocalState::new()),
        scopes: Arc::new(FakeLocalState::new()),
        registry: Arc::new(FakeLocalState::new()),
        allows: Arc::new(FakeLocalState::new()),
        lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
    });
    svc.graph_new(&ctx(), "/work/g", "demo", None, false, None)
        .unwrap();
    assert_eq!(
        svc.embeddings_repair(&ctx()).unwrap_err().code,
        "embedder_not_configured"
    );
    assert!(
        svc.take_embed_jobs().is_empty(),
        "no embedder, nothing queued"
    );
}

/// Answers 429 for its first `storm` calls, then embeds with the fake.
struct Storm {
    inner: FakeEmbedder,
    storm: usize,
    seen: AtomicUsize,
}

impl Embedder for Storm {
    fn credential(&self) -> Option<umx_core::ports::Credential> {
        None
    }
    fn ready(&self) -> Result<(), EmbedError> {
        Ok(())
    }
    fn adapter_id(&self) -> &'static str {
        "test.storm"
    }
    fn embed(&self, texts: &[String], kind: TextKind) -> Result<Embedded, EmbedError> {
        if self.seen.fetch_add(1, Ordering::SeqCst) < self.storm {
            return Err(EmbedError::RateLimited {
                retry_after_ms: Some(500),
            });
        }
        self.inner.embed(texts, kind)
    }
    fn key(&self) -> ModelKey {
        self.inner.key()
    }
    fn concurrency_ceiling(&self) -> usize {
        1
    }
    fn max_input_tokens(&self) -> usize {
        self.inner.max_input_tokens()
    }
    fn locality(&self) -> Locality {
        Locality::Hosted
    }
}

#[test]
fn a_429_storm_halves_the_rate_once_and_fails_no_job() {
    let mut w = world();
    let ids: Vec<Id> = (0..6)
        .map(|i| add(&mut w, &format!("Node {i}"), "A note.", "Some impact"))
        .collect();
    let storm = Storm {
        inner: FakeEmbedder::new(key()),
        storm: 3,
        seen: AtomicUsize::new(0),
    };
    let mut pacer = Pacer::new(1_000_000);
    let (mut now, mut rates) = (0u64, Vec::new());
    let mut queue: Vec<_> = w.svc.take_embed_jobs();
    while !queue.is_empty() {
        let planned: Vec<Planned> = queue
            .drain(..)
            .filter_map(|j| w.svc.embed_plan(&j).unwrap())
            .collect();
        for batch in batches(planned) {
            now += pacer.wait_ms(now);
            match step(&storm, &batch, &mut pacer, now) {
                Step::Done { vectors: v, .. } => {
                    w.svc.embed_apply(&v).unwrap();
                }
                Step::Retry { jobs, wait_ms } => {
                    rates.push(pacer.rate());
                    assert!(wait_ms >= 500, "waits at least the provider's Retry-After");
                    queue.extend(jobs);
                }
                Step::Failed { error, .. } => panic!("a job failed: {error:?}"),
            }
        }
    }
    // One storm is one 429 episode: the rate halves once (embeddings-expert, ws004 s25), and successes alone do not
    // win it back; a quiet minute does.
    assert_eq!(rates, [Some(500_000); 3]);
    assert_eq!(pacer.rate(), Some(500_000));
    for id in ids {
        assert_eq!(fields_of(&w, id), ["@node.v2", "impact", "note", "title"]);
    }
}

#[test]
fn a_vector_for_text_that_changed_while_it_was_embedding_is_not_written() {
    let mut w = world();
    let id = add(&mut w, "Slow builds", "Builds take an hour.", "Teams wait");
    let jobs = w.svc.take_embed_jobs();
    let planned: Vec<Planned> = jobs
        .iter()
        .filter_map(|j| w.svc.embed_plan(j).unwrap())
        .collect();
    let Step::Done { vectors: old, .. } =
        step(&*w.embedder.clone(), &planned, &mut Pacer::default(), 0)
    else {
        panic!("the fake embeds");
    };
    // The title changes while the provider works on the old one.
    w.svc
        .node_edit(
            &ctx(),
            &id.to_text(),
            NodeEditRequest {
                title: Some("Slower builds".into()),
                ..Default::default()
            },
        )
        .unwrap();
    w.svc.embed_apply(&old).unwrap();
    let fields: Vec<String> = fields_of(&w, id);
    assert_eq!(fields, ["impact", "note"], "title and whole node held back");
    run(&mut w);
    assert_eq!(fields_of(&w, id), ["@node.v2", "impact", "note", "title"]);
    let title = lines(&w, id)
        .iter()
        .filter_map(ShardLine::vector)
        .find(|v| v.field == "title")
        .unwrap()
        .hash;
    assert_eq!(title, TextHash::of("title: none | text: Slower builds"));
}

#[test]
fn a_graph_that_records_another_model_is_never_embedded_by_this_one() {
    let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let embedder = Arc::new(FakeEmbedder::new(key()));
    let mut svc = services(&files, &embedder);
    let other = umx_core::GraphEmbedding {
        key: Some(ModelKey::new("other", 8).unwrap()),
        text_form: 1,
        policy: Default::default(),
        similarity: None,
        extra: Default::default(),
    };
    svc.graph_new(&ctx(), "/work/g", "demo", None, false, Some(other))
        .unwrap();
    let mut w = World {
        files,
        embedder,
        svc,
    };
    let id = add(&mut w, "Slow builds", "Builds take an hour.", "Teams wait");
    assert!(w.svc.take_embed_jobs().is_empty(), "nothing queued (D32)");
    let warnings = reload(&mut w);
    assert!(warnings.contains(&"embedder_mismatch"), "{warnings:?}");
    run(&mut w);
    assert_eq!(counts(&w), (0, 0, 0));
    assert!(lines(&w, id).is_empty());
}

/// R81-2 (embeddings-expert ruling): a graph holds the vectors of the space its graph.json records, whatever
/// embedder this machine has, so node-to-node similarity works from stored vectors (req-75). Edited nodes stay
/// pending: never embedded by the other model.
#[test]
fn the_blocks_hold_the_recorded_space_whatever_embedder_is_configured() {
    let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let a = Arc::new(FakeEmbedder::new(key()));
    let recorded = umx_core::GraphEmbedding {
        key: Some(key()),
        text_form: 1,
        policy: Default::default(),
        similarity: None,
        extra: Default::default(),
    };
    let mut svc = services(&files, &a);
    svc.graph_new(&ctx(), "/work/g", "demo", None, false, Some(recorded))
        .unwrap();
    svc.ontology_add_field(&ctx(), "problem", "impact:string", None)
        .unwrap();
    let mut w = World {
        files,
        embedder: a,
        svc,
    };
    let one = add(&mut w, "Slow builds", "Builds take an hour.", "Teams wait");
    let two = add(&mut w, "Slower builds", "Builds take a day.", "Teams wait");
    run(&mut w);
    let committed = blocks(&w, one);
    assert_eq!(committed.len(), 4);

    // The same files on a machine whose embedder is another model, and on one with none.
    let b = Arc::new(FakeEmbedder::new(ModelKey::new("other", 8).unwrap()));
    let none = {
        let ports = Ports {
            sources: std::sync::Arc::new(umx_fakes::FakeSourceReader::open(
                umx_core::ports::GraphDir::parse("/").unwrap(),
            )),
            files: w.files.clone(),
            vector_cache: w.files.clone(),
            vector_cache_root: GraphDir::parse("/work/cache").unwrap(),
            clock: Arc::new(FakeClock::default()),
            ids: Arc::new(FakeIds::new()),
            state: Arc::new(FakeLocalState::new()),
            scopes: Arc::new(FakeLocalState::new()),
            registry: Arc::new(FakeLocalState::new()),
            allows: Arc::new(FakeLocalState::new()),
            lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
        };
        Services::new(ports)
    };
    for svc in [services(&w.files, &b), none] {
        w.svc = svc;
        w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
        assert_eq!(
            blocks(&w, one),
            committed,
            "the recorded space is in memory"
        );
        let g = w.svc.loaded_graph("demo").unwrap();
        let space = key().to_string();
        assert!(
            g.whole_block(&ModelKey::new("other", 8).unwrap().to_string())
                .is_none()
        );
        // Node to node: `one`'s whole-node vector finds both nodes in the stored space.
        let whole = g.whole_block(&space).unwrap();
        let query = g.vector(&space, g.node_ix(one).unwrap(), None).unwrap();
        let hits: Vec<Id> =
            umx_core::similarity::top_k(query, whole.values(), whole.inverse_lengths(), 5, |r| {
                whole.has(r)
            })
            .iter()
            .map(|h| g.node_at(whole.node(h.row)).id())
            .collect();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0], one);
        assert!(hits.contains(&two));
    }

    // Under the other model an edit stays pending: nothing embedded, the stored vectors kept.
    w.svc = services(&w.files, &b);
    w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
    w.svc
        .node_edit(
            &ctx(),
            &one.to_text(),
            NodeEditRequest {
                title: Some("Slow builds again".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let embedder: Arc<FakeEmbedder> = b.clone();
    run_with(&mut w, &*embedder);
    assert_eq!(b.calls(), 0, "never embedded with the other model");
    assert_eq!(blocks(&w, one), committed);
}

/// Security: graph.json arrives by `git pull` and its recorded key sizes the in-memory blocks, so a key wider
/// than `ModelKey::MAX_DIMS` is refused as invalid data at load, before any block is sized from it.
#[test]
fn a_graph_json_recording_an_over_wide_key_is_refused_at_load() {
    let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let embedder = Arc::new(FakeEmbedder::new(key()));
    let recorded = umx_core::GraphEmbedding {
        key: Some(key()),
        text_form: 1,
        policy: Default::default(),
        similarity: None,
        extra: Default::default(),
    };
    let mut svc = services(&files, &embedder);
    svc.graph_new(&ctx(), "/work/g", "demo", None, false, Some(recorded))
        .unwrap();
    let graph_json = FileRef::Graph(umx_core::codec::GraphFile::Graph);
    let text = String::from_utf8(files.read_ref(&dir(), &graph_json).unwrap().unwrap()).unwrap();
    assert!(text.contains("\"fake@8\""), "{text}");
    for wide in ["fake@8193", "fake@100000"] {
        let pulled = text.replace("\"fake@8\"", &format!("\"{wide}\""));
        files
            .write_set(
                &dir(),
                &[Change {
                    file: graph_json.clone(),
                    bytes: Some(pulled.as_bytes()),
                }],
            )
            .unwrap();
        let mut fresh = services(&files, &embedder);
        let refused = fresh.graph_load(&ctx(), "/work/g", None).unwrap_err();
        assert_eq!(refused.code, "graph_invalid", "{wide}: {refused:?}");
    }
}

/// Refuses a text holding `POISON` by its index, refuses any text over its cap, and reports a text holding
/// `LONGISH` as truncated; otherwise embeds with the fake.
struct Picky {
    inner: FakeEmbedder,
    max_tokens: usize,
}

impl Embedder for Picky {
    fn credential(&self) -> Option<umx_core::ports::Credential> {
        None
    }
    fn ready(&self) -> Result<(), EmbedError> {
        Ok(())
    }
    fn adapter_id(&self) -> &'static str {
        "test.picky"
    }
    fn embed(&self, texts: &[String], kind: TextKind) -> Result<Embedded, EmbedError> {
        if let Some(index) = texts
            .iter()
            .position(|t| t.contains("POISON") || t.len() > self.max_tokens * 2)
        {
            return Err(EmbedError::InputTooLong { index });
        }
        let mut out = self.inner.embed(texts, kind)?;
        out.truncated = texts
            .iter()
            .enumerate()
            .filter(|(_, t)| t.contains("LONGISH"))
            .map(|(i, _)| i)
            .collect();
        Ok(out)
    }
    fn key(&self) -> ModelKey {
        self.inner.key()
    }
    fn concurrency_ceiling(&self) -> usize {
        1
    }
    fn max_input_tokens(&self) -> usize {
        self.max_tokens
    }
    fn locality(&self) -> Locality {
        Locality::Local
    }
}

fn picky(max_tokens: usize) -> Picky {
    Picky {
        inner: FakeEmbedder::new(key()),
        max_tokens,
    }
}

fn kinds(notes: &[EmbedNote], id: Id) -> Vec<(String, NoteKind)> {
    notes
        .iter()
        .filter(|n| n.job.node == id)
        .map(|n| (n.field.clone(), n.what))
        .collect()
}

#[test]
fn one_refused_text_never_fails_the_rest_of_its_batch() {
    let mut w = world();
    let ids: Vec<Id> = (0..16)
        .map(|i| add(&mut w, &format!("Node {i}"), "A note.", "Some impact"))
        .collect();
    let poisoned = ids[5];
    w.svc
        .node_edit(
            &ctx(),
            &poisoned.to_text(),
            NodeEditRequest {
                note: Some("POISON".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let notes = run_with(&mut w, &picky(10_000));
    for id in &ids {
        if *id != poisoned {
            assert_eq!(fields_of(&w, *id), ["@node.v2", "impact", "note", "title"]);
        }
    }
    // The note and the whole-node text (which holds the note) are refused; its other fields still embed.
    assert_eq!(fields_of(&w, poisoned), ["impact", "title"]);
    assert_eq!(
        kinds(&notes, poisoned),
        [
            ("@node.v2".to_string(), NoteKind::Refused),
            ("note".to_string(), NoteKind::Refused)
        ]
    );
    assert_eq!(notes.len(), 2);
    let shown = notes[0].to_string();
    assert!(
        shown.contains(&poisoned.to_text()) && !shown.contains("POISON"),
        "{shown}"
    );
}

#[test]
fn an_over_cap_text_is_cut_before_sending_and_counts_as_current() {
    let mut w = world();
    let long = "word ".repeat(200);
    let id = add(&mut w, "Slow builds", &long, "Teams wait");
    let notes = run_with(&mut w, &picky(100));
    assert_eq!(fields_of(&w, id), ["@node.v2", "impact", "note", "title"]);
    assert_eq!(
        kinds(&notes, id),
        [
            ("@node.v2".to_string(), NoteKind::Cut { max_tokens: 100 }),
            ("note".to_string(), NoteKind::Cut { max_tokens: 100 })
        ]
    );
    // Keyed by the whole text's hash, so a reload plans nothing.
    reload(&mut w);
    assert!(run_with(&mut w, &picky(100)).is_empty());
    assert!(w.svc.take_embed_jobs().is_empty());
}

#[test]
fn a_text_the_provider_truncates_is_noted() {
    let mut w = world();
    let id = add(&mut w, "LONGISH title", "A note.", "Some impact");
    let notes = run_with(&mut w, &picky(10_000));
    assert_eq!(fields_of(&w, id), ["@node.v2", "impact", "note", "title"]);
    assert_eq!(
        kinds(&notes, id),
        [
            ("@node.v2".to_string(), NoteKind::Truncated),
            ("title".to_string(), NoteKind::Truncated)
        ]
    );
}

// ---------------------------------------------------------------- round 1: secured graphs and corrupt shards

/// A graph made with `policy` and the fake's model key, over the shared fake disk; the fake set to `locality`.
fn world_with(policy: umx_core::Policy, locality: Locality) -> World {
    let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let embedder = Arc::new(FakeEmbedder::new(key()).with_locality(locality));
    let mut svc = services(&files, &embedder);
    svc.graph_new(
        &ctx(),
        "/work/g",
        "demo",
        None,
        false,
        Some(umx_core::GraphEmbedding::new(Some(key()), policy)),
    )
    .unwrap();
    World {
        files,
        embedder,
        svc,
    }
}

fn shards_under(w: &World, dir: &GraphDir) -> Vec<ShardFile> {
    w.files
        .list_shards(dir, &key(), ShardKind::Vectors)
        .unwrap()
}

#[test]
fn a_secured_graph_never_reaches_a_hosted_embedder() {
    let mut w = world_with(umx_core::Policy::SECURED, Locality::Hosted);
    let id = add(&mut w, "Slow builds", "Builds take an hour.", "Teams wait");
    assert!(w.svc.take_embed_jobs().is_empty(), "nothing queued");
    // Even a job handed straight to plan is refused, so the provider is never called.
    let job = umx_services::EmbedJob {
        alias: "demo".into(),
        node: id,
    };
    assert_eq!(w.svc.embed_plan(&job).unwrap(), None);
    assert_eq!(reload(&mut w), ["embedder_not_local_for_secured"]);
    let repaired = w.svc.embeddings_repair(&ctx()).unwrap();
    assert_eq!(repaired.value.queued, 0);
    assert!(
        repaired
            .notes
            .iter()
            .any(|p| p.code == "embedder_not_local_for_secured")
    );
    run(&mut w);
    assert_eq!(w.embedder.calls(), 0, "embed() was never called");
    assert!(shards_under(&w, &dir()).is_empty());
}

#[test]
fn secured_vectors_go_to_the_local_cache_never_the_graph_folder() {
    let mut w = world_with(umx_core::Policy::SECURED, Locality::Local);
    let id = add(&mut w, "Slow builds", "Builds take an hour.", "Teams wait");
    run(&mut w);
    assert_eq!(
        w.embedder.calls(),
        1,
        "a local embedder may embed a secured graph"
    );
    assert!(
        w.files
            .read_ref(&dir(), &FileRef::Shard(shard(id)))
            .unwrap()
            .is_none(),
        "no vector inside the graph folder"
    );
    assert!(shards_under(&w, &dir()).is_empty());
    // The cache folder: <root>/<graph id>/<16 hex of the folder's hash>.
    let graph_id = w.svc.loaded("demo").unwrap().meta.id.to_text();
    let folder_key = TextHash::of("/work/g").to_string();
    let cache = GraphDir::parse(&format!("/work/cache/{graph_id}/{}", &folder_key[..16])).unwrap();
    assert_eq!(shards_under(&w, &cache), vec![shard(id)]);
    // Load and repair read the cache: nothing to embed again.
    assert_eq!(reload(&mut w), Vec::<&str>::new());
    assert!(w.svc.take_embed_jobs().is_empty());
    assert_eq!(w.svc.embeddings_repair(&ctx()).unwrap().value.queued, 0);
}

#[test]
fn a_current_hash_does_not_make_a_wrong_width_vector_current() {
    let mut w = world();
    let id = add(&mut w, "Slow builds", "Builds take an hour.", "Teams wait");
    run(&mut w);
    edit_shard(&w, id, |ls| {
        ls.into_iter()
            .map(|l| match l {
                ShardLine::Vector(mut v) if v.field == "title" => {
                    v.values.clear();
                    ShardLine::Vector(v)
                }
                other => other,
            })
            .collect()
    });
    let (_, texts, _) = counts(&w);
    assert_eq!(reload(&mut w), ["vector_wrong_width"]);
    run(&mut w);
    assert_eq!(
        w.embedder.texts_embedded() - texts,
        1,
        "the title is embedded again"
    );
    let title = lines(&w, id)
        .iter()
        .filter_map(ShardLine::vector)
        .filter(|v| v.field == "title")
        .map(|v| v.values.len())
        .collect::<Vec<_>>();
    assert_eq!(title, [8], "only the good line is left");
}

#[test]
fn vector_warnings_report_physical_line_numbers() {
    let mut w = world();
    let id = add(&mut w, "Slow builds", "Builds take an hour.", "Teams wait");
    put_shard(&w, id, b"\nnot-a-vector\n");
    let mut fresh = services(&w.files, &w.embedder);
    let done = fresh.graph_load(&ctx(), "/work/g", None).unwrap();
    let warning = done
        .notes
        .iter()
        .find(|p| p.code == "vector_line_kept")
        .unwrap();
    assert_eq!(warning.slot("n"), Some("2".into()));
}

#[test]
fn a_shard_scan_failure_is_not_a_clean_successful_load() {
    let root = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("umx-s07a-scan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    let dir = GraphDir::parse(root.to_str().unwrap()).unwrap();
    let files: Arc<dyn GraphFiles> = Arc::new(umx_adapter_fs::FsGraphFiles::new(vec![dir.clone()]));
    let embedder = Arc::new(FakeEmbedder::new(key()));
    let open = |files: &Arc<dyn GraphFiles>| {
        Services::new(Ports {
            sources: std::sync::Arc::new(umx_fakes::FakeSourceReader::open(
                umx_core::ports::GraphDir::parse("/").unwrap(),
            )),
            files: files.clone(),
            vector_cache: files.clone(),
            vector_cache_root: dir.join("cache").unwrap(),
            clock: Arc::new(FakeClock::default()),
            ids: Arc::new(FakeIds::new()),
            state: Arc::new(FakeLocalState::new()),
            scopes: Arc::new(FakeLocalState::new()),
            registry: Arc::new(FakeLocalState::new()),
            allows: Arc::new(FakeLocalState::new()),
            lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
        })
        .with_embedder(embedder.clone())
    };
    let path = root.join("g");
    let path = path.to_str().unwrap();
    let mut first = open(&files);
    first
        .graph_new(&ctx(), path, "demo", None, false, None)
        .unwrap();
    first
        .node_add(
            &ctx(),
            NodeAddRequest {
                kind: "problem".into(),
                title: "Synthetic fixture".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let mut positive = open(&files);
    positive.graph_load(&ctx(), path, None).unwrap();
    assert_eq!(
        positive.take_embed_jobs().len(),
        1,
        "the instrument can see a missing vector"
    );
    std::fs::create_dir_all(root.join("g/embeddings/fake@8/00.tsv")).unwrap();
    let mut blocked = open(&files);
    let done = blocked.graph_load(&ctx(), path, None).unwrap();
    let codes: Vec<&str> = done.notes.iter().map(|p| p.code).collect();
    assert_eq!(codes, ["vectors_unreadable"]);
    let _ = std::fs::remove_dir_all(&root);
}

/// A hosted embedder that fails the test if anything reaches it.
struct MustNotCall;

impl Embedder for MustNotCall {
    fn credential(&self) -> Option<umx_core::ports::Credential> {
        None
    }
    fn ready(&self) -> Result<(), EmbedError> {
        Ok(())
    }
    fn adapter_id(&self) -> &'static str {
        "test.must-not-call"
    }
    fn embed(&self, _: &[String], _: TextKind) -> Result<Embedded, EmbedError> {
        panic!("a secured graph's text reached a hosted embedder");
    }
    fn key(&self) -> ModelKey {
        key()
    }
    fn concurrency_ceiling(&self) -> usize {
        1
    }
    fn max_input_tokens(&self) -> usize {
        512
    }
    fn locality(&self) -> Locality {
        Locality::Hosted
    }
}

#[test]
fn neither_a_planned_nor_a_query_embed_of_a_secured_graph_calls_a_hosted_embedder() {
    let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let mut svc = Services::new(Ports {
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
    })
    .with_embedder(Arc::new(MustNotCall));
    svc.graph_new(
        &ctx(),
        "/work/g",
        "demo",
        None,
        false,
        Some(umx_core::GraphEmbedding::new(
            Some(key()),
            umx_core::Policy::SECURED,
        )),
    )
    .unwrap();
    let id = svc
        .node_add(
            &ctx(),
            NodeAddRequest {
                kind: "problem".into(),
                title: "Slow builds".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .value
        .node
        .id;
    let job = umx_services::EmbedJob {
        alias: "demo".into(),
        node: id,
    };
    let planned: Vec<Planned> = svc
        .take_embed_jobs()
        .into_iter()
        .chain([job.clone()])
        .filter_map(|j| svc.embed_plan(&j).unwrap())
        .collect();
    assert!(planned.is_empty());
    assert_eq!(svc.embed_query("demo", "slow builds").unwrap(), None);
    // apply of vectors made elsewhere writes nothing for it either.
    let stray = umx_services::Vectors {
        job,
        key: None,
        vectors: vec![(
            "title".into(),
            TextHash::of("title: none | text: Slow builds"),
            vec![1.0; 8],
        )],
    };
    assert_eq!(svc.embed_apply(&[stray]).unwrap().0.shards_written, 0);
    assert!(
        files
            .list_shards(&dir(), &key(), ShardKind::Vectors)
            .unwrap()
            .is_empty()
    );
    assert!(files.list_vector_spaces(&dir()).unwrap().is_empty());
}

struct ProvenanceGuard(Arc<FakeEmbedder>);

impl Embedder for ProvenanceGuard {
    fn credential(&self) -> Option<umx_core::ports::Credential> {
        None
    }
    fn ready(&self) -> Result<(), EmbedError> {
        Ok(())
    }
    fn adapter_id(&self) -> &'static str {
        "test.provenance-guard"
    }
    fn embed(&self, texts: &[String], kind: TextKind) -> Result<Embedded, EmbedError> {
        assert!(
            texts
                .iter()
                .all(|t| !t.contains("private-provenance-marker")),
            "provenance text reached a hosted embedder"
        );
        self.0.embed(texts, kind)
    }
    fn key(&self) -> ModelKey {
        self.0.key()
    }
    fn concurrency_ceiling(&self) -> usize {
        self.0.concurrency_ceiling()
    }
    fn max_input_tokens(&self) -> usize {
        self.0.max_input_tokens()
    }
    fn locality(&self) -> Locality {
        Locality::Hosted
    }
}

#[test]
fn provenance_is_excluded_at_load_queue_plan_send_and_apply_with_a_content_control() {
    let mut w = world();
    let content = add(&mut w, "Visible content", "A note", "Normal impact");
    let hidden = add(
        &mut w,
        "private-provenance-marker",
        "private-provenance-marker",
        "private-provenance-marker",
    );
    let user_agent = w
        .svc
        .node_add(
            &ctx(),
            NodeAddRequest {
                kind: "agent".into(),
                title: "User-made agent content".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .value
        .node
        .id;
    let job = umx_services::EmbedJob {
        alias: "demo".into(),
        node: hidden,
    };
    let before = w.svc.embed_plan(&job).unwrap().unwrap();
    let mut records: Vec<_> = w
        .svc
        .loaded_graph("demo")
        .unwrap()
        .nodes()
        .map(|n| n.to_node())
        .collect();
    for n in &mut records {
        if n.id == hidden {
            n.kind = "agent".into();
            n.fields
                .insert("prov_key".into(), serde_json::json!("agent/pij/pij-seat"));
        } else if n.id == content {
            n.fields.insert(
                "prov_key".into(),
                serde_json::json!("private-provenance-marker"),
            );
        }
    }
    w.files
        .write_set(
            &dir(),
            &[Change {
                file: FileRef::Graph(umx_core::codec::GraphFile::Nodes),
                bytes: Some(&umx_core::codec::encode_nodes(&records)),
            }],
        )
        .unwrap();
    let imported = vectors::encode_shard(&[ShardLine::Vector(vectors::VectorLine {
        node: hidden,
        field: vectors::whole_node_field(),
        hash: TextHash::of("private-provenance-marker"),
        values: vec![1; 8],
    })]);
    put_shard(&w, hidden, &imported);
    let guard: Arc<dyn Embedder> = Arc::new(ProvenanceGuard(w.embedder.clone()));
    w.svc.set_embedder(Some(guard.clone()));
    w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
    assert!(w.svc.embed_plan(&job).unwrap().is_none());
    assert!(!w.svc.graph().node_ids("demo").contains(&hidden));
    assert!(
        w.svc
            .embedding()
            .needs(w.svc.graph(), "demo", &key())
            .unwrap()
            .iter()
            .all(|p| p.job.node != hidden)
    );
    let jobs = w.svc.take_embed_jobs();
    assert!(jobs.iter().all(|j| j.node != hidden));
    assert!(jobs.iter().any(|j| j.node == content));
    assert!(jobs.iter().any(|j| j.node == user_agent));
    let mut planned: Vec<_> = jobs
        .iter()
        .filter_map(|j| w.svc.embed_plan(j).unwrap())
        .collect();
    planned.push(before);
    let batch = w
        .svc
        .embedding()
        .still_sendable(w.svc.graph(), &guard, planned);
    assert!(batch.iter().all(|p| p.job.node != hidden));
    let Step::Done { vectors, .. } = step(&*guard, &batch, &mut Pacer::default(), 0) else {
        panic!("ordinary content must embed");
    };
    assert!(w.embedder.calls() > 0, "positive hosted-call control");
    w.svc.embed_apply(&vectors).unwrap();
    let after_content = w
        .files
        .read_ref(&dir(), &FileRef::Shard(shard(hidden)))
        .unwrap()
        .unwrap();
    assert_eq!(
        vectors::encode_shard(
            &vectors::decode_shard(&after_content)
                .into_iter()
                .filter(|line| line.vector().is_some_and(|v| v.node == hidden))
                .collect::<Vec<_>>()
        ),
        imported
    );
    let written = w
        .svc
        .embed_apply(&[umx_services::Vectors {
            job,
            key: Some(key()),
            vectors: vec![(
                vectors::whole_node_field(),
                TextHash::of("private-provenance-marker"),
                vec![1.0; 8],
            )],
        }])
        .unwrap()
        .0;
    assert_eq!(written.shards_written, 0);
    assert_eq!(
        w.files
            .read_ref(&dir(), &FileRef::Shard(shard(hidden)))
            .unwrap()
            .unwrap(),
        after_content,
        "imported provenance vectors remain storage data"
    );
}

#[test]
fn a_query_embed_of_an_open_graph_uses_the_query_kind() {
    let mut w = world();
    add(&mut w, "Slow builds", "Builds take an hour.", "Teams wait");
    let calls = w.embedder.calls();
    let v = w.svc.embed_query("demo", "slow builds").unwrap().unwrap();
    assert_eq!((v.len(), w.embedder.calls()), (8, calls + 1));
    assert_eq!(w.svc.embed_query("nope", "slow builds").unwrap(), None);
}

// ws004 s06 part B (req-75): pending vectors, reembed, and one policy read per batch.

/// A graph at /work/g that records `recorded` as its model, served by an embedder of `key()` with `locality`.
fn pulled(recorded: &str, locality: Locality) -> (World, Arc<FakeLocalState>) {
    let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let embedder = Arc::new(FakeEmbedder::new(key()).with_locality(locality));
    let state = Arc::new(FakeLocalState::new());
    let mut svc = Services::new(Ports {
        sources: std::sync::Arc::new(umx_fakes::FakeSourceReader::open(
            umx_core::ports::GraphDir::parse("/").unwrap(),
        )),
        files: files.clone(),
        vector_cache: files.clone(),
        vector_cache_root: GraphDir::parse("/work/cache").unwrap(),
        clock: Arc::new(FakeClock::default()),
        ids: Arc::new(FakeIds::new()),
        state: state.clone(),
        scopes: Arc::new(FakeLocalState::new()),
        registry: Arc::new(FakeLocalState::new()),
        allows: Arc::new(FakeLocalState::new()),
        lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
    })
    .with_embedder(embedder.clone());
    let section = umx_core::GraphEmbedding::new(
        Some(ModelKey::parse(recorded).unwrap()),
        umx_core::Policy::OPEN,
    );
    svc.graph_new(&ctx(), "/work/g", "demo", None, false, Some(section))
        .unwrap();
    svc.ontology_add_field(&ctx(), "problem", "impact:string", None)
        .unwrap();
    (
        World {
            files,
            embedder,
            svc,
        },
        state,
    )
}

fn pending(w: &World) -> Option<usize> {
    w.svc.graph_list()[0].vectors_pending
}

fn recorded_key(w: &World) -> Option<ModelKey> {
    let bytes = w
        .files
        .read(&dir(), umx_core::codec::GraphFile::Graph)
        .unwrap()
        .unwrap();
    umx_core::codec::decode_graph(&bytes)
        .unwrap()
        .embedding
        .and_then(|e| e.key)
}

#[test]
fn a_graph_on_another_model_keeps_new_vectors_pending_until_a_reembed_makes_them() {
    let (mut w, _) = pulled("other@8", Locality::Local);
    assert_eq!(pending(&w), Some(0));
    let a = add(&mut w, "Slow builds", "Builds take an hour.", "Teams wait");
    let b = add(
        &mut w,
        "Flaky tests",
        "One in ten runs fails.",
        "Trust drops",
    );
    run(&mut w);
    // Pending, never made with this embedder's model (D32).
    assert_eq!(pending(&w), Some(2));
    assert_eq!(counts(&w), (0, 0, 0));
    let warnings = reload(&mut w);
    assert!(warnings.contains(&"embedder_mismatch"), "{warnings:?}");
    assert_eq!(pending(&w), Some(2), "a load counts them from the shards");

    // The cost first: counts and keys only, nothing embedded, nothing written.
    let dry = w.svc.embeddings_reembed(&ctx(), true, None).unwrap().value;
    assert_eq!(
        (dry.nodes, dry.texts, dry.dry_run, dry.queued, dry.local),
        (2, 8, true, 0, true)
    );
    assert!(dry.estimated_tokens > 0);
    assert_eq!(
        (dry.from, dry.to.clone()),
        (Some(ModelKey::parse("other@8").unwrap()), key())
    );
    assert_eq!(counts(&w), (0, 0, 0));
    assert_eq!(recorded_key(&w), Some(ModelKey::parse("other@8").unwrap()));

    // Then for real: graph.json records the new model, every node is queued and embedded, pending goes to zero.
    let done = w
        .svc
        .embeddings_reembed(&ctx(), false, Some(&key()))
        .unwrap();
    assert_eq!((done.value.queued, done.value.dry_run), (2, false));
    assert_eq!(recorded_key(&w), Some(key()));
    assert_eq!(pending(&w), Some(2));
    run(&mut w);
    assert_eq!(pending(&w), Some(0));
    assert_eq!((fields_of(&w, a).len(), fields_of(&w, b).len()), (4, 4));
    // Nothing more to do: a second reembed costs nothing.
    let again = w.svc.embeddings_reembed(&ctx(), true, None).unwrap().value;
    assert_eq!((again.nodes, again.texts), (0, 0));
}

#[test]
fn reembed_refuses_another_model_and_a_hosted_embedder_for_a_secured_graph() {
    let (mut w, _) = pulled("other@8", Locality::Local);
    add(&mut w, "Slow builds", "Builds take an hour.", "Teams wait");
    let p = w
        .svc
        .embeddings_reembed(&ctx(), false, Some(&ModelKey::parse("third@8").unwrap()))
        .unwrap_err();
    assert_eq!(p.code, "reembed_model_not_configured");
    assert_eq!(recorded_key(&w), Some(ModelKey::parse("other@8").unwrap()));

    let mut w = world_with(umx_core::Policy::SECURED, Locality::Hosted);
    add(&mut w, "Slow builds", "Builds take an hour.", "Teams wait");
    for dry_run in [true, false] {
        let p = w.svc.embeddings_reembed(&ctx(), dry_run, None).unwrap_err();
        assert_eq!(p.code, "secured_embed_refused");
    }
    assert!(w.svc.take_embed_jobs().is_empty());
    assert_eq!(counts(&w), (0, 0, 0));

    let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let mut none = Services::new(Ports {
        sources: std::sync::Arc::new(umx_fakes::FakeSourceReader::open(
            umx_core::ports::GraphDir::parse("/").unwrap(),
        )),
        files: files.clone(),
        vector_cache: files,
        vector_cache_root: GraphDir::parse("/work/cache").unwrap(),
        clock: Arc::new(FakeClock::default()),
        ids: Arc::new(FakeIds::new()),
        state: Arc::new(FakeLocalState::new()),
        scopes: Arc::new(FakeLocalState::new()),
        registry: Arc::new(FakeLocalState::new()),
        allows: Arc::new(FakeLocalState::new()),
        lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
    });
    none.graph_new(&ctx(), "/work/g", "demo", None, false, None)
        .unwrap();
    let p = none.embeddings_reembed(&ctx(), true, None).unwrap_err();
    assert_eq!(p.code, "embedder_not_configured");
    assert_eq!(
        none.graph_list()[0].vectors_pending,
        None,
        "no model, no embedder"
    );
}

#[test]
fn with_a_matching_embedder_pending_falls_to_zero_as_vectors_land() {
    let mut w = world();
    let id = add(&mut w, "Slow builds", "Builds take an hour.", "Teams wait");
    assert_eq!(pending(&w), Some(1));
    run(&mut w);
    assert_eq!(pending(&w), Some(0));
    w.svc
        .node_edit(
            &ctx(),
            &id.to_text(),
            NodeEditRequest {
                title: Some("Very slow builds".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(pending(&w), Some(1));
    run(&mut w);
    assert_eq!(pending(&w), Some(0));
}

#[test]
fn a_round_of_plans_reads_each_graphs_policy_once() {
    // Architect follow-up: one read of this machine's record per graph per round, not one per node or shard.
    let (mut w, state) = pulled("fake@8", Locality::Local);
    for i in 0..5 {
        add(&mut w, &format!("Node {i}"), "Some text.", "Some impact");
    }
    let jobs = w.svc.take_embed_jobs();
    assert_eq!(jobs.len(), 5);
    let before = state.reads();
    let planned = w.svc.embed_plan_all(&jobs);
    assert_eq!(state.reads() - before, 1);
    assert_eq!(
        planned.iter().filter(|p| matches!(p, Ok(Some(_)))).count(),
        5
    );
    // The positive control: planning one by one reads it each time.
    let before = state.reads();
    for j in &jobs {
        w.svc.embed_plan(j).unwrap();
    }
    assert!(state.reads() - before >= 5);
}

#[test]
fn a_policy_change_recounts_pending_in_the_store_it_now_uses() {
    // Review R85-2: open (graph folder) to secured (this machine's cache), and a confirmed lowering back.
    let mut w = world_with(umx_core::Policy::OPEN, Locality::Local);
    add(&mut w, "Slow builds", "Builds take an hour.", "Teams wait");
    run(&mut w);
    assert_eq!(pending(&w), Some(0), "current in the graph folder");
    w.svc
        .graph_set(&ctx(), "demo", &[("policy".into(), "secured".into())])
        .unwrap();
    assert_eq!(pending(&w), Some(1), "the empty cache has none of them");
    run(&mut w);
    assert_eq!(pending(&w), Some(0), "made again, in the cache");
    // A node added while secured has vectors in the cache only.
    add(
        &mut w,
        "Flaky tests",
        "One in ten runs fails.",
        "Trust drops",
    );
    run(&mut w);
    assert_eq!(pending(&w), Some(0));
    // Lowered in graph.json: still secured, same store, same count.
    w.svc
        .graph_set(&ctx(), "demo", &[("policy".into(), "open".into())])
        .unwrap();
    assert_eq!(pending(&w), Some(0));
    // Confirmed: open again, back to the graph folder, whose vectors are still current.
    w.svc.confirm_policy(&ctx(), "demo", Some("demo")).unwrap();
    assert_eq!(w.svc.effective_policy("demo"), Some(umx_core::Policy::OPEN));
    let before_reload = pending(&w);
    assert_eq!(
        before_reload,
        Some(1),
        "the graph folder lacks the node added while secured"
    );
    reload(&mut w);
    assert_eq!(
        before_reload,
        pending(&w),
        "the count matches the store in use"
    );
}

#[test]
fn an_unreadable_shard_after_an_edit_counts_the_node_pending_and_warns() {
    // Review R85-1, on the real fs adapter: a folder where the node's shard file belongs.
    let root = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("umx-s06b-pending-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    let base = GraphDir::parse(root.to_str().unwrap()).unwrap();
    let files: Arc<dyn GraphFiles> =
        Arc::new(umx_adapter_fs::FsGraphFiles::new(vec![base.clone()]));
    let embedder = Arc::new(FakeEmbedder::new(key()));
    let mut svc = Services::new(Ports {
        sources: std::sync::Arc::new(umx_fakes::FakeSourceReader::open(
            umx_core::ports::GraphDir::parse("/").unwrap(),
        )),
        files: files.clone(),
        vector_cache: files,
        vector_cache_root: base.join("cache").unwrap(),
        clock: Arc::new(FakeClock::default()),
        ids: Arc::new(FakeIds::new()),
        state: Arc::new(FakeLocalState::new()),
        scopes: Arc::new(FakeLocalState::new()),
        registry: Arc::new(FakeLocalState::new()),
        allows: Arc::new(FakeLocalState::new()),
        lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
    })
    .with_embedder(embedder.clone());
    let path = root.join("g");
    let section = umx_core::GraphEmbedding::new(Some(key()), umx_core::Policy::OPEN);
    svc.graph_new(
        &ctx(),
        path.to_str().unwrap(),
        "demo",
        None,
        false,
        Some(section),
    )
    .unwrap();
    let id = svc
        .node_add(
            &ctx(),
            NodeAddRequest {
                kind: "problem".into(),
                title: "Original".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .value
        .node
        .id;
    let jobs = svc.take_embed_jobs();
    let plans: Vec<Planned> = svc
        .embed_plan_all(&jobs)
        .into_iter()
        .map(|p| p.unwrap().unwrap())
        .collect();
    let (vectors, _) = umx_services::embed(embedder.as_ref(), &plans).unwrap();
    svc.embed_apply(&vectors).unwrap();
    assert_eq!(
        svc.graph_list()[0].vectors_pending,
        Some(0),
        "control: current"
    );
    let shard_path = path.join(FileRef::Shard(shard(id)).path());
    std::fs::remove_file(&shard_path).unwrap();
    std::fs::create_dir(&shard_path).unwrap();
    let done = svc
        .node_edit(
            &ctx(),
            &id.to_text(),
            NodeEditRequest {
                title: Some("Changed".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let pending = svc.graph_list()[0].vectors_pending;
    let codes: Vec<&str> = done.notes.iter().map(|p| p.code).collect();
    // Applying vectors for it now cannot read the shard either: the node stays pending.
    let still = svc.embed_apply(&vectors);
    std::fs::remove_dir_all(&root).unwrap();
    assert_eq!(pending, Some(1), "never a false zero");
    assert_eq!(codes, ["vector_shard_unreadable"]);
    assert!(still.is_err() || svc.graph_list()[0].vectors_pending == Some(1));
}

/// An embedder that records every text it is sent, with its kind, and embeds as the fake does.
struct Recording {
    inner: FakeEmbedder,
    seen: std::sync::Mutex<Vec<(TextKind, String)>>,
}

impl Embedder for Recording {
    fn embed(&self, texts: &[String], kind: TextKind) -> Result<Embedded, EmbedError> {
        let mut seen = self.seen.lock().unwrap();
        seen.extend(texts.iter().map(|t| (kind, t.clone())));
        self.inner.embed(texts, kind)
    }
    fn key(&self) -> ModelKey {
        self.inner.key()
    }
    fn concurrency_ceiling(&self) -> usize {
        self.inner.concurrency_ceiling()
    }
    fn max_input_tokens(&self) -> usize {
        self.inner.max_input_tokens()
    }
    fn locality(&self) -> Locality {
        self.inner.locality()
    }
    fn ready(&self) -> Result<(), EmbedError> {
        self.inner.ready()
    }
    fn credential(&self) -> Option<umx_core::ports::Credential> {
        self.inner.credential()
    }
    fn adapter_id(&self) -> &'static str {
        self.inner.adapter_id()
    }
}

/// A world served by a recording embedder: `problem` has impact (content), file (data) and prov_key declared
/// content (as a pulled graph.json might), and a session node exists.
fn recorded_world() -> (World, Arc<Recording>, Id) {
    let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
    let embedder = Arc::new(FakeEmbedder::new(key()));
    let recording = Arc::new(Recording {
        inner: FakeEmbedder::new(key()),
        seen: std::sync::Mutex::new(Vec::new()),
    });
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
        allows: Arc::new(FakeLocalState::new()),
        lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
        registry: Arc::new(FakeLocalState::new()),
    };
    let mut svc = Services::new(ports).with_embedder(recording.clone());
    svc.graph_new(&ctx(), "/work/g", "demo", None, false, None)
        .unwrap();
    svc.ontology_add_field(&ctx(), "problem", "impact:string", None)
        .unwrap();
    svc.ontology_add_field_as(&ctx(), "problem", "file:string", None, Some("data"))
        .unwrap();
    // prov_key is reserved to umx (ws007), so the verbs refuse it: declare and set a stand-in, then rename it in the
    // files, as a graph.json pulled from elsewhere might have it.
    svc.ontology_add_field_as(&ctx(), "problem", "zzstandin:string", None, Some("content"))
        .unwrap();
    svc.ontology_add_field(&ctx(), "session", "summary:string", None)
        .unwrap();
    let mut w = World {
        files,
        embedder,
        svc,
    };
    let id = add(&mut w, "Slow builds", "Builds take an hour.", "Teams wait");
    w.svc
        .node_edit(
            &ctx(),
            &id.to_text(),
            NodeEditRequest {
                sets: vec![
                    ("file".into(), SetValue::Str("src/zzfile.rs".into())),
                    ("zzstandin".into(), SetValue::Str("agent/pij/zzseat".into())),
                ],
                ..Default::default()
            },
        )
        .unwrap();
    w.svc
        .node_add(
            &ctx(),
            NodeAddRequest {
                kind: "session".into(),
                title: "zzsession".into(),
                note: Some("zzsession note".into()),
                sets: vec![("summary".into(), SetValue::Str("zzsession summary".into()))],
                ..Default::default()
            },
        )
        .unwrap();
    w.svc.graph_unload(&ctx(), "demo").unwrap();
    for file in [GraphFile::Graph, GraphFile::Nodes] {
        let text = String::from_utf8(w.files.read(&dir(), file).unwrap().unwrap()).unwrap();
        assert!(text.contains("\"zzstandin\""));
        let renamed = text.replace("\"zzstandin\"", "\"prov_key\"");
        let bytes = if file == GraphFile::Nodes {
            let mut nodes = umx_core::codec::decode_nodes(renamed.as_bytes())
                .unwrap()
                .records;
            let session = nodes.iter_mut().find(|n| n.title == "zzsession").unwrap();
            session.fields.insert(
                "prov_key".into(),
                serde_json::json!("session/copilot/h-1111111111111111"),
            );
            session.authored_by_session = None;
            session.edited_by_session = None;
            umx_core::codec::encode_nodes(&nodes)
        } else {
            renamed.into_bytes()
        };
        w.files.write(&dir(), file, &bytes).unwrap();
    }
    w.svc.graph_load(&ctx(), "/work/g", None).unwrap();
    (w, recording, id)
}

#[test]
fn document_texts_sent_for_field_and_whole_node_vectors_hold_no_data_field() {
    let (mut w, recording, id) = recorded_world();
    run_with(&mut w, &*recording);
    let seen = recording.seen.lock().unwrap().clone();
    assert!(!seen.is_empty());
    assert!(seen.iter().all(|(k, _)| *k == TextKind::Document));
    for (_, text) in &seen {
        assert!(
            !text.contains("zz"),
            "a data field, provenance or a session node was sent: {text}"
        );
    }
    // The content field and the whole node went, built without the data fields.
    let whole = seen
        .iter()
        .find(|(_, t)| t.starts_with("title: Slow builds"))
        .unwrap();
    assert_eq!(
        whole.1,
        "title: Slow builds | text: impact: Teams wait\nnote: Builds take an hour."
    );
    assert!(
        seen.iter()
            .any(|(_, t)| t == "title: none | text: Teams wait")
    );
    assert_eq!(fields_of(&w, id), ["@node.v2", "impact", "note", "title"]);
}

#[test]
fn a_search_query_text_is_sent_unchanged() {
    let (w, recording, _) = recorded_world();
    let query = "src/zzfile.rs agent/pij/zzseat: title | text";
    w.svc.embed_query("demo", query).unwrap().unwrap();
    let seen = recording.seen.lock().unwrap().clone();
    assert_eq!(seen, [(TextKind::Query, query.to_owned())]);
}

#[test]
fn word_search_still_finds_the_values_of_data_fields_of_kind_string() {
    let (mut w, recording, id) = recorded_world();
    // Enough other nodes that the word is informative (in at most a quarter of them).
    for i in 0..4 {
        add(&mut w, &format!("Other {i}"), "A note.", "Some impact");
    }
    run_with(&mut w, &*recording);
    let done = w.svc.search(&ctx(), "zzfile", None, 10).unwrap();
    let hit = done.value.hits.iter().find(|h| h.node.id == id).unwrap();
    assert_eq!(hit.word.as_ref().map(|m| m.field.as_str()), Some("file"));
}

// ---------------------------------------------------------------- F144 (review R1-2): a secured graph's cache

/// The cache folder of `w`'s secured graph: <root>/<graph id>/<16 hex of the folder's hash>.
fn cache_of(w: &World) -> GraphDir {
    let graph_id = w.svc.loaded("demo").unwrap().meta.id.to_text();
    let folder_key = TextHash::of("/work/g").to_string();
    GraphDir::parse(&format!("/work/cache/{graph_id}/{}", &folder_key[..16])).unwrap()
}

/// The first batch of vectors for the queued jobs, made but not applied.
fn embedded(w: &mut World) -> Vec<umx_services::Vectors> {
    let jobs = w.svc.take_embed_jobs();
    let planned: Vec<Planned> = jobs
        .iter()
        .filter_map(|j| w.svc.embed_plan(j).unwrap())
        .collect();
    let batch = batches(planned).into_iter().next().unwrap();
    match step(&*w.embedder, &batch, &mut Pacer::default(), 0) {
        Step::Done { vectors, .. } => vectors,
        other => panic!("{other:?}"),
    }
}

/// A secured graph's cache holds none of its content files, so a cache line made from text a checkout then
/// replaced may stand there. A line is keyed by its text's hash, so after a load of the changed folder that line is
/// not current: the node is pending, to be embedded again. (Search may still use the stale vector meanwhile, req-75;
/// this does not claim it is never served.)
#[test]
fn a_cache_line_from_text_a_checkout_replaced_is_not_current_and_the_node_is_pending() {
    let mut w = world_with(umx_core::Policy::SECURED, Locality::Local);
    let id = add(&mut w, "Slow builds", "Builds take an hour.", "Teams wait");
    run(&mut w);
    assert_eq!(pending(&w), Some(0));
    assert_eq!(shards_under(&w, &cache_of(&w)), vec![shard(id)]);
    let nodes = w.files.raw_for_tests(&dir(), GraphFile::Nodes).unwrap();
    let checked_out = String::from_utf8(nodes)
        .unwrap()
        .replace("Builds take an hour.", "Builds take a minute.");
    w.files
        .tamper_for_tests(&dir(), GraphFile::Nodes, checked_out.as_bytes());
    reload(&mut w);
    assert_eq!(
        shards_under(&w, &cache_of(&w)),
        vec![shard(id)],
        "the line stays"
    );
    assert_eq!(pending(&w), Some(1), "but is not current");
}

/// A checkout that lands while a secured graph's vectors go into the cache (after the look before the set) is
/// found by the look after it: refused with graph_files_changed naming the graph's files, never the cache, and
/// memory keeps none of the vectors (the node stays pending). Applied as the daemon's backlog applies (the
/// similar_to files deferred), so the shard write is the only write. Control: with nothing landing, they are kept.
#[test]
fn a_checkout_landing_while_vectors_are_cached_keeps_none_of_them() {
    let mut w = world_with(umx_core::Policy::SECURED, Locality::Local);
    add(&mut w, "Slow builds", "Builds take an hour.", "Teams wait");
    let vectors = embedded(&mut w);
    let cache = cache_of(&w);
    w.files
        .change_after_next_marker_for_tests(&cache, &dir(), GraphFile::Edges, b"checked out\n");
    let refused = w.svc.embed_apply_deferred(&vectors).unwrap_err();
    assert_eq!(refused.code, "graph_files_changed");
    let said = format!("{refused:?}");
    assert!(
        said.contains("edges.jsonl") && !said.contains("/work/cache"),
        "{said}"
    );
    assert_eq!(pending(&w), Some(1), "memory keeps none of the vectors");
    assert_eq!(
        w.files.raw_for_tests(&dir(), GraphFile::Edges).as_deref(),
        Some(&b"checked out\n"[..])
    );
    // Control: the same vectors with nothing landing are kept.
    let mut w = world_with(umx_core::Policy::SECURED, Locality::Local);
    add(&mut w, "Slow builds", "Builds take an hour.", "Teams wait");
    let vectors = embedded(&mut w);
    w.svc.embed_apply_deferred(&vectors).unwrap();
    assert_eq!(pending(&w), Some(0));
}

/// ws006 S23 round 2 (review R8): embed work queued before a rename, and work already taken under the old alias when
/// it lands, both end with the node's vectors written: none of it is lost with the old name.
#[test]
fn embed_work_queued_or_taken_before_a_rename_still_lands() {
    let mut w = world();
    let taken = add(&mut w, "Taken", "Taken before the rename.", "Late");
    let jobs = w.svc.take_embed_jobs();
    let planned: Vec<Planned> = jobs
        .iter()
        .filter_map(|j| w.svc.embed_plan(j).unwrap())
        .collect();
    let queued = add(&mut w, "Queued", "Queued before the rename.", "Waits");
    w.svc.graph_rename(&ctx(), "demo", "renamed").unwrap();
    let mut pacer = Pacer::default();
    for batch in batches(planned) {
        if let Step::Done { vectors, .. } = step(&*w.embedder.clone(), &batch, &mut pacer, 0) {
            w.svc.embed_apply(&vectors).unwrap();
        }
    }
    run(&mut w);
    for id in [taken, queued] {
        assert_eq!(fields_of(&w, id), ["@node.v2", "impact", "note", "title"]);
    }
    assert_eq!(w.svc.loaded("renamed").unwrap().pending_count(), Some(0));
}

/// ws006 S23 round 2 (review R2): a moved graph keeps what its load built: its vectors stay current (nothing pending,
/// nothing queued, nothing embedded again) and its shards are read at the new folder.
#[test]
fn a_moved_graph_keeps_its_vectors_and_queues_nothing() {
    let mut w = world();
    let config = umx_fakes::FakeConfigFiles::new(
        GraphDir::parse("/home").unwrap(),
        GraphDir::parse("/work").unwrap(),
    );
    w.svc = std::mem::replace(&mut w.svc, services(&w.files, &w.embedder))
        .with_config_files(Arc::new(config));
    let id = add(&mut w, "Indexed", "Embedded before the move.", "Kept");
    run(&mut w);
    let before = counts(&w);
    w.svc.graph_move(&ctx(), "demo", "/work/h").unwrap();
    assert_eq!(w.svc.loaded("demo").unwrap().pending_count(), Some(0));
    assert!(w.svc.take_embed_jobs().is_empty());
    assert_eq!(counts(&w), before);
    let moved = GraphDir::parse("/work/h").unwrap();
    assert!(
        w.files
            .read_ref(&moved, &FileRef::Shard(shard(id)))
            .unwrap()
            .is_some()
    );
}

#[test]
fn a_field_in_the_wrong_role_group_is_hinted_and_its_role_never_changes_by_itself() {
    // PRD 002 search-05 on the fakes: `impact` (string, content by default) holds two short labels; `summary`,
    // declared data, holds prose; `due` (a date) repeats one value and is never judged.
    let mut w = world();
    w.svc
        .ontology_add_field_as(&ctx(), "problem", "summary:string", None, Some("data"))
        .unwrap();
    for i in 0..6 {
        let id = add(
            &mut w,
            &format!("Node {i}"),
            "A note.",
            ["low", "high"][i % 2],
        );
        w.svc
            .node_edit(
                &ctx(),
                &id.to_text(),
                NodeEditRequest {
                    sets: vec![
                        (
                            "summary".into(),
                            SetValue::Str(format!(
                                "the supplier asked for a longer review window before round {i}"
                            )),
                        ),
                        ("due".into(), SetValue::Raw("2026-12-01".into())),
                    ],
                    ..Default::default()
                },
            )
            .unwrap();
    }
    let before = graph_json(&w);
    let notes = w.svc.graph_guide(&ctx(), "demo").unwrap().notes;
    let mut codes: Vec<&str> = notes
        .iter()
        .map(|p| p.code)
        .filter(|c| c.starts_with("field_looks_like"))
        .collect();
    codes.sort_unstable();
    assert_eq!(codes, ["field_looks_like_labels", "field_looks_like_prose"]);
    // Pointing it out changes nothing: graph.json is byte for byte the same, and no vector is made pending.
    assert_eq!(graph_json(&w), before);
    assert_eq!(w.svc.role_hints("demo", None).len(), 2);
    assert_eq!(
        w.svc.role_hints("demo", Some(("problem", "impact"))).len(),
        1
    );
    assert!(
        w.svc
            .role_hints("demo", Some(("problem", "due")))
            .is_empty()
    );
    // After the role is set as the hint says, that field's hint goes.
    w.svc
        .ontology_set_field(&ctx(), "problem", "impact", "data")
        .unwrap();
    assert!(
        w.svc
            .role_hints("demo", Some(("problem", "impact")))
            .is_empty()
    );
    // A graph read only when named is never read for the `embeddings status` listing.
    assert_eq!(w.svc.role_hints_unnamed("demo").len(), 1);
    w.svc
        .graph_set(&ctx(), "demo", &[("visibility".into(), "named".into())])
        .unwrap();
    assert!(w.svc.role_hints_unnamed("demo").is_empty());
    assert_eq!(w.svc.role_hints("demo", None).len(), 1);
}

//! related-record (ws004 s28, embeddings-expert ruling): records every vector the related-on-write fixture needs,
//! exactly as production makes them, for `crates/umx-services/tests/related_recorded.rs`. The fixture's existing nodes
//! go into a scratch graph through umx's own services and are embedded by umx's embed loop (documents: whole node,
//! title and note); then each new node is added and checked by umx's own related check (its whole-node text as a
//! query, through `related_kind`). Every text the instance is asked to embed is written as `<document|query> TAB
//! <umx TextHash of the text, hex> TAB <base64 of the int8 row>`, sorted: the format hybrid_recall's rows use.
//!
//! Usage: related-record --embedder <instance> --fixture <nodes.jsonl> --out <rows.tsv> [--config-home <umx home>]
//! The fixture is synthetic text only. A hosted instance is sent the fixture's text.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use umx_compose::{Config, Mode, compose, compose_config};
use umx_core::ports::{Credential, EmbedError, Embedded, Embedder, Locality, ModelKey, TextKind};
use umx_core::vectors::{TextHash, base64_encode, quantise};
use umx_core::{GraphEmbedding, Id, Policy};
use umx_services::related::RelatedRequest;
use umx_services::{Ctx, NodeAddRequest, Pacer, Planned, Services, Step, batches, step};

/// Passes every call to `inner` and keeps each text's vector by kind and hash.
struct Recorder {
    inner: Arc<dyn Embedder>,
    rows: Mutex<BTreeMap<(&'static str, String), Vec<f32>>>,
}

impl Embedder for Recorder {
    fn embed(&self, texts: &[String], kind: TextKind) -> Result<Embedded, EmbedError> {
        let out = self.inner.embed(texts, kind)?;
        let word = match kind {
            TextKind::Document => "document",
            TextKind::Query => "query",
        };
        let mut rows = self.rows.lock().unwrap();
        for (t, v) in texts.iter().zip(&out.vectors) {
            rows.insert((word, TextHash::of(t).to_string()), v.clone());
        }
        Ok(out)
    }
    fn key(&self) -> ModelKey {
        self.inner.key()
    }
    fn concurrency_ceiling(&self) -> usize {
        self.inner.concurrency_ceiling()
    }
    fn max_batch_inputs(&self) -> usize {
        self.inner.max_batch_inputs()
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
    fn credential(&self) -> Option<Credential> {
        self.inner.credential()
    }
    fn adapter_id(&self) -> &'static str {
        self.inner.adapter_id()
    }
}

struct Args {
    embedder: String,
    fixture: PathBuf,
    out: PathBuf,
    config_home: Option<PathBuf>,
}

fn args() -> Result<Args, String> {
    let usage = "usage: related-record --embedder <instance> --fixture <nodes.jsonl> --out <rows.tsv> [--config-home <dir>]";
    let mut it = std::env::args().skip(1);
    let (mut embedder, mut fixture, mut out, mut config_home) = (None, None, None, None);
    while let Some(flag) = it.next() {
        let value = it.next().ok_or(usage)?;
        match flag.as_str() {
            "--embedder" => embedder = Some(value),
            "--fixture" => fixture = Some(PathBuf::from(value)),
            "--out" => out = Some(PathBuf::from(value)),
            "--config-home" => config_home = Some(PathBuf::from(value)),
            _ => return Err(usage.into()),
        }
    }
    Ok(Args {
        embedder: embedder.ok_or(usage)?,
        fixture: fixture.ok_or(usage)?,
        out: out.ok_or(usage)?,
        config_home,
    })
}

fn embed_all(svc: &mut Services, embedder: &dyn Embedder) -> Result<(), String> {
    let mut pacer = Pacer::default();
    loop {
        let jobs = svc.take_embed_jobs();
        if jobs.is_empty() {
            return Ok(());
        }
        let planned: Vec<Planned> = jobs
            .iter()
            .filter_map(|j| svc.embed_plan(j).ok().flatten())
            .collect();
        for batch in batches(planned) {
            match step(embedder, &batch, &mut pacer, 0) {
                Step::Done { vectors, .. } => {
                    svc.embed_apply(&vectors).map_err(|p| p.code.to_string())?;
                }
                other => return Err(format!("{other:?}")),
            }
        }
    }
}

fn run() -> Result<(), String> {
    let a = args()?;
    let env = Config::from_env();
    let mut source = match &a.config_home {
        Some(home) => Config::at(home.clone(), None).search_path(env.search_path.clone()),
        None => Config::from_env(),
    };
    source.models_dir = env.models_dir.clone();
    let settings = compose_config(Mode::Real, &source);
    let embedders = umx_compose::embedders(Mode::Real, &source, &settings);
    let instance = embedders
        .get(&a.embedder)
        .ok_or(format!("no embedder instance {}", a.embedder))?;
    let inner = instance
        .built
        .clone()
        .map_err(|e| format!("instance {}: {e}", a.embedder))?;
    let recorder = Arc::new(Recorder {
        inner,
        rows: Mutex::new(BTreeMap::new()),
    });
    let nodes: Vec<serde_json::Value> = fs::read_to_string(&a.fixture)
        .map_err(|e| e.to_string())?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    let scratch = std::env::temp_dir().join(format!("umx-related-record-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
    let scratch = scratch.canonicalize().map_err(|e| e.to_string())?;
    let config = Config::at(scratch.join("home"), None).roots([scratch.to_str().unwrap()]);
    let mut svc = compose(Mode::Real, &config).with_embedder(recorder.clone() as Arc<dyn Embedder>);
    let mut ctx = Ctx::new("related-record");
    svc.graph_new(
        &ctx,
        scratch.join("g").to_str().unwrap(),
        "fixture",
        None,
        false,
        Some(GraphEmbedding::new(None, Policy::OPEN)),
    )
    .map_err(|p| p.code.to_string())?;
    ctx.graph = Some("fixture".into());
    let add = |svc: &mut Services, n: &serde_json::Value| -> Result<Id, String> {
        svc.node_add(
            &ctx,
            NodeAddRequest {
                kind: "problem".into(),
                title: n["title"].as_str().unwrap_or_default().into(),
                note: n["note"].as_str().map(Into::into),
                ..Default::default()
            },
        )
        .map(|d| d.value.node.id)
        .map_err(|p| p.code.to_string())
    };
    for n in nodes.iter().filter(|n| n["role"] == "existing") {
        add(&mut svc, n)?;
    }
    embed_all(&mut svc, &*recorder)?;
    for n in nodes.iter().filter(|n| n["role"] == "new") {
        let id = add(&mut svc, n)?;
        let r = svc.related(
            &RelatedRequest {
                alias: "fixture",
                added: &[id],
                flag: None,
                machine: true,
                verb: "node add",
            },
            &|_, _| true,
        );
        let titles: Vec<String> = r
            .rows
            .get(&id)
            .into_iter()
            .flatten()
            .map(|row| format!("{} ({:.4})", row.title, row.score))
            .collect();
        let floor = r.floor.map_or("none".into(), |f| {
            format!("{} {} {:.4}", f.use_word(), f.source.word(), f.value)
        });
        eprintln!(
            "{}: {:?} floor {floor}; hints {:?}",
            n["name"].as_str().unwrap_or_default(),
            titles,
            r.hints.iter().map(|h| h.slot("reason")).collect::<Vec<_>>()
        );
    }
    let _ = fs::remove_dir_all(&scratch);
    let rows = recorder.rows.lock().unwrap();
    let mut lines: Vec<String> = rows
        .iter()
        .map(|((kind, hash), v)| {
            let row: Vec<u8> = quantise(v).into_iter().map(|x| x as u8).collect();
            format!("{kind}\t{hash}\t{}", base64_encode(&row))
        })
        .collect();
    lines.sort();
    fs::write(&a.out, lines.join("\n") + "\n").map_err(|e| e.to_string())?;
    eprintln!(
        "related-record: {} through {} (backend {}): {} rows; wrote {}",
        recorder.key(),
        recorder.adapter_id(),
        instance.local_backend().map(|(b, _)| b).unwrap_or("hosted"),
        lines.len(),
        a.out.display()
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("related-record: {e}");
            ExitCode::FAILURE
        }
    }
}

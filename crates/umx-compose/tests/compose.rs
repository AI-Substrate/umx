//! The composition matches the documents, and the real adapters carry the inner layers end to end on disk.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use umx_compose::{Config, Mode, client_adapters, compose, compose_config, describe};
use umx_core::codec::{self, GraphFile};
use umx_services::{Ctx, NodeAddRequest};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// (mode, port, adapter) rows read from the adapters `services` run over, plus the client-only ports (`client`).
fn rows(
    mode: &str,
    services: &umx_services::Services,
    config: &umx_services::ConfigServices,
    client: Vec<(&'static str, &'static str)>,
) -> Vec<umx_arch::WiringRow> {
    describe(services, config)
        .into_iter()
        .chain(client)
        .map(|(port, adapter)| umx_arch::WiringRow {
            mode: mode.to_string(),
            port: port.to_string(),
            adapter: adapter.to_string(),
        })
        .collect()
}

#[test]
fn describe_equals_the_adapter_matrix() {
    // r-wiring-matches-docs: rules.toml's [[port]] rows are d-6 as data; describe() reads the adapter ids from
    // the services compose built (E-1). Stub rows need `--features stub` (just check runs both).
    let root = repo_root();
    let rules = umx_arch::load_rules(&root.join("crates/umx-arch/rules.toml")).unwrap();
    let active = rules.active_ports();
    assert_eq!(
        active.len(),
        17,
        "graph-files, clock and id-generator from s05a; config-files and secrets from ws002 s07; vector-cache \
         from ws004 s12; local-state from ws004 s06; service-manager from ws002 s10; embedder from ws004 s05; \
         model-fetcher from ws004 s14; secret-allows from ws004 s08; scopes from ws005 s07a; registry from \
         ws002 F72; ui-host from ws009 s2; source-reader from ws006 s1; client-only agent-identity from ws007 s02; \
         lint-terms from F139"
    );
    let t = temp("describe");
    let config = config(&t.0);
    #[cfg(feature = "stub")]
    let stub = rows(
        "stub",
        &compose(Mode::Stub, &config),
        &compose_config(Mode::Stub, &config),
        client_adapters(Mode::Stub, &config),
    );
    #[cfg(not(feature = "stub"))]
    let stub = Vec::new();
    // Real mode twice: no embedder chosen (none is allowed by the list), and the azure instance chosen.
    let real = rows(
        "real",
        &compose(Mode::Real, &config),
        &compose_config(Mode::Real, &config),
        client_adapters(Mode::Real, &config),
    );
    let azure = with_azure(&t.0);
    let built = [real, stub].concat();
    let real_only: Vec<_> = umx_arch::check_wiring(&rules, &active, &azure)
        .into_iter()
        .filter(|v| !v.at.starts_with("stub/"))
        .collect();
    assert_eq!(real_only, vec![]);
    assert!(
        azure
            .iter()
            .any(|r| r.port == "embedder" && r.adapter == "azure-openai.embedder"),
        "{azure:?}"
    );
    // Without the fakes only the real rows can be built; just check also runs this with --features stub.
    if !cfg!(feature = "stub") {
        println!("stub rows not built: run with --features stub");
    }
    let found: Vec<_> = umx_arch::check_wiring(&rules, &active, &built)
        .into_iter()
        .filter(|v| cfg!(feature = "stub") || !v.at.starts_with("stub/"))
        .collect();
    assert_eq!(found, vec![]);
}

#[cfg(feature = "stub")]
#[test]
fn a_stub_that_wires_a_real_adapter_fails_the_wiring_check() {
    // Planted mis-wiring: stub mode with the system clock. describe() reads it from the object, so it shows.
    let rules = umx_arch::load_rules(&repo_root().join("crates/umx-arch/rules.toml")).unwrap();
    let ports = umx_services::Ports {
        sources: std::sync::Arc::new(umx_fakes::FakeSourceReader::open(
            umx_core::ports::GraphDir::parse("/").unwrap(),
        )),
        files: Arc::new(umx_fakes::FakeGraphFiles::new(
            umx_core::ports::GraphDir::parse("/").unwrap(),
        )),
        vector_cache: Arc::new(umx_fakes::FakeGraphFiles::new(
            umx_core::ports::GraphDir::parse("/cache").unwrap(),
        )),
        vector_cache_root: umx_core::ports::GraphDir::parse("/cache").unwrap(),
        clock: Arc::new(umx_adapter_system::SystemClock),
        ids: Arc::new(umx_fakes::FakeIds::new()),
        state: Arc::new(umx_fakes::FakeLocalState::new()),
        scopes: Arc::new(umx_fakes::FakeLocalState::new()),
        registry: Arc::new(umx_fakes::FakeLocalState::new()),
        allows: Arc::new(umx_fakes::FakeLocalState::new()),
        lint_terms: Arc::new(umx_fakes::FakeLocalState::new().with_absent_ok(true)),
    };
    let t = temp("miswired");
    let config = config(&t.0);
    let mut built = rows(
        "real",
        &compose(Mode::Real, &config),
        &compose_config(Mode::Real, &config),
        client_adapters(Mode::Real, &config),
    );
    let stub = umx_services::Services::new(ports)
        .with_service_manager(Arc::new(umx_fakes::FakeServiceManager::new()))
        .with_embedder(Arc::new(umx_fakes::FakeEmbedder::new(
            umx_core::ports::ModelKey::parse("m@8").unwrap(),
        )))
        .with_model_fetcher(Arc::new(umx_fakes::FakeModelFetcher::new()));
    built.extend(rows(
        "stub",
        &stub,
        &compose_config(Mode::Stub, &config),
        client_adapters(Mode::Stub, &config),
    ));
    let found = umx_arch::check_wiring(&rules, &rules.active_ports(), &built);
    let at: Vec<&str> = found.iter().map(|v| v.at.as_str()).collect();
    assert_eq!(at, ["stub/clock", "stub/clock"], "{found:?}");
}

/// The real rows with `embedders.az` (azure_openai) active, wired as the daemon does: built with no I/O.
fn with_azure(root: &Path) -> Vec<umx_arch::WiringRow> {
    let config = config(root);
    std::fs::create_dir_all(&config.home).unwrap();
    std::fs::write(
        config.config_file(),
        "embedder.active = \"az\"\nembedders.az.kind = \"azure_openai\"\n\
         embedders.az.endpoint = \"https://127.0.0.1:9\"\nembedders.az.deployment = \"emb\"\n\
         embedders.az.model = \"text-embedding-3-small\"\nembedders.az.dims = 256\n",
    )
    .unwrap();
    let settings = compose_config(Mode::Real, &config);
    let embedders = umx_compose::embedders(Mode::Real, &config, &settings);
    let (name, embedder) = embedders
        .active(&settings.resolve(None))
        .unwrap()
        .expect("az is active");
    assert_eq!(name, "az");
    let mut services = compose(Mode::Real, &config);
    services.set_embedder(Some(embedder));
    rows(
        "real",
        &services,
        &settings,
        client_adapters(Mode::Real, &config),
    )
}

struct Temp(PathBuf);

/// The machine config's floors reach services by model key; two instances of one model: the first name's wins.
#[test]
fn instance_floors_come_from_the_machine_config_by_model() {
    use umx_core::similarity::{FloorSource, Use};
    let t = temp("floors");
    let config = config(&t.0);
    std::fs::create_dir_all(&config.home).unwrap();
    std::fs::write(
        config.config_file(),
        "embedders.a.kind = \"azure_openai\"\nembedders.a.endpoint = \"https://127.0.0.1:9\"\n\
         embedders.a.deployment = \"emb\"\nembedders.a.model = \"mystery\"\nembedders.a.dims = 256\n\
         embedders.a.search_floor = 0.25\n\
         embedders.b.kind = \"azure_openai\"\nembedders.b.endpoint = \"https://127.0.0.1:9\"\n\
         embedders.b.deployment = \"emb\"\nembedders.b.model = \"mystery\"\nembedders.b.dims = 256\n\
         embedders.b.search_floor = 0.75\nembedders.b.similar_floor = 0.5\n",
    )
    .unwrap();
    let settings = compose_config(Mode::Real, &config);
    let embedders = umx_compose::embedders(Mode::Real, &config, &settings);
    let floors = embedders.floors(&settings.resolve(None));
    let key = umx_core::ports::ModelKey::parse("mystery@256").unwrap();
    let search = floors.get(&key, Use::Search);
    assert_eq!((search.value, search.source), (0.25, FloorSource::Config));
    assert_eq!(floors.get(&key, Use::Similar).value, 0.5);
    let other = umx_core::ports::ModelKey::parse("mystery@512").unwrap();
    assert_eq!(floors.get(&other, Use::Search).source, FloorSource::None);
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp(name: &str) -> Temp {
    let p = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("umx-compose-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    Temp(p)
}

fn config(root: &Path) -> Config {
    Config::at(root.join("home"), None).roots([root.to_str().unwrap()])
}

#[test]
fn real_adapters_write_files_that_reload_byte_identical() {
    let t = temp("reload");
    let path = t.0.join("g");
    let path = path.to_str().unwrap();
    let ctx = Ctx::new("tester");
    let mut svc = compose(Mode::Real, &config(&t.0));
    svc.graph_new(&ctx, path, "demo", None, false, None)
        .unwrap();
    let a = svc
        .node_add(
            &ctx,
            NodeAddRequest {
                kind: "problem".into(),
                title: "Approval delays".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let b = svc
        .node_add(
            &ctx,
            NodeAddRequest {
                kind: "decision".into(),
                title: "Launch date".into(),
                ..Default::default()
            },
        )
        .unwrap();
    svc.edge_add(
        &ctx,
        &a.value.node.id.to_text(),
        "blocks",
        &b.value.node.id.to_text(),
        &[],
    )
    .unwrap();
    // req-04: the node is in nodes.jsonl when the call returns.
    let nodes = std::fs::read_to_string(t.0.join("g/nodes.jsonl")).unwrap();
    assert!(nodes.contains(&a.value.node.id.to_text()));

    // req-01 and req-02: a fresh composition rebuilds the graph from the files alone, and writing it back is
    // byte-identical.
    let mut fresh = compose(Mode::Real, &config(&t.0));
    fresh.graph_load(&ctx, path, None).unwrap();
    let l = fresh.loaded("demo").unwrap();
    for (file, bytes) in [
        (GraphFile::Graph, codec::encode_graph(&l.meta)),
        (GraphFile::Nodes, l.graph.encode_nodes()),
        (GraphFile::Edges, l.graph.encode_edges()),
    ] {
        assert_eq!(
            std::fs::read(t.0.join("g").join(file.file_name())).unwrap(),
            bytes,
            "{}",
            file.file_name()
        );
    }
    assert_eq!(
        fresh
            .node_get(&ctx, &a.value.node.id.to_text())
            .unwrap()
            .value
            .1,
        a.value.node
    );
}

#[test]
fn four_writers_through_one_lock_lose_nothing() {
    // The daemon's writer (d-7) holds Services behind one lock; this is that shape without the socket. The
    // process-level proof (req-52: 4 x 250 through the CLI) lands with the daemon in phase 2. Each add fsyncs the
    // file and folder, so this stays small to keep `just check` fast.
    let t = temp("writers");
    let path = t.0.join("g");
    let ctx = Ctx::new("tester");
    let mut svc = compose(Mode::Real, &config(&t.0));
    svc.graph_new(&ctx, path.to_str().unwrap(), "demo", None, false, None)
        .unwrap();
    let svc = Arc::new(Mutex::new(svc));
    let handles: Vec<_> = (0..4)
        .map(|w| {
            let svc = Arc::clone(&svc);
            std::thread::spawn(move || {
                let ctx = Ctx::new(format!("writer{w}"));
                for i in 0..50 {
                    let req = NodeAddRequest {
                        kind: "problem".into(),
                        title: format!("w{w} n{i}"),
                        ..Default::default()
                    };
                    svc.lock().unwrap().node_add(&ctx, req).unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let lines = std::fs::read_to_string(path.join("nodes.jsonl")).unwrap();
    let decoded = codec::decode_nodes(lines.as_bytes()).unwrap().records;
    assert_eq!(
        decoded
            .iter()
            .filter(|n| !umx_core::provenance::is_provenance(n))
            .count(),
        200
    );
    assert_eq!(codec::encode_nodes(&decoded), lines.as_bytes());
}

/// ws006 S1 R2: in one stub composition, `root add` and `root remove` change what a graph write's `root:` lookup
/// sees. Each `compose_config` call has its own fake machine file, so this fails if the graph gets its own.
#[cfg(feature = "stub")]
#[test]
fn a_stub_composition_shares_one_config_between_root_commands_and_evidence() {
    let t = temp("stub-roots");
    let (mut svc, cs) = umx_compose::compose_with_config(Mode::Stub, &config(&t.0));
    let ctx = Ctx::new("tester");
    svc.graph_new(&ctx, "/work/g", "demo", None, false, None)
        .unwrap();
    let cite = |svc: &mut umx_services::Services| {
        svc.node_add(
            &ctx,
            NodeAddRequest {
                kind: "problem".into(),
                title: "cited".into(),
                evidence: vec!["root:acme/transcripts/016.ndjson#seq=1-5".into()],
                ..Default::default()
            },
        )
        .map(|_| ())
        .map_err(|p| p.code)
    };
    assert_eq!(cite(&mut svc), Err("root_unknown"));
    cs.root_add("acme", "/work/acme").unwrap();
    assert_eq!(cite(&mut svc), Ok(()));
    cs.root_remove("acme").unwrap();
    assert_eq!(cite(&mut svc), Err("root_unknown"));
}

#[test]
fn config_keeps_only_absolute_roots() {
    let roots = Config::from_env().roots;
    assert!(!roots.is_empty());
    assert!(roots.windows(2).all(|w| w[0] < w[1]));
}

#[test]
fn a_secret_never_reaches_a_formatted_config() {
    let t = temp("secret");
    let sentinel = "sentinel-qz9-value";
    let home = t.0.join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("config.toml"),
        "embedders.az.kind = \"azure_openai\"\nembedders.az.auth = \"api_key\"\nembedders.az.api_key_env = \"MY_KEY\"\n",
    )
    .unwrap();
    let config = config(&t.0).secrets_env([("MY_KEY".to_owned(), sentinel.to_owned())].into());
    let cs = compose_config(Mode::Real, &config);
    // Positive control: the secret is there for the code that asks for it.
    let found = cs.secret("MY_KEY").expect("found");
    assert_eq!(found.1.expose(), sentinel);
    let shown = cs.show(None).unwrap().value;
    let api_key_env = shown
        .settings
        .iter()
        .find(|s| s.key == "embedders.az.api_key_env")
        .unwrap();
    assert_eq!(api_key_env.secret, Some("set (env)"));
    let text = format!(
        "{config:?} {found:?} {shown:?} {:?} {:?}",
        cs.resolve(None),
        cs.check(None)
    );
    assert!(!text.contains(sentinel), "{text}");
    assert!(text.contains("<redacted>"));
}

/// s17 item 7: the local llama.cpp adapter, built the way compose builds a `local` instance, over the adapter's tiny
/// stub model (no download, CPU). A secured graph recording the stub's key embeds with it end to end on the real file
/// adapters, its vectors go to the local cache and never into the graph folder, and on the CPU the adapter says so.
#[test]
fn a_secured_graph_embeds_with_the_local_llamacpp_stub() {
    use umx_adapter_llamacpp::{Execution, LlamaConfig, ModelFile, ModelSpec, Pooling};
    use umx_core::ports::{Embedder, Locality};
    let fixtures =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../umx-adapter-llamacpp/tests/fixtures/models");
    let spec = ModelSpec {
        name: "stub".to_string(),
        repo: "umx/stub".to_string(),
        commit: "fixture".to_string(),
        file: ModelFile {
            path: "stub.gguf".to_string(),
            sha256: "9a8ae47fec88f720b91ce079afdd04bbfd699f3b33024088dd8948385faf52a6".to_string(),
        },
        cpu_file: None,
        pooling: Pooling::Mean,
        dims: vec![16, 8],
        max_tokens: 64,
        query_prefix: "q: ".to_string(),
        document_prefix: String::new(),
        prompt_version: 1,
    };
    let mut local = LlamaConfig::new(spec, fixtures);
    local.dims = 8;
    local.threads = 1;
    local.execution = Execution::Cpu;
    let stub = umx_compose::local_embedder(local).unwrap();
    assert_eq!(stub.locality(), Locality::Local);
    assert_eq!(stub.backend().as_str(), "cpu");
    assert!(
        stub.slow_warning().is_some(),
        "a CPU-only local embedder is never silent"
    );
    stub.ready().unwrap();

    let t = temp("llamacpp");
    let config = config(&t.0);
    let mut svc = compose(Mode::Real, &config);
    svc.set_instances(
        vec![umx_services::embedding::Instance {
            name: "stub".into(),
            embedder: stub.clone(),
        }],
        Some("stub"),
    );
    let path = t.0.join("vault");
    let ctx = Ctx::new("tester");
    let key = umx_core::ports::ModelKey::parse("stub@8+p1").unwrap();
    let done = svc
        .graph_new(
            &ctx,
            path.to_str().unwrap(),
            "vault",
            None,
            false,
            Some(umx_core::GraphEmbedding::new(
                Some(key.clone()),
                umx_core::Policy::SECURED,
            )),
        )
        .unwrap();
    assert!(done.notes.is_empty(), "{:?}", done.notes);
    let id = svc
        .node_add(
            &ctx,
            NodeAddRequest {
                kind: "problem".into(),
                title: "client plan".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .value
        .node
        .id;
    let job = umx_services::EmbedJob {
        alias: "vault".into(),
        node: id,
    };
    let instance = svc.embedding().embedder_for(svc.graph(), "vault").unwrap();
    assert_eq!(instance.name, "stub");
    let planned = svc.embed_plan(&job).unwrap().unwrap();
    let batch = svc
        .embedding()
        .still_sendable(svc.graph(), &instance.embedder, vec![planned]);
    let (vectors, _) = umx_services::embedding::embed(&*instance.embedder, &batch).unwrap();
    let (applied, warnings) = svc.embed_apply(&vectors).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(applied.vectors_written > 0);
    assert!(stub.texts_embedded() > 0);
    assert!(
        !path.join("embeddings").exists(),
        "a secured graph's vectors never go into its folder"
    );
}

type LockFn = fn(&Path) -> std::io::Result<std::fs::File>;
type SpaceFn = fn(&Path) -> std::io::Result<u64>;

/// Whether `<folder or an ancestor>/.models.lock` is held by someone else right now: a competing try_lock.
fn models_lock_is_held(at: &Path) -> bool {
    let lock = at
        .ancestors()
        .map(|a| a.join(".models.lock"))
        .find(|l| l.exists())
        .expect("the models lock file exists while a writer holds it");
    let f = std::fs::File::open(lock).unwrap();
    matches!(f.try_lock(), Err(std::fs::TryLockError::WouldBlock))
}

static WIDENER_SAW_LOCK_HELD: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static FETCHER_SAW_LOCK_HELD: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// The real free space, after noting whether the models lock is held at that moment.
fn widener_space(at: &Path) -> std::io::Result<u64> {
    WIDENER_SAW_LOCK_HELD.store(models_lock_is_held(at), std::sync::atomic::Ordering::SeqCst);
    umx_adapter_fs::available_bytes(at)
}

/// No space at all, so the fetch stops before it downloads anything, after noting whether the lock is held.
fn fetcher_space(at: &Path) -> std::io::Result<u64> {
    FETCHER_SAW_LOCK_HELD.store(models_lock_is_held(at), std::sync::atomic::Ordering::SeqCst);
    Ok(0)
}

/// Both writers in the models folder, the model fetcher (s14) and the local embedder's CPU file (s16), get the real
/// `umx_adapter_fs::models_folder_lock` and `available_bytes` from compose, and hold that lock while they check the
/// space and until they rename: inside each one's space callback, a competing try_lock is blocked.
#[test]
fn both_models_folder_writers_hold_the_real_lock_from_space_check_to_rename() {
    use std::ptr::fn_addr_eq;
    use umx_adapter_llamacpp::{Execution, ModelFile, ModelSpec, Pooling};
    use umx_core::ports::{
        Embedder, ModelFetcher,
        model_fetcher::{FetchError, ModelFileSpec, ModelSource},
    };
    let real_lock: LockFn = umx_adapter_fs::models_folder_lock;
    let real_space: SpaceFn = umx_adapter_fs::available_bytes;
    let hooks = umx_compose::models_hooks();
    assert!(
        hooks.lock.is_some_and(|f| fn_addr_eq(f, real_lock)),
        "{hooks:?}"
    );
    assert!(
        hooks.available.is_some_and(|f| fn_addr_eq(f, real_space)),
        "{hooks:?}"
    );

    // What build_local gives a configured `local` instance.
    let t = temp("models-lock");
    let config = config(&t.0);
    std::fs::create_dir_all(&config.home).unwrap();
    std::fs::write(
        config.config_file(),
        "embedder.active = \"gemma\"\nembedders.gemma.kind = \"local\"\nembedders.gemma.model = \"embeddinggemma-2\"\n\
         embedders.gemma.dims = 256\n",
    )
    .unwrap();
    let settings = compose_config(Mode::Real, &config);
    let embedders = umx_compose::embedders(Mode::Real, &config, &settings);
    let local = embedders
        .get("gemma")
        .and_then(|i| i.local.clone())
        .expect("gemma is built");
    assert!(
        local
            .config()
            .models_lock
            .is_some_and(|f| fn_addr_eq(f, real_lock))
    );
    assert!(
        local
            .config()
            .available_bytes
            .is_some_and(|f| fn_addr_eq(f, real_space))
    );
    assert_eq!(local.config().models_dir, config.models_dir);

    // The widener, with compose's real lock, over the adapter's BF16 stub.
    let models = t.0.join("models");
    let dir = models.join("umx--stub/fixture");
    std::fs::create_dir_all(&dir).unwrap();
    let fixtures =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../umx-adapter-llamacpp/tests/fixtures");
    std::fs::copy(fixtures.join("stub-bf16.gguf"), dir.join("stub-bf16.gguf")).unwrap();
    let spec = ModelSpec {
        name: "stub".to_string(),
        repo: "umx/stub".to_string(),
        commit: "fixture".to_string(),
        file: ModelFile {
            path: "stub-bf16.gguf".to_string(),
            sha256: "5e3bf044c8a0692d921cf17a50a483c4164c99ba6df9b028d23542e7178d6344".to_string(),
        },
        cpu_file: Some(ModelFile {
            path: "stub-f32.gguf".to_string(),
            sha256: "9956c225740b6555687342bbe5e038abef110ac9be07f821c0ff71666737c8ad".to_string(),
        }),
        pooling: Pooling::Mean,
        dims: vec![16, 8],
        max_tokens: 64,
        query_prefix: "q: ".to_string(),
        document_prefix: String::new(),
        prompt_version: 1,
    };
    let mut c = umx_compose::local_config(
        spec,
        models.clone(),
        umx_compose::ModelsHooks {
            available: Some(widener_space),
            ..hooks
        },
    );
    c.dims = 8;
    c.threads = 1;
    c.execution = Execution::Cpu;
    umx_compose::local_embedder(c).unwrap().ready().unwrap();
    assert!(
        dir.join("stub-f32.gguf").is_file(),
        "the CPU file was made and renamed into place"
    );
    assert!(
        WIDENER_SAW_LOCK_HELD.load(std::sync::atomic::Ordering::SeqCst),
        "widener: lock not held at the space check"
    );
    assert!(!models_lock_is_held(&models), "released after the rename");

    // The fetcher, with compose's real lock; no space, so it stops before any download.
    let fetcher = umx_compose::hf_model_fetcher(umx_compose::ModelsHooks {
        available: Some(fetcher_space),
        ..hooks
    });
    let source = ModelSource {
        repo: "umx/absent".to_string(),
        commit: "0".repeat(40),
        files: vec![ModelFileSpec {
            path: "absent.gguf".to_string(),
            sha256: "0".repeat(64),
            size: 10,
        }],
    };
    let got = fetcher.fetch(&source, models.to_str().unwrap(), &mut |_| {});
    assert!(matches!(got, Err(FetchError::Space { .. })), "{got:?}");
    assert!(
        FETCHER_SAW_LOCK_HELD.load(std::sync::atomic::Ordering::SeqCst),
        "fetcher: lock not held at the space check"
    );
    assert!(!models_lock_is_held(&models), "released after the fetch");
}

//! Always-on tests over a tiny synthetic GGUF (tests/fixtures/make_stub.py): a one-layer, 16-wide llama-architecture
//! model with random weights and a character vocabulary. They run everywhere on the CPU, CI included, with no model
//! download.

use std::path::PathBuf;

use umx_adapter_llamacpp::{
    BUILT_GPU, Backend, Execution, LOCAL_EMBEDDER_CPU, LOCAL_EMBEDDER_SLOW, LlamaConfig,
    LlamaEmbedder, MODEL_DISK_SPACE, ModelFile, ModelSpec, Pooling,
};
use umx_contracts::embedder::{Reach, cosine, ready_sends_no_text};
use umx_core::ports::{EmbedError, Embedder, Locality, TextKind};

const STUB_SHA256: &str = "9a8ae47fec88f720b91ce079afdd04bbfd699f3b33024088dd8948385faf52a6";
/// The same stub with its matrices in BF16, and its F32 widening made by gguf-py (tools/embed-bakeoff/widen_f32.py).
const STUB_BF16_SHA256: &str = "5e3bf044c8a0692d921cf17a50a483c4164c99ba6df9b028d23542e7178d6344";
const STUB_WIDENED_SHA256: &str =
    "9956c225740b6555687342bbe5e038abef110ac9be07f821c0ff71666737c8ad";
/// The stub's window in tokens (one per character, plus BOS and EOS).
const STUB_MAX_TOKENS: usize = 64;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn stub_spec() -> ModelSpec {
    ModelSpec {
        name: "stub".to_string(),
        repo: "umx/stub".to_string(),
        commit: "fixture".to_string(),
        file: ModelFile {
            path: "stub.gguf".to_string(),
            sha256: STUB_SHA256.to_string(),
        },
        cpu_file: None,
        pooling: Pooling::Mean,
        dims: vec![16, 8],
        max_tokens: STUB_MAX_TOKENS,
        query_prefix: "q: ".to_string(),
        document_prefix: String::new(),
        prompt_version: 1,
    }
}

fn config(spec: ModelSpec) -> LlamaConfig {
    let mut c = LlamaConfig::new(spec, fixtures().join("models"));
    c.dims = 8;
    c.threads = 1;
    c.execution = Execution::Cpu;
    c
}

fn stub(reach: Reach) -> LlamaEmbedder {
    let mut c = config(stub_spec());
    if reach == Reach::Offline {
        // No model file at all: what a machine that never fetched the model sees.
        c.models_dir = fixtures().join("no-models-here");
    }
    LlamaEmbedder::new(c).unwrap()
}

umx_contracts::embedder_contract!(stub;
    em_c1_one_vector_per_input_in_order(),
    em_c2_width_is_key_dims(),
    em_c3_vectors_are_unit_length(),
    em_c4_same_text_and_kind_give_the_same_vector(),
    em_c5_empty_batch_is_empty_even_offline(),
    em_c6_distinct_texts_give_distinct_vectors(),
    em_c7_identical_texts_in_one_batch_match(),
    em_c8_blank_input_is_refused_by_index(),
    em_c9_offline_is_unavailable(),
    em_c10_over_cap_input_is_refused_or_reported(),
    em_c11_ready_is_ok_online(),
    em_c12_a_hosted_adapter_names_its_credential_after_ready(),
    em_c13_reported_tokens_are_none_or_count_the_batch(),
    em_c14_a_batch_of_max_batch_inputs_embeds(),
);

fn texts(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

#[test]
fn it_is_local_and_says_so() {
    let e = stub(Reach::Online);
    assert_eq!(e.locality(), Locality::Local);
    assert_eq!(e.adapter_id(), "llamacpp.embedder");
    e.ready().unwrap();
    assert_eq!(e.credential(), None, "a local model has no credential");
    assert_eq!(e.key().to_string(), "stub@8+p1");
    assert_eq!(
        (e.concurrency_ceiling(), e.max_input_tokens()),
        (1, STUB_MAX_TOKENS)
    );
}

#[test]
fn ready_embeds_nothing() {
    for reach in [Reach::Online, Reach::Offline] {
        let e = stub(reach);
        ready_sends_no_text(&e, || e.texts_embedded());
    }
    let e = stub(Reach::Online);
    e.embed(&texts(&["a", "b"]), TextKind::Document).unwrap();
    assert_eq!(e.texts_embedded(), 2, "the counter sees embeds");
}

#[test]
fn a_file_with_the_wrong_hash_is_refused_by_name() {
    let mut spec = stub_spec();
    spec.file.sha256 = "0".repeat(64);
    match LlamaEmbedder::new(config(spec)).unwrap().ready() {
        Err(EmbedError::Unavailable { reason, next }) => {
            assert!(reason.contains("stub.gguf"), "{reason}");
            assert!(reason.contains(STUB_SHA256), "{reason}");
            assert!(next.starts_with("delete it"), "{next}");
        }
        other => panic!("expected Unavailable, got {other:?}"),
    }
}

#[test]
fn a_missing_file_points_at_the_fetch_step() {
    match stub(Reach::Offline).embed(&texts(&["hello"]), TextKind::Query) {
        Err(EmbedError::Unavailable { reason, next }) => {
            assert!(reason.contains("stub.gguf"), "{reason}");
            assert!(next.contains("umx/stub at commit fixture"), "{next}");
            assert!(next.contains("umx models fetch"), "{next}");
        }
        other => panic!("expected Unavailable, got {other:?}"),
    }
}

#[test]
fn queries_get_the_prefix_and_documents_nothing() {
    let e = stub(Reach::Online);
    let query = e.embed(&texts(&["soup"]), TextKind::Query).unwrap();
    let as_doc = e.embed(&texts(&["q: soup"]), TextKind::Document).unwrap();
    let doc = e.embed(&texts(&["soup"]), TextKind::Document).unwrap();
    assert!(cosine(&query.vectors[0], &as_doc.vectors[0]) > 0.9999);
    assert!(cosine(&query.vectors[0], &doc.vectors[0]) < 0.99);
}

#[test]
fn long_inputs_are_cut_and_reported_across_runs() {
    let mut c = config(stub_spec());
    c.batch_size = 2;
    let e = LlamaEmbedder::new(c).unwrap();
    let long = "x".repeat(STUB_MAX_TOKENS * 2);
    let out = e
        .embed(&texts(&["a", "b", "c", &long, "d"]), TextKind::Document)
        .unwrap();
    assert_eq!(out.truncated, vec![3]);
    assert_eq!(out.vectors.len(), 5);
    // Inside the window (BOS, the leading-space token and EOS included): not reported.
    let fits = "y".repeat(STUB_MAX_TOKENS - 4);
    assert!(
        e.embed(&texts(&[&fits]), TextKind::Document)
            .unwrap()
            .truncated
            .is_empty()
    );
}

#[test]
fn run_size_does_not_change_vectors() {
    let batch = texts(&["short", "a somewhat longer text here", "mid length", "z"]);
    let together = stub(Reach::Online)
        .embed(&batch, TextKind::Document)
        .unwrap();
    let mut c = config(stub_spec());
    c.batch_size = 1;
    let each = LlamaEmbedder::new(c)
        .unwrap()
        .embed(&batch, TextKind::Document)
        .unwrap();
    for (a, b) in together.vectors.iter().zip(&each.vectors) {
        assert!(cosine(a, b) > 0.9999);
    }
}

#[test]
fn a_session_pool_raises_the_ceiling() {
    let mut c = config(stub_spec());
    c.sessions = 2;
    let e = LlamaEmbedder::new(c).unwrap();
    assert_eq!(e.concurrency_ceiling(), 2);
    std::thread::scope(|s| {
        for _ in 0..4 {
            s.spawn(|| e.embed(&texts(&["a", "b"]), TextKind::Query).unwrap());
        }
    });
}

#[test]
fn bad_configuration_is_refused() {
    let mut c = config(stub_spec());
    c.dims = 5;
    assert!(matches!(LlamaEmbedder::new(c), Err(EmbedError::Config(m)) if m.contains("[16, 8]")));
    let mut c = config(stub_spec());
    c.sessions = 0;
    assert!(matches!(LlamaEmbedder::new(c), Err(EmbedError::Config(m)) if m.contains("sessions")));
    let mut spec = stub_spec();
    spec.dims = vec![32, 8];
    match LlamaEmbedder::new(config(spec)).unwrap().ready() {
        Err(EmbedError::Config(m)) => assert!(m.contains("16-wide"), "{m}"),
        other => panic!("a spec wider than the model must be refused, got {other:?}"),
    }
}

#[test]
fn the_shipped_spec_is_found_by_name() {
    let spec = ModelSpec::by_name("embeddinggemma-2").unwrap();
    let e = LlamaEmbedder::new(LlamaConfig::new(spec, fixtures().join("no-models-here"))).unwrap();
    assert_eq!(e.key().to_string(), "embeddinggemma-2@256+p1");
    assert!(ModelSpec::by_name("nomic-embed-text-v1.5").is_none());
}

/// Run by `llama_cpp_writes_nothing_to_stderr` in a child process; ignored on its own.
#[test]
#[ignore = "child of llama_cpp_writes_nothing_to_stderr"]
fn child_embeds_a_marked_text() {
    let e = stub(Reach::Online);
    e.embed(&texts(&["MARKER-7f3a secret words"]), TextKind::Query)
        .unwrap();
    let long = "MARKER-7f3a ".repeat(20);
    e.embed(&texts(&[&long]), TextKind::Document).unwrap();
}

/// llama.cpp logs (model load, GPU init, tokens) are off: a process that loads the model and embeds a marked text
/// writes nothing to stderr, so no prompt or token text can reach a terminal or daemon.log (req-48).
#[test]
fn llama_cpp_writes_nothing_to_stderr() {
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "child_embeds_a_marked_text",
            "--test-threads=1",
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "child failed: {stdout} {stderr}");
    assert!(stdout.contains("1 passed"), "the child must run: {stdout}");
    assert_eq!(stderr.trim(), "", "llama.cpp wrote to stderr");
    assert!(!stdout.contains("MARKER-7f3a"), "text reached stdout");
}

/// CPU-only is never silent: with no CPU file (BF16 on the CPU) a `local_embedder_slow` warning names the backend.
#[test]
fn cpu_only_with_no_cpu_file_says_it_is_slow() {
    let cpu = stub(Reach::Online);
    assert_eq!(cpu.backend(), Backend::Cpu);
    let w = cpu.slow_warning().unwrap();
    assert!(
        w.starts_with(LOCAL_EMBEDDER_SLOW) && w.contains("cpu"),
        "{w}"
    );
    assert_eq!(cpu.cpu_note(), None);
}

/// With the CPU file the embedder is at the CPU floor: a `local_embedder_cpu` note with the measured rate once enough
/// texts are embedded, and the warning only when this machine measures under the floor.
#[test]
fn the_cpu_floor_decides_between_a_note_and_a_warning() {
    let (models, spec) = bf16_stub("floor");
    let batch: Vec<String> = (0..32).map(|i| format!("text number {i}")).collect();
    let at_floor = in_folder(&models, spec.clone());
    // Nothing measured yet: the estimate from the threads (1 in these tests) is under the floor.
    assert!(at_floor.slow_warning().unwrap().contains("about 0.9"));
    let mut c = config(spec.clone());
    c.models_dir = models.clone();
    c.threads = 4;
    let estimated = LlamaEmbedder::new(c).unwrap();
    assert_eq!(estimated.slow_warning(), None);
    assert!(estimated.cpu_note().unwrap().contains("about 3.6"));
    at_floor.embed(&batch, TextKind::Document).unwrap();
    assert!(at_floor.measured_texts_per_s().unwrap() > 2.0);
    assert_eq!(at_floor.slow_warning(), None);
    assert!(at_floor.cpu_note().unwrap().starts_with(LOCAL_EMBEDDER_CPU));

    let mut c = config(spec);
    c.models_dir = models.clone();
    c.cpu_floor = 1e12;
    let under = LlamaEmbedder::new(c).unwrap();
    under.embed(&batch[..31], TextKind::Document).unwrap();
    assert_eq!(
        under.measured_texts_per_s(),
        None,
        "31 texts are too few to judge"
    );
    assert_eq!(under.cpu_texts_per_s(), Some(0.9), "so the estimate stands");
    under.embed(&batch[..1], TextKind::Document).unwrap();
    let measured = under.measured_texts_per_s().unwrap();
    assert_eq!(under.cpu_texts_per_s(), Some(measured));
    let w = under.slow_warning().unwrap();
    assert!(
        w.starts_with(LOCAL_EMBEDDER_SLOW)
            && w.contains(&format!("about {measured:.1} texts"))
            && w.contains("under the CPU floor"),
        "{w}"
    );
    assert_eq!(under.cpu_note(), None);
    std::fs::remove_dir_all(&models).unwrap();
}

/// No room for the CPU file: refused with model_disk_space before anything is written.
#[test]
fn no_room_for_the_cpu_file_writes_nothing() {
    let (models, spec) = bf16_stub("no-room");
    let mut c = config(spec);
    c.models_dir = models.clone();
    c.available_bytes = Some(|_| Ok(1024));
    match LlamaEmbedder::new(c).unwrap().ready() {
        Err(EmbedError::Unavailable { reason, next }) => {
            assert!(
                reason.starts_with(MODEL_DISK_SPACE) && reason.contains("stub-f32.gguf"),
                "{reason}"
            );
            assert!(next.starts_with("free "), "{next}");
        }
        other => panic!("expected Unavailable, got {other:?}"),
    }
    assert_eq!(files(&models.join("umx--stub/fixture")), ["stub-bf16.gguf"]);
    std::fs::remove_dir_all(&models).unwrap();
}

/// The default (GPU) config, in a process where llama.cpp sees `devices` GPU devices (GGML_METAL_DEVICES on a Mac,
/// CUDA_VISIBLE_DEVICES in a cuda build, set by the parent). The report must match what llama.cpp actually has, read
/// from its device list (a second source), and the embedder must still embed: with no device it falls back to the CPU
/// and says so. A cuda build on a machine with no NVIDIA driver lists no GPU either way and must fall back the same.
fn gpu_config_reports_what_runs(devices: usize) {
    let (models, spec) = bf16_stub(&format!("devices-{devices}"));
    let mut c = config(spec);
    c.models_dir = models.clone();
    c.execution = Execution::Gpu;
    c.threads = 4;
    let e = LlamaEmbedder::new(c).unwrap();
    let backend = e.backend();
    let gpu_listed = llama_cpp_2::list_llama_ggml_backend_devices()
        .iter()
        .any(|d| {
            matches!(
                d.device_type,
                llama_cpp_2::LlamaBackendDeviceType::Gpu
                    | llama_cpp_2::LlamaBackendDeviceType::IntegratedGpu
            )
        });
    println!("devices={devices} gpu_listed={gpu_listed} backend={backend:?}");
    if devices == 0 {
        assert!(!gpu_listed, "a device count of 0 must hide the GPU");
    }
    match BUILT_GPU.filter(|_| gpu_listed) {
        Some(gpu) => assert_eq!(
            (backend, e.slow_warning(), e.cpu_note(), e.fallback_reason()),
            (gpu, None, None, None)
        ),
        None => {
            // The CPU file brings it to the CPU floor: a note, not a warning.
            assert_eq!((backend, e.slow_warning()), (Backend::Cpu, None));
            let note = e.cpu_note().unwrap();
            assert!(note.starts_with(LOCAL_EMBEDDER_CPU));
            // A cuda build says why it is on the CPU; a Metal build's report is unchanged.
            let why = (BUILT_GPU == Some(Backend::Cuda))
                .then_some("this cuda build found no CUDA device");
            assert_eq!(e.fallback_reason(), why);
            assert_eq!(note.contains("no CUDA device"), why.is_some(), "{note}");
        }
    }
    assert_eq!(
        e.embed(&texts(&["fallback works"]), TextKind::Query)
            .unwrap()
            .vectors
            .len(),
        1
    );
    // The CPU path loads the F32 copy, made on first use; a GPU loads the BF16 file and makes nothing.
    let made = models.join("umx--stub/fixture/stub-f32.gguf").exists();
    println!("devices={devices} f32_made={made}");
    assert_eq!(made, backend == Backend::Cpu);
    std::fs::remove_dir_all(&models).unwrap();
}

#[test]
#[ignore = "child of the_backend_report_follows_the_devices"]
fn child_gpu_config_with_no_device() {
    gpu_config_reports_what_runs(0);
}

#[test]
#[ignore = "child of the_backend_report_follows_the_devices"]
fn child_gpu_config_with_one_device() {
    gpu_config_reports_what_runs(1);
}

/// R89-1: with the GPU chosen but no Metal (or, in a cuda build, CUDA) device, llama.cpp runs on the CPU; the adapter
/// must report CPU and warn. The one-device run is the positive control (Metal on a Mac with a GPU, CUDA on a machine
/// with an NVIDIA GPU in a cuda build).
#[test]
fn the_backend_report_follows_the_devices() {
    for (devices, child) in [
        ("0", "child_gpu_config_with_no_device"),
        ("1", "child_gpu_config_with_one_device"),
    ] {
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                child,
                "--nocapture",
                "--test-threads=1",
            ])
            .env("GGML_METAL_DEVICES", devices)
            .env(
                "CUDA_VISIBLE_DEVICES",
                if devices == "0" { "-1" } else { "0" },
            )
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        for line in stdout
            .lines()
            .filter_map(|l| l.find("devices=").map(|i| &l[i..]))
        {
            eprintln!("{line}");
        }
        assert!(
            out.status.success() && stdout.contains("1 passed"),
            "{child}: {stdout}"
        );
    }
}

fn symbols(file: &std::path::Path) -> String {
    let out = std::process::Command::new("nm")
        .arg(file)
        .output()
        .expect("nm (Xcode tools or binutils) is needed for this check");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Security: llama-cpp-sys-2 builds llama.cpp's `common` library (its default feature), which holds an HTTP
/// client (httplib) and model download code. Nothing here calls it, so the linker must drop it: this test binary links
/// the adapter and llama.cpp, and its symbols must show llama.cpp (control) and none of that code. The positive
/// control finds the same names in the built `libllama-common.a`, so a rename cannot make this pass by accident.
#[test]
fn no_download_code_is_linked() {
    const BANNED: [&str; 3] = ["httplib", "common_download", "curl_easy"];
    let exe = std::env::current_exe().unwrap();
    let linked = symbols(&exe);
    assert!(
        linked.contains("llama_decode"),
        "control: llama.cpp must be linked"
    );
    for bad in BANNED {
        assert!(
            !linked.contains(bad),
            "{bad} code is linked into {}",
            exe.display()
        );
    }
    // target/<profile>/deps/<test> -> target/<profile>/build/llama-cpp-sys-2-*/out/lib/libllama-common.a
    let build = exe.parent().unwrap().parent().unwrap().join("build");
    let archive = std::fs::read_dir(&build)
        .unwrap()
        .filter_map(Result::ok)
        .map(|d| d.path().join("out/lib/libllama-common.a"))
        .find(|p| p.is_file())
        .expect("libllama-common.a under target/<profile>/build");
    let archived = symbols(&archive);
    assert!(
        archived.contains("httplib") && archived.contains("common_download"),
        "positive control: the archive must hold the names this test bans"
    );
}

/// ws004 s19: llama.cpp's CUDA backend is linked only with the `cuda` feature. Without it this test binary (which
/// links the adapter and llama.cpp) holds no CUDA or cuBLAS code; with it, the same names must be there, which is the
/// positive control that the instrument can see them (run by `just check-cuda` on a machine with the toolkit).
#[test]
fn cuda_code_is_linked_only_with_the_feature() {
    const CUDA: [&str; 2] = ["ggml_backend_cuda_reg", "cublas"];
    let exe = std::env::current_exe().unwrap();
    let linked = symbols(&exe);
    assert!(
        linked.contains("llama_decode"),
        "control: llama.cpp must be linked"
    );
    for name in CUDA {
        assert_eq!(
            linked.contains(name),
            cfg!(feature = "cuda"),
            "{name} in {} with cuda {}",
            exe.display(),
            if cfg!(feature = "cuda") { "on" } else { "off" }
        );
    }
}

/// A worker that does not answer in time fails the call (never a hang) and is set aside; dropping the embedder waits
/// for it, so the process exits cleanly (a worker left running crashed the test binary at exit). The worker is held
/// before the job, so the deadline is missed for certain; a watchdog turns a missing timeout into a failure, not a hang.
#[test]
fn a_late_worker_fails_the_call_and_is_set_aside() {
    let mut c = config(stub_spec());
    c.timeout_per_text = std::time::Duration::from_millis(50);
    let e = LlamaEmbedder::new(c).unwrap();
    e.ready().unwrap();
    let held = e.hold_workers_for_test();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::scope(|s| {
        s.spawn(|| {
            let _ = tx.send(e.embed(&texts(&["held"]), TextKind::Document));
        });
        let result = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("embed did not return: no timeout");
        match result {
            Err(EmbedError::BadResponse(m)) => assert!(m.contains("did not answer"), "{m}"),
            other => panic!("expected a timeout, got {other:?}"),
        }
        drop(held);
    });
    drop(e);
}

/// F207: `release` waits for a call already running (it gets its vector), frees the sessions, and the model never
/// loads again: later calls are `Unavailable`, and a second release returns at once. The worker is held so the call
/// is certainly in flight when the release starts.
#[test]
fn release_lets_a_running_call_finish_then_refuses_loads() {
    let e = LlamaEmbedder::new(config(stub_spec())).unwrap();
    e.ready().unwrap();
    let held = e.hold_workers_for_test();
    std::thread::scope(|s| {
        let running = s.spawn(|| e.embed(&texts(&["in flight"]), TextKind::Document));
        while e.texts_embedded() == 0 {
            std::thread::yield_now();
        }
        let release = s.spawn(|| e.release());
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(
            !release.is_finished(),
            "release returned while a worker still held the model"
        );
        drop(held);
        release.join().unwrap();
        assert_eq!(running.join().unwrap().unwrap().vectors.len(), 1);
    });
    for after in [
        e.embed(&texts(&["after"]), TextKind::Query).map(drop),
        e.ready(),
    ] {
        match after {
            Err(EmbedError::Unavailable { reason, .. }) => {
                assert!(reason.contains("released"), "{reason}")
            }
            other => panic!("a released model must not load again, got {other:?}"),
        }
    }
    e.release();
    assert_eq!(e.texts_embedded(), 1);
}

/// R348-B1: workers set aside by one call's timeout end once their queued jobs are done, even while a second call still
/// holds them, and release joins them holding no lock that call needs when it times out in turn. Before, release held
/// `late` while joining and the second call, waiting for `late`, kept their queue open: each waited on the other for
/// ever. One held session takes both calls (deadlines 100 and 250 ms; 4 texts, so the CPU path sends them as one
/// job); release starts after the first times out, the hold lifts after the second does. Every wait is bounded, so a
/// deadlock fails the test instead of hanging it.
#[test]
fn release_does_not_wait_on_a_call_still_holding_timed_out_workers() {
    use std::sync::Arc;
    use std::sync::mpsc::channel;
    use std::time::Duration;
    let bound = Duration::from_secs(10);
    let mut c = config(stub_spec());
    c.timeout_per_text = Duration::from_millis(50);
    let e = Arc::new(LlamaEmbedder::new(c).unwrap());
    e.ready().unwrap();
    let held = e.hold_workers_for_test();
    let call = |n: usize| {
        let (tx, rx) = channel();
        let e = Arc::clone(&e);
        std::thread::spawn(move || {
            let batch: Vec<String> = (0..n).map(|i| format!("text {i}")).collect();
            let _ = tx.send(e.embed(&batch, TextKind::Document));
        });
        rx
    };
    let first = call(1);
    let second = call(4);
    let sent_by = std::time::Instant::now() + bound;
    while e.texts_embedded() < 5 {
        assert!(
            std::time::Instant::now() < sent_by,
            "both calls were not queued"
        );
        std::thread::yield_now();
    }
    match first
        .recv_timeout(bound)
        .expect("the first call did not time out")
    {
        Err(EmbedError::BadResponse(m)) => assert!(m.contains("did not answer"), "{m}"),
        other => panic!("expected the first call to time out, got {other:?}"),
    }
    let (done, released) = channel();
    let releasing = Arc::clone(&e);
    std::thread::spawn(move || {
        releasing.release();
        let _ = done.send(());
    });
    // Past the second call's deadline: it is timing out now, with the workers still held.
    std::thread::sleep(Duration::from_millis(400));
    drop(held);
    released
        .recv_timeout(bound)
        .expect("release hung on workers a timed-out call kept alive (R348-B1)");
    assert!(
        second
            .recv_timeout(bound)
            .expect("the second call never returned")
            .is_err()
    );
    assert_eq!(e.sessions_alive(), 0, "a worker outlived release");
}

/// A models folder of its own holding only the BF16 stub, and a spec whose CPU file is its F32 widening.
fn bf16_stub(name: &str) -> (PathBuf, ModelSpec) {
    let models = std::env::temp_dir().join(format!("umx-llamacpp-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&models);
    let dir = models.join("umx--stub/fixture");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(
        fixtures().join("stub-bf16.gguf"),
        dir.join("stub-bf16.gguf"),
    )
    .unwrap();
    let mut spec = stub_spec();
    spec.file = ModelFile {
        path: "stub-bf16.gguf".to_string(),
        sha256: STUB_BF16_SHA256.to_string(),
    };
    spec.cpu_file = Some(ModelFile {
        path: "stub-f32.gguf".to_string(),
        sha256: STUB_WIDENED_SHA256.to_string(),
    });
    (models, spec)
}

fn in_folder(models: &std::path::Path, spec: ModelSpec) -> LlamaEmbedder {
    let mut c = config(spec);
    c.models_dir = models.to_path_buf();
    LlamaEmbedder::new(c).unwrap()
}

fn files(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// On the CPU the adapter makes the F32 copy of the BF16 file beside it, byte for byte what gguf-py makes (a second
/// writer), keeps nothing else, and embeds as the BF16 file does.
#[test]
fn the_cpu_path_makes_an_exact_f32_copy_and_embeds_the_same() {
    let (models, spec) = bf16_stub("widen");
    let dir = models.join("umx--stub/fixture");
    let e = in_folder(&models, spec.clone());
    e.ready().unwrap();
    assert_eq!(files(&dir), ["stub-bf16.gguf", "stub-f32.gguf"]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join("stub-f32.gguf"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "the derived file is the owner's only");
    }
    assert!(
        std::fs::read(dir.join("stub-f32.gguf")).unwrap()
            == std::fs::read(fixtures().join("stub-bf16-widened.gguf")).unwrap(),
        "the adapter's widening differs from gguf-py's"
    );
    let batch = texts(&["short", "a somewhat longer text here", "z"]);
    let f32 = e.embed(&batch, TextKind::Document).unwrap();
    let mut bf16_spec = spec;
    bf16_spec.cpu_file = None;
    let bf16 = in_folder(&models, bf16_spec)
        .embed(&batch, TextKind::Document)
        .unwrap();
    for (a, b) in f32.vectors.iter().zip(&bf16.vectors) {
        assert!(cosine(a, b) > 0.9999, "{}", cosine(a, b));
    }
    std::fs::remove_dir_all(&models).unwrap();
}

/// A damaged or swapped CPU file is refused by name, and deleting it is the whole fix.
#[test]
fn a_cpu_file_with_the_wrong_hash_is_refused_and_made_again_once_deleted() {
    let (models, spec) = bf16_stub("bad-cpu-file");
    let cpu = models.join("umx--stub/fixture/stub-f32.gguf");
    std::fs::write(&cpu, b"not the widened model").unwrap();
    match in_folder(&models, spec.clone()).ready() {
        Err(EmbedError::Unavailable { reason, next }) => {
            assert!(reason.contains("stub-f32.gguf"), "{reason}");
            assert!(reason.contains(STUB_WIDENED_SHA256), "{reason}");
            assert!(
                next.starts_with("delete it; umx makes it again from")
                    && next.contains("stub-bf16.gguf"),
                "{next}"
            );
        }
        other => panic!("expected Unavailable, got {other:?}"),
    }
    std::fs::remove_file(&cpu).unwrap();
    in_folder(&models, spec).ready().unwrap();
    std::fs::remove_dir_all(&models).unwrap();
}

/// A widening whose hash is not the pinned one is refused and leaves no file behind.
#[test]
fn a_widening_that_misses_its_pin_keeps_nothing() {
    let (models, mut spec) = bf16_stub("bad-pin");
    let dir = models.join("umx--stub/fixture");
    spec.cpu_file.as_mut().unwrap().sha256 = "0".repeat(64);
    match in_folder(&models, spec).ready() {
        Err(EmbedError::Unavailable { reason, .. }) => {
            assert!(
                reason.contains(STUB_WIDENED_SHA256) && reason.contains("not the pinned"),
                "{reason}"
            );
        }
        other => panic!("expected Unavailable, got {other:?}"),
    }
    assert_eq!(files(&dir), ["stub-bf16.gguf"]);
    std::fs::remove_dir_all(&models).unwrap();
}

/// Widening a file that is all F32 already gives it back byte for byte (only `general.file_type` and tensor types
/// change, and here they are F32 already).
#[test]
fn widening_an_f32_file_changes_nothing() {
    let out = std::env::temp_dir().join(format!("umx-llamacpp-f32-{}.gguf", std::process::id()));
    let _ = std::fs::remove_file(&out);
    let sha = umx_adapter_llamacpp::widen_bf16_to_f32(
        &fixtures().join("models/umx--stub/fixture/stub.gguf"),
        &out,
    )
    .unwrap();
    assert_eq!(sha, STUB_SHA256);
    std::fs::remove_file(&out).unwrap();
}

/// R110-1: CPU embedders warming one empty models folder at once all get the F32 file; none removes another's
/// temporary file. Many rounds of eight, because one round of a 64 KB widening rarely overlaps.
#[test]
fn concurrent_initialisers_share_one_widening() {
    for round in 0..20 {
        let lock = (round % 2 == 1)
            .then_some(test_lock as fn(&std::path::Path) -> std::io::Result<std::fs::File>);
        let (models, spec) = bf16_stub(&format!("race-{round}"));
        let start = std::sync::Barrier::new(8);
        let results: Vec<Result<(), EmbedError>> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    s.spawn(|| {
                        let mut c = config(spec.clone());
                        c.models_dir = models.clone();
                        c.models_lock = lock;
                        let e = LlamaEmbedder::new(c).unwrap();
                        start.wait();
                        e.ready()
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for r in &results {
            assert!(r.is_ok(), "round {round}: {r:?}");
        }
        assert_eq!(
            files(&models.join("umx--stub/fixture")),
            ["stub-bf16.gguf", "stub-f32.gguf"]
        );
        std::fs::remove_dir_all(&models).unwrap();
    }
}

/// What compose passes (umx_adapter_fs::models_folder_lock): an exclusive lock on `<folder>/.models.lock`.
fn test_lock(folder: &std::path::Path) -> std::io::Result<std::fs::File> {
    let f = std::fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(folder.join(".models.lock"))?;
    f.lock()?;
    Ok(f)
}

/// A temporary file left by a crash is removed by the next widening that holds the models folder's lock; with no
/// lock nothing is removed, since another writer may own it; other files are never touched.
#[test]
fn the_lock_holder_removes_temporary_files_left_by_a_crash() {
    for locked in [false, true] {
        let (models, spec) = bf16_stub(&format!("stale-{locked}"));
        let dir = models.join("umx--stub/fixture");
        for f in ["stub-f32.gguf.1.0.part", "other.gguf.1.0.part"] {
            std::fs::write(dir.join(f), b"half a model").unwrap();
        }
        let mut c = config(spec);
        c.models_dir = models.clone();
        c.models_lock =
            locked.then_some(test_lock as fn(&std::path::Path) -> std::io::Result<std::fs::File>);
        LlamaEmbedder::new(c).unwrap().ready().unwrap();
        let mut want = vec!["other.gguf.1.0.part", "stub-bf16.gguf", "stub-f32.gguf"];
        if !locked {
            want.push("stub-f32.gguf.1.0.part");
        }
        assert_eq!(files(&dir), want, "locked {locked}");
        std::fs::remove_dir_all(&models).unwrap();
    }
}

/// While another writer holds the models folder's lock, the widening waits for it.
#[test]
fn the_widening_waits_for_the_models_lock() {
    let (models, spec) = bf16_stub("waits");
    let held = test_lock(&models).unwrap();
    let mut c = config(spec);
    c.models_dir = models.clone();
    c.models_lock = Some(test_lock);
    let e = LlamaEmbedder::new(c).unwrap();
    std::thread::scope(|s| {
        let warming = s.spawn(|| e.ready());
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!warming.is_finished(), "it must wait for the lock");
        assert!(!models.join("umx--stub/fixture/stub-f32.gguf").exists());
        drop(held);
        warming.join().unwrap().unwrap();
    });
    assert!(models.join("umx--stub/fixture/stub-f32.gguf").exists());
    std::fs::remove_dir_all(&models).unwrap();
}

/// Header sizes that overflow are refused, never wrapped (security N2).
#[test]
fn widening_refuses_sizes_that_overflow() {
    fn gguf(dims: &[u64], offset: u64) -> Vec<u8> {
        let mut b = b"GGUF".to_vec();
        b.extend(3u32.to_le_bytes());
        b.extend(1u64.to_le_bytes());
        b.extend(0u64.to_le_bytes());
        b.extend(1u64.to_le_bytes());
        b.push(b't');
        b.extend((dims.len() as u32).to_le_bytes());
        for d in dims {
            b.extend(d.to_le_bytes());
        }
        b.extend(30u32.to_le_bytes());
        b.extend(offset.to_le_bytes());
        b.resize(b.len() + 64, 0);
        b
    }
    let dir = std::env::temp_dir().join(format!("umx-llamacpp-overflow-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (i, (dims, offset)) in [
        (vec![u64::MAX / 2, 4], 0),
        (vec![1u64 << 62], 0),
        (vec![8], u64::MAX - 2),
    ]
    .into_iter()
    .enumerate()
    {
        let src = dir.join(format!("in-{i}.gguf"));
        std::fs::write(&src, gguf(&dims, offset)).unwrap();
        let err = umx_adapter_llamacpp::widen_bf16_to_f32(&src, &dir.join(format!("out-{i}.gguf")))
            .unwrap_err();
        assert_eq!(
            err.kind(),
            std::io::ErrorKind::InvalidData,
            "{dims:?} {offset}: {err}"
        );
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

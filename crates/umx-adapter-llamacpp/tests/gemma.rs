//! Tests on the real embeddinggemma-2 GGUF, run where it is in `~/.cache/umx/models` (fetch it with
//! `umx models fetch embeddinggemma-2`) and skipped with a message elsewhere, CI included: the file is 558 MB.
//!
//! Parity: the reference vectors are from the ws004 s03 bake-off (sentence-transformers at the same pinned model,
//! `beir_eval.py vectors embeddinggemma-2`): 20 fixture documents and 20 queries at 256 wide, synthetic text only.

use std::path::PathBuf;
use std::sync::Arc;

use umx_adapter_llamacpp::{Execution, LlamaConfig, LlamaEmbedder, ModelSpec};
use umx_contracts::embedder::{self as contract, Reach, cosine};
use umx_core::ports::{Credential, EmbedError, Embedded, Embedder, Locality, ModelKey, TextKind};

const QUERY_PREFIX: &str = "task: search result | query: ";

fn config() -> LlamaConfig {
    // Tests may not read env (r-env-in-compose); home_dir is only how this test finds the shared model cache.
    let home = std::env::home_dir().unwrap_or_default();
    LlamaConfig::new(
        ModelSpec::embeddinggemma_2(),
        home.join(".cache/umx/models"),
    )
}

/// None (and a note on stderr) when the model files are not on this machine.
fn present() -> Option<LlamaConfig> {
    let c = config();
    let gguf = c.model_dir().join(&c.spec.file.path);
    if gguf.is_file() {
        Some(c)
    } else {
        eprintln!(
            "skipped: {} is absent (fetch it with umx models fetch embeddinggemma-2)",
            gguf.display()
        );
        None
    }
}

fn reference() -> serde_json::Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/parity-ref-embeddinggemma-2.json");
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn strings(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect()
}

fn vectors(v: &serde_json::Value) -> Vec<Vec<f32>> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|row| {
            row.as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_f64().unwrap() as f32)
                .collect()
        })
        .collect()
}

/// On the GPU where there is one (Metal on macOS) and on the CPU, the path Linux and CI take. The CPU loads the F32
/// copy (made from the BF16 file on first use), so the CPU vectors are also held to the GPU's: the key is the same.
#[test]
fn parity_with_the_reference_vectors() {
    let Some(c) = present() else { return };
    let mut runs = Vec::new();
    for execution in [Execution::Gpu, Execution::Cpu] {
        let mut c = c.clone();
        c.execution = execution;
        let e = LlamaEmbedder::new(c).unwrap();
        eprintln!("{execution:?} runs on {:?}", e.backend());
        runs.push(parity(e, execution));
    }
    for (i, kind) in ["Document", "Query"].iter().enumerate() {
        let min = runs[0][i]
            .iter()
            .zip(&runs[1][i])
            .map(|(a, b)| cosine(a, b))
            .fold(1.0f32, f32::min);
        eprintln!("Cpu vs Gpu {kind}: min cosine {min:.5}");
        assert!(min >= 0.999, "Cpu vs Gpu {kind} {min}");
    }
}

/// ws004 s19: a cuda build on a machine whose llama.cpp lists a GPU (the second source) runs the default config on
/// CUDA, from the BF16 file under the same key, with parity against the reference. Skipped, with a message, when the
/// model is absent or no GPU is listed; not built without the feature.
#[cfg(feature = "cuda")]
#[test]
fn a_cuda_build_runs_the_model_on_cuda() {
    use umx_adapter_llamacpp::Backend;
    let Some(c) = present() else { return };
    assert_eq!(c.execution, Execution::Gpu, "the default in a cuda build");
    let e = LlamaEmbedder::new(c).unwrap();
    let backend = e.backend();
    let gpu_listed = llama_cpp_2::list_llama_ggml_backend_devices()
        .iter()
        .any(|d| matches!(d.device_type, llama_cpp_2::LlamaBackendDeviceType::Gpu));
    if !gpu_listed {
        eprintln!("skipped: llama.cpp lists no CUDA device here (backend {backend:?})");
        return;
    }
    assert_eq!((backend, e.fallback_reason()), (Backend::Cuda, None));
    assert_eq!(e.key().to_string(), "embeddinggemma-2@256+p1");
    parity(e, Execution::Gpu);
}

fn parity(e: LlamaEmbedder, execution: Execution) -> Vec<Vec<Vec<f32>>> {
    let r = reference();
    // The reference documents are already in the core text form, which is the model's document prompt; the
    // queries carry the query prompt, which the adapter adds itself.
    let docs = strings(&r["docs"]);
    let queries: Vec<String> = strings(&r["queries"])
        .iter()
        .map(|q| q.strip_prefix(QUERY_PREFIX).unwrap().to_string())
        .collect();
    let mut out = Vec::new();
    for (kind, texts, want) in [
        (TextKind::Document, docs, vectors(&r["doc_vectors_256"])),
        (TextKind::Query, queries, vectors(&r["query_vectors_256"])),
    ] {
        let got = e.embed(&texts, kind).unwrap();
        assert!(got.truncated.is_empty());
        let min = got
            .vectors
            .iter()
            .zip(&want)
            .map(|(a, b)| cosine(a, b))
            .fold(1.0f32, f32::min);
        eprintln!(
            "{execution:?} {kind:?}: min cosine {min:.5} over {}",
            want.len()
        );
        assert!(min >= 0.999, "{execution:?} {kind:?} parity {min}");
        out.push(got.vectors);
    }
    out
}

/// One loaded model shared by the contract's clauses: each load hashes 558 MB, which a debug build does slowly, and
/// the clauses are about behaviour, not loading (the stub tests cover loading).
struct Shared(Arc<LlamaEmbedder>);

impl Embedder for Shared {
    fn embed(&self, texts: &[String], kind: TextKind) -> Result<Embedded, EmbedError> {
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
        self.0.locality()
    }
    fn ready(&self) -> Result<(), EmbedError> {
        self.0.ready()
    }
    fn credential(&self) -> Option<Credential> {
        self.0.credential()
    }
    fn adapter_id(&self) -> &'static str {
        self.0.adapter_id()
    }
}

#[test]
fn the_contract_holds_on_the_real_model() {
    let Some(c) = present() else { return };
    let online = Arc::new(LlamaEmbedder::new(c).unwrap());
    let factory = |reach: Reach| match reach {
        Reach::Online => Shared(online.clone()),
        Reach::Offline => {
            let mut c = config();
            c.models_dir =
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/no-models-here");
            Shared(Arc::new(LlamaEmbedder::new(c).unwrap()))
        }
    };
    contract::em_c1_one_vector_per_input_in_order(factory);
    contract::em_c2_width_is_key_dims(factory);
    contract::em_c3_vectors_are_unit_length(factory);
    contract::em_c4_same_text_and_kind_give_the_same_vector(factory);
    contract::em_c5_empty_batch_is_empty_even_offline(factory);
    contract::em_c6_distinct_texts_give_distinct_vectors(factory);
    contract::em_c7_identical_texts_in_one_batch_match(factory);
    contract::em_c8_blank_input_is_refused_by_index(factory);
    contract::em_c9_offline_is_unavailable(factory);
    contract::em_c10_over_cap_input_is_refused_or_reported(factory);
    contract::em_c11_ready_is_ok_online(factory);
    contract::em_c12_a_hosted_adapter_names_its_credential_after_ready(factory);
}

/// Each text embedded alone matches the same text inside a run of 16 texts of different lengths (packing and the
/// attention mask leak nothing between texts), on the GPU and the CPU.
#[test]
fn batch_invariance_on_the_real_model() {
    let Some(c) = present() else { return };
    let docs: Vec<String> = strings(&reference()["docs"]).into_iter().take(16).collect();
    for execution in [Execution::Gpu, Execution::Cpu] {
        let mut c = c.clone();
        c.execution = execution;
        let e = LlamaEmbedder::new(c).unwrap();
        let together = e.embed(&docs, TextKind::Document).unwrap();
        let min = docs
            .iter()
            .zip(&together.vectors)
            .map(|(d, v)| {
                let alone = e
                    .embed(std::slice::from_ref(d), TextKind::Document)
                    .unwrap();
                cosine(&alone.vectors[0], v)
            })
            .fold(1.0f32, f32::min);
        eprintln!(
            "{execution:?} batch invariance: min cosine {min:.5} over {}",
            docs.len()
        );
        assert!(min >= 0.999, "{execution:?} batch invariance {min}");
    }
}

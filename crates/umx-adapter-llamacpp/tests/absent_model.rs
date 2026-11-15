//! CI: an instance whose model file was never fetched fails `ready()` at once, before llama.cpp's backend
//! starts (Metal on a Mac, which can take longer than a CLI test's deadline on a CI VM). Its own test binary, so no
//! other test has started the backend in this process.

use umx_adapter_llamacpp::{LlamaConfig, LlamaEmbedder, ModelSpec, backend_started};
use umx_core::ports::{EmbedError, Embedder};

#[test]
fn an_absent_model_fails_before_the_backend_starts() {
    let spec = ModelSpec::by_name("embeddinggemma-2").unwrap();
    assert!(
        spec.cpu_file.is_some(),
        "the shipped spec has a CPU file, so the backend question arises"
    );
    let dir = std::env::temp_dir().join(format!("umx-absent-model-{}", std::process::id()));
    let e = LlamaEmbedder::new(LlamaConfig::new(spec, dir)).unwrap();
    assert!(matches!(e.ready(), Err(EmbedError::Unavailable { .. })));
    assert!(
        !backend_started(),
        "llama.cpp started for a model that is not there"
    );
}

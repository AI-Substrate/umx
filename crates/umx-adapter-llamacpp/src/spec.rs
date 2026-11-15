//! What the adapter needs to know about one model: the GGUF file and its hash, pooling, widths, input window and
//! prompts. The GGUF carries its own tokenizer. Changing the model means changing only the spec.

/// How the token vectors become one vector per text. Always set explicitly on the llama.cpp context, never left to
/// the GGUF's metadata (embeddings-expert, s04).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pooling {
    /// Mean over the text's tokens.
    Mean,
}

/// A file in the model's folder, with its sha256 (lower-case hex).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelFile {
    pub path: String,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelSpec {
    /// The model in `ModelKey` form: `[a-z0-9._-]`.
    pub name: String,
    /// The Hugging Face repo and the commit the file comes from. It lives in
    /// `<models dir>/<repo with / as -->/<commit>/<file>`.
    pub repo: String,
    pub commit: String,
    /// The GGUF, checked before each load.
    pub file: ModelFile,
    /// The GGUF the CPU path loads instead, when the model has one: an exact F32 widening of `file`
    /// ([`crate::widen_bf16_to_f32`]), made from it beside it on first use and pinned by its own sha256. Same vectors,
    /// so the same key.
    pub cpu_file: Option<ModelFile>,
    pub pooling: Pooling,
    /// The widths the model may be cut to (Matryoshka), widest first; the widest is the native width.
    pub dims: Vec<usize>,
    /// The model's input window in tokens, special tokens included. Longer texts are cut and reported.
    pub max_tokens: usize,
    /// Put before each text of that kind. Bump `prompt_version` when either changes: it changes the vector space.
    pub query_prefix: String,
    pub document_prefix: String,
    pub prompt_version: u32,
}

impl ModelSpec {
    /// The specs this adapter ships, by `name`.
    pub fn by_name(name: &str) -> Option<ModelSpec> {
        match name {
            "embeddinggemma-2" => Some(ModelSpec::embeddinggemma_2()),
            _ => None,
        }
    }

    /// EmbeddingGemma 2 (Apache 2.0), ggml-org's BF16 GGUF, picked by the ws004 s03 bake-off and the GPU spike
    /// (D44, D46). The key `embeddinggemma-2@<dims>+p1` names this BF16 file only: another quantisation gives other
    /// vectors, so it would be another model name (for example `embeddinggemma-2-q8`); the Q8_0 file also misses the
    /// 0.999 parity gate. The CPU path loads its exact F32 widening under the same key (ws004 s16: parity with the s03
    /// reference and with BF16 on Metal holds both ways; BF16 on the CPU is about 16 times slower). Its document prompt,
    /// `title: <title> | text: <text>`, is the core's text form already (s12), so documents get no prefix here;
    /// queries get the card's search prompt.
    pub fn embeddinggemma_2() -> ModelSpec {
        // The file is pinned once, in umx-core (what `umx models fetch` fetches is what this loads).
        let source = umx_core::models::pinned("embeddinggemma-2").expect("pinned in umx-core");
        let file = &source.files[0];
        ModelSpec {
            name: "embeddinggemma-2".to_string(),
            file: ModelFile {
                path: file.path.clone(),
                sha256: file.sha256.clone(),
            },
            repo: source.repo,
            commit: source.commit,
            cpu_file: umx_core::models::cpu_file("embeddinggemma-2").map(|f| ModelFile {
                path: f.path,
                sha256: f.sha256,
            }),
            pooling: Pooling::Mean,
            dims: vec![768, 512, 256, 128],
            max_tokens: 2048,
            query_prefix: "task: search result | query: ".to_string(),
            document_prefix: String::new(),
            prompt_version: 1,
        }
    }
}

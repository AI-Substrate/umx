//! The model files umx can fetch (req-74): each pinned to a repo commit, a file, its sha256 and its size. One
//! source of truth: the local embedder's spec takes its file from here, and `umx models fetch` fetches what is here.

use crate::ports::model_fetcher::{ModelFileSpec, ModelSource};
use alloc::string::{String, ToString};
use alloc::vec;
use sha2::{Digest, Sha256};

/// A file's sha256, fed as it streams in: what a model fetch and its fake check before keeping a file.
#[derive(Clone, Default)]
pub struct FileHasher(Sha256);

impl FileHasher {
    pub fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    /// The sha256 as lower-case hex, as model specs pin it.
    pub fn hex(self) -> String {
        use core::fmt::Write;
        let mut out = String::with_capacity(64);
        for b in self.0.finalize() {
            let _ = write!(out, "{b:02x}");
        }
        out
    }
}

/// `bytes`' sha256 as lower-case hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = FileHasher::default();
    h.update(bytes);
    h.hex()
}

/// The names `umx models fetch` knows.
pub const NAMES: &[&str] = &["embeddinggemma-2"];

/// The pinned source of model `name`.
pub fn pinned(name: &str) -> Option<ModelSource> {
    match name {
        // EmbeddingGemma 2 (Apache 2.0), ggml-org's BF16 GGUF (ws004 s03, D44, D46; s04).
        "embeddinggemma-2" => Some(ModelSource {
            repo: "ggml-org/embeddinggemma-2-GGUF".to_string(),
            commit: "6ea3e3a309d12a4779cfda98b8c19800d2d5da71".to_string(),
            files: vec![ModelFileSpec {
                path: "embeddinggemma-2-BF16.gguf".to_string(),
                sha256: "68bae29d62fb8c7d23e98d21fd4662753ddd636e6b62b8a70dfe059a9844f216"
                    .to_string(),
                size: 557_950_176,
            }],
        }),
        _ => None,
    }
}

/// The file the CPU path loads instead of model `name`'s pinned file, when it has one: derived by the local embedder
/// (never fetched), and pinned here beside the file it is made from.
pub fn cpu_file(name: &str) -> Option<ModelFileSpec> {
    match name {
        // The exact F32 widening of embeddinggemma-2-BF16.gguf (same weights, same key), which umx-adapter-llamacpp
        // makes beside it on first CPU use; llama.cpp runs BF16 about 16 times slower on the CPU (ws004 s16).
        "embeddinggemma-2" => Some(ModelFileSpec {
            path: "embeddinggemma-2-F32.gguf".to_string(),
            sha256: "351a5215d6ec908ee228c3a621dd28445e42dff7a7dc2dee125a2946bc128aff".to_string(),
            size: 1_099_801_824,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::model_fetcher::safe_file_name;

    #[test]
    fn every_pinned_model_is_well_formed() {
        for name in NAMES {
            let s = pinned(name).unwrap();
            assert_eq!(s.commit.len(), 40, "{name}: a full commit");
            assert!(
                s.commit
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "{name}: a lowercase hex commit"
            );
            assert!(s.repo.split('/').count() == 2);
            for f in &s.files {
                assert!(safe_file_name(&f.path));
                assert_eq!(f.sha256.len(), 64);
                assert!(
                    f.sha256
                        .chars()
                        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
                );
                assert!(f.size > 0);
            }
        }
        assert!(pinned("nope").is_none());
    }

    #[test]
    fn a_cpu_file_is_well_formed_and_never_a_fetched_file() {
        for name in NAMES {
            let Some(f) = cpu_file(name) else { continue };
            assert!(safe_file_name(&f.path));
            assert_eq!(f.sha256.len(), 64);
            assert!(f.size > 0);
            assert!(
                pinned(name)
                    .unwrap()
                    .files
                    .iter()
                    .all(|p| p.path != f.path && p.sha256 != f.sha256),
                "{name}: the CPU file is derived, not fetched"
            );
        }
        assert!(cpu_file("nope").is_none());
    }

    #[test]
    fn sha256_hex_matches_a_known_digest() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn file_names_are_one_plain_component() {
        for bad in ["", "..", ".", ".hidden", "a/b", "a\\b", "x\ny", "../up"] {
            assert!(!safe_file_name(bad), "{bad:?}");
        }
        assert!(safe_file_name("embeddinggemma-2-BF16.gguf"));
    }
}

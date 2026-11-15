//! `umx models fetch <model> [--dry-run]` (req-74; ws004 s14): fetches a pinned model's files into the models
//! folder, through the ModelFetcher port. Runs in the CLI process like `umx doctor`, with no daemon: a download of
//! hundreds of MB never holds a daemon worker. Embed never downloads; this is the one way a model arrives.

use std::io::IsTerminal;
use std::path::Path;
use umx_core::ports::{FetchError, FetchProgress, ModelFetcher, ModelSource};
use umx_protocol::{Code, Envelope, Fill, Issue, json};

use crate::Config;

const CMD: &str = "models fetch";

/// `umx models fetch`: the composed fetcher, the models folder compose derives, and progress on stderr when it is
/// a terminal (sizes only).
pub fn models_fetch(config: &Config, name: &str, dry_run: bool) -> Envelope {
    let services = umx_compose::compose(crate::mode(), config);
    let models = config.models_dir.clone();
    let Some(fetcher) = services.model_fetcher() else {
        return Envelope::fail(CMD, crate::internal("no_model_fetcher"));
    };
    let show = std::io::stderr().is_terminal();
    let mut progress = |p: FetchProgress| {
        if show {
            const MB: u64 = 1024 * 1024;
            eprintln!("{}: {} of {} MB", p.file, p.done / MB, p.total.div_ceil(MB));
        }
    };
    fetch_with(
        name,
        dry_run,
        &models,
        fetcher.as_ref(),
        &umx_compose::available_bytes,
        &mut progress,
    )
}

/// The verb over its parts, so tests can give it a fake fetcher, a models folder and a free-space answer.
pub(crate) fn fetch_with(
    name: &str,
    dry_run: bool,
    models: &Path,
    fetcher: &dyn ModelFetcher,
    available: &dyn Fn(&Path) -> std::io::Result<u64>,
    progress: &mut dyn FnMut(FetchProgress),
) -> Envelope {
    let Some(source) = umx_core::models::pinned(name) else {
        return Envelope::fail(
            CMD,
            Issue::fill(
                Code::ModelUnknown,
                &[
                    ("model", Fill::text(name)),
                    ("known", Fill::text(&umx_core::models::NAMES.join(", "))),
                ],
            ),
        );
    };
    let models_text = models.display().to_string();
    let folder = source.folder(&models_text);
    // What must still arrive: every file not already there with the pinned sha256 (a same-size file that does not
    // verify is fetched in full, so it needs the full size; reviewer R112-2).
    let needed: u64 = source
        .files
        .iter()
        .filter(|f| !verified(&Path::new(&folder).join(&f.path), &f.sha256))
        .map(|f| f.size)
        .sum();
    let free = available(Path::new(&folder)).ok();
    if dry_run {
        let files: Vec<_> = source
            .files
            .iter()
            .map(|f| json!({ "file": f.path, "size": f.size, "sha256": f.sha256 }))
            .collect();
        return Envelope::ok(
            CMD,
            json!({
                "model": name,
                "repo": source.repo,
                "commit": source.commit,
                "folder": folder,
                "files": files,
                "needed": needed,
                "available": free,
                "dry_run": true,
            }),
        );
    }
    if needed > 0
        && let Some(free) = free
        && free < needed
    {
        return no_space(name, needed, &folder, free);
    }
    // The verb's check refuses early; the fetcher checks again under the models lock, which is the one that counts.
    match fetcher.fetch(&source, &models_text, progress) {
        Ok(files) => {
            let files: Vec<_> = files
                .iter()
                .map(|f| json!({ "file": f.file, "size": f.size, "downloaded": f.downloaded }))
                .collect();
            Envelope::ok(
                CMD,
                json!({ "model": name, "folder": folder, "files": files, "dry_run": false }),
            )
        }
        Err(FetchError::Space { needed, available }) => no_space(name, needed, &folder, available),
        Err(e) => failed(name, &e, Some(&source)),
    }
}

/// Whether `path` is a regular file (not a symlink) whose bytes hash to `sha256`.
fn verified(path: &Path, sha256: &str) -> bool {
    use std::io::Read;
    if !std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file()) {
        return false;
    }
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut hasher = umx_core::models::FileHasher::default();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        match file.read(&mut buf) {
            Ok(0) => return hasher.hex() == sha256,
            Ok(n) => hasher.update(&buf[..n]),
            Err(_) => return false,
        }
    }
}

fn no_space(name: &str, needed: u64, folder: &str, available: u64) -> Envelope {
    Envelope::fail(
        CMD,
        Issue::fill(
            Code::ModelDiskSpace,
            &[
                ("model", Fill::text(name)),
                ("needed", Fill::count(needed)),
                ("folder", Fill::path(Path::new(folder))),
                ("available", Fill::count(available)),
            ],
        ),
    )
}

/// A fetch error as its catalogue issue: a hash mismatch names the file and both hashes; anything else names the
/// file, the step and the reason (hosts and sizes, never a URL).
fn failed(name: &str, e: &FetchError, source: Option<&ModelSource>) -> Envelope {
    if let FetchError::Hash {
        file,
        expected,
        got,
    } = e
    {
        return Envelope::fail(
            CMD,
            Issue::fill(
                Code::ModelHashMismatch,
                &[
                    ("file", Fill::text(file)),
                    ("got", Fill::text(got)),
                    ("expected", Fill::text(expected)),
                ],
            ),
        );
    }
    let file = match e {
        FetchError::Path { file, .. }
        | FetchError::Size { file, .. }
        | FetchError::Write { file, .. }
        | FetchError::Disk { file } => file.clone(),
        _ => source
            .and_then(|s| s.files.first())
            .map_or_else(|| name.to_string(), |f| f.path.clone()),
    };
    Envelope::fail(
        CMD,
        Issue::fill(
            Code::ModelFetchFailed,
            &[
                ("file", Fill::text(&file)),
                ("step", Fill::word(e.step())),
                ("reason", Fill::text(&e.to_string())),
            ],
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use umx_core::ports::{FileFetched, ModelFileSpec};
    use umx_fakes::{FakeModelFetcher, Served};

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("umx-models-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn plenty(_: &Path) -> std::io::Result<u64> {
        Ok(u64::MAX)
    }

    fn code(env: &Envelope) -> &str {
        env.errors[0].code()
    }

    #[test]
    fn an_unknown_model_names_the_known_ones() {
        let env = fetch_with(
            "nope",
            false,
            &temp("unknown"),
            &FakeModelFetcher::new(),
            &plenty,
            &mut |_| {},
        );
        assert_eq!(code(&env), "model_unknown");
        assert!(env.errors[0].message().contains("embeddinggemma-2"));
    }

    #[test]
    fn a_dry_run_shows_the_files_and_fetches_nothing() {
        let fetcher = FakeModelFetcher::new();
        let models = temp("dry");
        let env = fetch_with(
            "embeddinggemma-2",
            true,
            &models,
            &fetcher,
            &plenty,
            &mut |_| {},
        );
        assert!(env.errors.is_empty(), "{}", env.to_json());
        assert_eq!(env.data["files"][0]["size"], 557_950_176u64);
        assert_eq!(env.data["needed"], 557_950_176u64);
        assert!(env.data["available"].is_u64(), "{}", env.to_json());
        assert_eq!(fetcher.downloads(), 0);
        assert_eq!(std::fs::read_dir(&models).unwrap().count(), 0);
    }

    /// Architect: not enough free disk is refused, naming what is needed, before any byte is fetched.
    #[test]
    fn too_little_disk_is_refused_before_fetching() {
        let fetcher = FakeModelFetcher::new();
        let tight = |_: &Path| Ok(1_000);
        let env = fetch_with(
            "embeddinggemma-2",
            false,
            &temp("disk"),
            &fetcher,
            &tight,
            &mut |_| {},
        );
        assert_eq!(code(&env), "model_disk_space");
        assert!(
            env.errors[0].message().contains("557950176"),
            "{}",
            env.to_json()
        );
        assert_eq!(fetcher.downloads(), 0);
    }

    /// Reviewer R112-2's witness: a same-size file that does not verify still needs the full size, so a full disk
    /// is refused before the fetcher is called.
    #[test]
    fn a_same_size_bad_cache_file_does_not_skip_disk_preflight() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Called(AtomicUsize);
        impl ModelFetcher for Called {
            fn adapter_id(&self) -> &'static str {
                "test.model-fetcher"
            }
            fn fetch(
                &self,
                source: &ModelSource,
                _: &str,
                _: &mut dyn FnMut(FetchProgress),
            ) -> Result<Vec<FileFetched>, FetchError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Err(FetchError::Write {
                    file: source.files[0].path.clone(),
                    reason: "fetching began without space".into(),
                })
            }
        }
        let models = temp("bad-cache");
        let source = umx_core::models::pinned("embeddinggemma-2").unwrap();
        let folder = source.folder(models.to_str().unwrap());
        std::fs::create_dir_all(&folder).unwrap();
        let file = std::fs::File::create(Path::new(&folder).join(&source.files[0].path)).unwrap();
        file.set_len(source.files[0].size).unwrap();
        drop(file);
        let fetcher = Called(AtomicUsize::new(0));
        let env = fetch_with(
            "embeddinggemma-2",
            false,
            &models,
            &fetcher,
            &|_| Ok(0),
            &mut |_| {},
        );
        std::fs::remove_dir_all(&models).unwrap();
        assert_eq!(code(&env), "model_disk_space", "{}", env.to_json());
        assert_eq!(fetcher.0.load(Ordering::SeqCst), 0);
    }

    /// The fetcher's own check under the models lock (space taken by another writer meanwhile) is the same code.
    #[test]
    fn the_fetchers_space_refusal_is_model_disk_space() {
        struct Full;
        impl ModelFetcher for Full {
            fn fetch(
                &self,
                _: &ModelSource,
                _: &str,
                _: &mut dyn FnMut(FetchProgress),
            ) -> Result<Vec<FileFetched>, FetchError> {
                Err(FetchError::Space {
                    needed: 7,
                    available: 3,
                })
            }
            fn adapter_id(&self) -> &'static str {
                "test.full"
            }
        }
        let env = fetch_with(
            "embeddinggemma-2",
            false,
            &temp("full"),
            &Full,
            &plenty,
            &mut |_| {},
        );
        assert_eq!(code(&env), "model_disk_space", "{}", env.to_json());
    }

    #[test]
    fn a_hash_mismatch_names_the_file_and_both_hashes() {
        let fetcher = FakeModelFetcher::new();
        let source = umx_core::models::pinned("embeddinggemma-2").unwrap();
        let wrong = vec![0u8; source.files[0].size as usize];
        fetcher.serve(&source.repo, Served::Bytes(wrong));
        let env = fetch_with(
            "embeddinggemma-2",
            false,
            &temp("hash"),
            &fetcher,
            &plenty,
            &mut |_| {},
        );
        assert_eq!(code(&env), "model_hash_mismatch");
        let message = env.errors[0].message().to_string();
        assert!(
            message.contains("embeddinggemma-2-BF16.gguf")
                && message.contains(&source.files[0].sha256)
        );
    }

    #[test]
    fn a_failed_step_is_named_and_never_a_url() {
        let fetcher = FakeModelFetcher::new();
        let source = umx_core::models::pinned("embeddinggemma-2").unwrap();
        fetcher.serve(&source.repo, Served::RedirectOff("evil.example".into()));
        let env = fetch_with(
            "embeddinggemma-2",
            false,
            &temp("redirect"),
            &fetcher,
            &plenty,
            &mut |_| {},
        );
        assert_eq!(code(&env), "model_fetch_failed");
        let message = env.errors[0].message();
        assert!(
            message.contains("redirect") && message.contains("evil.example"),
            "{message}"
        );
        assert!(!message.contains("://"));
    }

    /// A fetched model is reported file by file; a second fetch downloads nothing.
    #[test]
    fn a_fetch_reports_each_file_and_a_second_downloads_nothing() {
        struct One(FakeModelFetcher);
        impl ModelFetcher for One {
            fn fetch(
                &self,
                source: &ModelSource,
                models: &str,
                progress: &mut dyn FnMut(FetchProgress),
            ) -> Result<Vec<FileFetched>, FetchError> {
                // The real model is 558 MB: fetch a small stand-in with the same shape.
                let bytes = b"stand-in".to_vec();
                let small = ModelSource {
                    files: vec![ModelFileSpec {
                        path: source.files[0].path.clone(),
                        sha256: umx_core::models::sha256_hex(&bytes),
                        size: bytes.len() as u64,
                    }],
                    ..source.clone()
                };
                self.0.serve(&source.repo, Served::Bytes(bytes));
                self.0.fetch(&small, models, progress)
            }
            fn adapter_id(&self) -> &'static str {
                "fake.model-fetcher"
            }
        }
        let fetcher = One(FakeModelFetcher::new());
        let models = temp("fetch");
        let env = fetch_with(
            "embeddinggemma-2",
            false,
            &models,
            &fetcher,
            &plenty,
            &mut |_| {},
        );
        assert!(env.errors.is_empty(), "{}", env.to_json());
        assert_eq!(env.data["files"][0]["downloaded"], true);
        let again = fetch_with(
            "embeddinggemma-2",
            false,
            &models,
            &fetcher,
            &plenty,
            &mut |_| {},
        );
        assert_eq!(again.data["files"][0]["downloaded"], false);
    }
}

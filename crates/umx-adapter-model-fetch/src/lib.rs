//! hf.model-fetcher (req-74; blueprint c-adapter-model-fetch): fetches a pinned model revision from Hugging Face
//! over https with ureq (its second owner, after the hosted embedder). The local embedder crate has no network code
//! at all; this crate is the only way a model reaches the models folder, and only through `umx models fetch`.
//!
//! Security (security and architect rulings, s14):
//! - https only, to a fixed allow-list of hosts. Hugging Face answers a resolve URL with a redirect to its CDN, so
//!   ureq follows none (max redirects 0) and this adapter follows each one itself, only to an allowed https host,
//!   at most [`MAX_HOPS`] times.
//! - No credential is ever sent: no Authorization header, and no Hugging Face token, `HF_*` variable or
//!   `~/.cache/huggingface` is read. The platform TLS verifier. A proxy only from the user's own environment.
//! - Raw bytes only: `Accept-Encoding: identity`, a Content-Length equal to the pinned size (missing is refused),
//!   and the body read through `take(size + 1)`, so a longer body is refused unwritten.
//! - Each file is written to `.<name>.part` (created new, 0600; a stale one removed first, a symlink refused),
//!   hashed as it streams, and renamed to its final name only when size and sha256 match; the folder is 0700. A
//!   file already there counts only if it is a regular file, owned by the user, with the pinned sha256, and the
//!   file read is the one checked (device and inode compared), so a planted symlink never passes.
//! - Errors and progress name hosts, files, sizes and hashes, never a URL or query string (CDN URLs are signed).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;
use umx_core::models::FileHasher;
use umx_core::ports::model_fetcher::safe_file_name;
use umx_core::ports::{FetchError, FetchProgress, FileFetched, ModelFetcher, ModelSource};

/// The hosts a fetch may reach: Hugging Face and the CDN hosts its resolve URLs redirect to (measured 2026-11-10:
/// huggingface.co answers 302 to us.aws.cdn.hf.co), by exact name.
pub const ALLOWED_HOSTS: &[&str] = &[
    "huggingface.co",
    "us.aws.cdn.hf.co",
    "eu.aws.cdn.hf.co",
    "cas-bridge.xethub.hf.co",
    "cdn-lfs.huggingface.co",
    "cdn-lfs-us-1.huggingface.co",
];

/// Redirects followed per file at most.
pub const MAX_HOPS: usize = 5;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(60);
/// The whole body: about 560 MB at 0.5 MB/s still fits.
const BODY_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Progress is reported at most this often (bytes).
const PROGRESS_STEP: u64 = 16 * 1024 * 1024;
const CHUNK: usize = 256 * 1024;

/// Takes the models folder's one writer lock, held until the file is dropped (compose passes
/// `umx_adapter_fs::models_folder_lock`).
pub type FolderLock = fn(&Path) -> io::Result<File>;
/// Bytes free on the file system holding a path (compose passes `umx_adapter_fs::available_bytes`).
pub type FreeSpace = fn(&Path) -> io::Result<u64>;

pub struct HfModelFetcher {
    agent: ureq::Agent,
    /// None (tests): no lock and no space check.
    models_lock: Option<FolderLock>,
    available: Option<FreeSpace>,
    /// `https://huggingface.co`, or a stub server in this crate's tests.
    base: String,
    allowed: Vec<String>,
    https_only: bool,
}

impl HfModelFetcher {
    /// https to huggingface.co and [`ALLOWED_HOSTS`] only. Every write happens under `models_lock` when given,
    /// after the space check when `available` is given.
    pub fn new(models_lock: Option<FolderLock>, available: Option<FreeSpace>) -> HfModelFetcher {
        HfModelFetcher {
            models_lock,
            available,
            ..HfModelFetcher::build(
                "https://huggingface.co".to_string(),
                ALLOWED_HOSTS.iter().map(|h| h.to_string()).collect(),
                true,
            )
        }
    }

    fn build(base: String, allowed: Vec<String>, https_only: bool) -> HfModelFetcher {
        let agent = ureq::Agent::config_builder()
            .max_redirects(0)
            .http_status_as_error(false)
            .accept_encoding(ureq::config::AutoHeaderValue::None)
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_recv_response(Some(RESPONSE_TIMEOUT))
            .timeout_recv_body(Some(BODY_TIMEOUT))
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                    .build(),
            )
            .build()
            .new_agent();
        HfModelFetcher {
            agent,
            models_lock: None,
            available: None,
            base,
            allowed,
            https_only,
        }
    }

    /// Whether `url` may be fetched: https (unless a test allows plain http) to an allowed host. Returns the host.
    fn allowed_host(&self, url: &str) -> Result<String, FetchError> {
        let (scheme, rest) = url.split_once("://").ok_or_else(|| FetchError::Redirect {
            host: "(no host)".to_string(),
        })?;
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        // No user info in a URL we follow, ever.
        let host_port = authority.rsplit('@').next().unwrap_or_default();
        let (host, port) = match host_port.rsplit_once(':') {
            Some((h, port)) if port.chars().all(|c| c.is_ascii_digit()) => (h, Some(port)),
            _ => (host_port, None),
        };
        let host = host.to_ascii_lowercase();
        // https's own port or none: an allowed host on another port is another service (security F2).
        let port_ok = !self.https_only || matches!(port, None | Some("443"));
        let scheme_ok =
            scheme.eq_ignore_ascii_case("https") || (!self.https_only && scheme == "http");
        if !scheme_ok || !port_ok || authority.contains('@') || !self.allowed.contains(&host) {
            return Err(FetchError::Redirect { host });
        }
        Ok(host)
    }

    /// The response for the file's resolve URL, after following allowed redirects only.
    fn get(
        &self,
        source: &ModelSource,
        file: &str,
    ) -> Result<(String, ureq::http::Response<ureq::Body>), FetchError> {
        let mut url = format!(
            "{}/{}/resolve/{}/{}",
            self.base, source.repo, source.commit, file
        );
        for _ in 0..=MAX_HOPS {
            let host = self.allowed_host(&url)?;
            let response = self
                .agent
                .get(&url)
                .header("Accept-Encoding", "identity")
                .call()
                .map_err(|e| FetchError::Connect {
                    host: host.clone(),
                    reason: safe_reason(&e),
                })?;
            let status = response.status().as_u16();
            if matches!(status, 301 | 302 | 303 | 307 | 308) {
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or(FetchError::Status {
                        host: host.clone(),
                        status,
                    })?;
                url = absolute(&url, location);
                continue;
            }
            if status != 200 {
                return Err(FetchError::Status { host, status });
            }
            return Ok((host, response));
        }
        Err(FetchError::Status {
            host: "(too many redirects)".to_string(),
            status: 310,
        })
    }

    fn fetch_file(
        &self,
        source: &ModelSource,
        folder: &Path,
        index: usize,
        uid: &mut Option<u32>,
        progress: &mut dyn FnMut(FetchProgress),
    ) -> Result<FileFetched, FetchError> {
        let spec = &source.files[index];
        let name = spec.path.as_str();
        let target = folder.join(name);
        let (host, response) = self.get(source, name)?;
        let length = response
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        if length != Some(spec.size) {
            return Err(FetchError::Size {
                file: name.to_string(),
                expected: spec.size,
                got: length,
            });
        }
        let temp = folder.join(format!(".{name}.part"));
        let mut out = create_temp(&temp, name)?;
        // The folder must be the user's: compare its owner with the temp file just made (no getuid here).
        let mine = out.metadata().map_err(|e| write_error(name, &e))?.uid();
        if uid.is_none() {
            check_owned(folder, mine, name).inspect_err(|_| {
                let _ = fs::remove_file(&temp);
            })?;
            *uid = Some(mine);
        }
        let result = (|| {
            let mut body = response.into_body().into_reader().take(spec.size + 1);
            let mut hasher = FileHasher::default();
            let mut buf = vec![0u8; CHUNK];
            let (mut done, mut told) = (0u64, 0u64);
            loop {
                let n = body.read(&mut buf).map_err(|e| FetchError::Connect {
                    host: host.clone(),
                    reason: e.kind().to_string(),
                })?;
                if n == 0 {
                    break;
                }
                done += n as u64;
                if done > spec.size {
                    return Err(FetchError::Size {
                        file: name.to_string(),
                        expected: spec.size,
                        got: Some(done),
                    });
                }
                hasher.update(&buf[..n]);
                out.write_all(&buf[..n])
                    .map_err(|e| write_error(name, &e))?;
                if done - told >= PROGRESS_STEP || done == spec.size {
                    progress(FetchProgress {
                        file: name.to_string(),
                        done,
                        total: spec.size,
                    });
                    told = done;
                }
            }
            if done != spec.size {
                return Err(FetchError::Size {
                    file: name.to_string(),
                    expected: spec.size,
                    got: Some(done),
                });
            }
            let got = hasher.hex();
            if got != spec.sha256 {
                return Err(FetchError::Hash {
                    file: name.to_string(),
                    expected: spec.sha256.clone(),
                    got,
                });
            }
            out.sync_all().map_err(|e| write_error(name, &e))?;
            // The temp name must still be this file: a writer that ignores the lock may have replaced it.
            let (ours, now) = (
                out.metadata().map_err(|e| write_error(name, &e))?,
                fs::symlink_metadata(&temp).map_err(|e| write_error(name, &e))?,
            );
            if (ours.dev(), ours.ino()) != (now.dev(), now.ino()) {
                return Err(FetchError::Path {
                    file: name.to_string(),
                    reason: "its temp file was replaced while it was written".to_string(),
                });
            }
            fs::rename(&temp, &target).map_err(|e| write_error(name, &e))?;
            if let Ok(dir) = File::open(folder) {
                let _ = dir.sync_all();
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result?;
        Ok(FileFetched {
            file: name.to_string(),
            size: spec.size,
            downloaded: true,
        })
    }
}

/// `location` made absolute against `current` (a path-only Location keeps the scheme and host).
fn absolute(current: &str, location: &str) -> String {
    if location.contains("://") {
        return location.to_string();
    }
    let origin_end = current
        .find("://")
        .and_then(|i| current[i + 3..].find('/').map(|j| i + 3 + j))
        .unwrap_or(current.len());
    if location.starts_with('/') {
        format!("{}{location}", &current[..origin_end])
    } else {
        // A relative reference is never what Hugging Face sends; resolve it against the origin all the same.
        format!("{}/{location}", &current[..origin_end])
    }
}

/// ureq's error, without any URL in it: its kind only.
fn safe_reason(e: &ureq::Error) -> String {
    match e {
        ureq::Error::Timeout(_) => "timed out".to_string(),
        ureq::Error::Io(io) => io.kind().to_string(),
        ureq::Error::HostNotFound => "host not found".to_string(),
        ureq::Error::ConnectionFailed => "connection failed".to_string(),
        ureq::Error::Tls(_) | ureq::Error::Rustls(_) => "TLS failed".to_string(),
        _ => "the request failed".to_string(),
    }
}

fn write_error(file: &str, e: &io::Error) -> FetchError {
    if e.raw_os_error() == Some(28) {
        FetchError::Disk {
            file: file.to_string(),
        }
    } else {
        FetchError::Write {
            file: file.to_string(),
            reason: e.kind().to_string(),
        }
    }
}

/// The temp file, created new (0600). A stale one is removed first; a symlink in its place is refused.
fn create_temp(temp: &Path, name: &str) -> Result<File, FetchError> {
    if let Ok(m) = fs::symlink_metadata(temp) {
        if m.file_type().is_symlink() || !m.file_type().is_file() {
            return Err(FetchError::Path {
                file: name.to_string(),
                reason: "its temp name is taken by something that is not a file".to_string(),
            });
        }
        fs::remove_file(temp).map_err(|e| write_error(name, &e))?;
    }
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(temp)
        .map_err(|e| write_error(name, &e))
}

/// Whether `target` is already the pinned file: a regular file (not a symlink) whose bytes hash to `sha256`, and the
/// file opened is the one checked (device and inode).
fn present(target: &Path, sha256: &str) -> bool {
    let Ok(before) = fs::symlink_metadata(target) else {
        return false;
    };
    if !before.file_type().is_file() {
        return false;
    }
    let Ok(mut file) = File::open(target) else {
        return false;
    };
    let Ok(opened) = file.metadata() else {
        return false;
    };
    if (opened.dev(), opened.ino()) != (before.dev(), before.ino()) {
        return false;
    }
    let mut hasher = FileHasher::default();
    let mut buf = vec![0u8; CHUNK];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buf[..n]),
            Err(_) => return false,
        }
    }
    hasher.hex() == sha256
}

/// The folders from `models` down to the revision's: each is a real folder (not a symlink) owned by `uid`.
fn check_owned(folder: &Path, uid: u32, name: &str) -> Result<(), FetchError> {
    for dir in [folder, folder.parent().unwrap_or(folder)] {
        let m = fs::symlink_metadata(dir).map_err(|e| write_error(name, &e))?;
        if m.file_type().is_symlink() || !m.is_dir() || m.uid() != uid {
            return Err(FetchError::Path {
                file: name.to_string(),
                reason: format!(
                    "{} is a symlink or not your folder",
                    dir.file_name()
                        .map_or("the models folder".into(), |n| n.to_string_lossy())
                ),
            });
        }
    }
    Ok(())
}

/// `path` and any missing parents made, each new one 0700; the revision's folder set to 0700 whatever it was.
fn make_folder(path: &Path, name: &str) -> Result<(), FetchError> {
    if let Ok(m) = fs::symlink_metadata(path)
        && m.file_type().is_symlink()
    {
        return Err(FetchError::Path {
            file: name.to_string(),
            reason: "the model's folder is a symlink".to_string(),
        });
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(|e| write_error(name, &e))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|e| write_error(name, &e))
}

impl ModelFetcher for HfModelFetcher {
    fn fetch(
        &self,
        source: &ModelSource,
        models: &str,
        progress: &mut dyn FnMut(FetchProgress),
    ) -> Result<Vec<FileFetched>, FetchError> {
        for f in &source.files {
            if !safe_file_name(&f.path) {
                return Err(FetchError::Path {
                    file: f.path.clone(),
                    reason: "not one plain file name".to_string(),
                });
            }
        }
        let folder = PathBuf::from(source.folder(models));
        let first = source.files.first().map_or("", |f| f.path.as_str());
        make_folder(&folder, first)?;
        // Lock, check space, then write, verify and rename each file; the lock drops at the end.
        let models = Path::new(models);
        let _lock = self
            .models_lock
            .map(|lock| lock(models))
            .transpose()
            .map_err(|e| FetchError::Write {
                file: first.to_string(),
                reason: format!("taking the models lock: {}", e.kind()),
            })?;
        // Present means verified (sha256), never just the right size: anything else is fetched in full, and its
        // temp file needs the full size (reviewer R112-2).
        let there: Vec<bool> = source
            .files
            .iter()
            .map(|f| present(&folder.join(&f.path), &f.sha256))
            .collect();
        if let Some(available) = self.available {
            let needed: u64 = source
                .files
                .iter()
                .zip(&there)
                .filter(|(_, there)| !**there)
                .map(|(f, _)| f.size)
                .sum();
            if needed > 0
                && let Ok(free) = available(&folder)
                && free < needed
            {
                return Err(FetchError::Space {
                    needed,
                    available: free,
                });
            }
        }
        let mut uid = None;
        let mut out = Vec::new();
        for (i, f) in source.files.iter().enumerate() {
            out.push(if there[i] {
                FileFetched {
                    file: f.path.clone(),
                    size: f.size,
                    downloaded: false,
                }
            } else {
                self.fetch_file(source, &folder, i, &mut uid, progress)?
            });
        }
        Ok(out)
    }

    fn adapter_id(&self) -> &'static str {
        "hf.model-fetcher"
    }
}

#[cfg(test)]
mod tests;

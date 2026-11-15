//! The adapter against a stub HTTP server on 127.0.0.1 (never the network): the contract suite, and what only the
//! real adapter does (headers sent, allow-list and scheme, a missing size, URLs never in errors).

use super::*;
use std::io::{BufRead, BufReader};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use umx_contracts::model_fetcher::{Case, FetchWorld, good_bytes, source};

/// A stub of Hugging Face and its CDN on 127.0.0.1. Records every request's header lines.
struct Stub {
    port: u16,
    requests: Arc<Mutex<Vec<Vec<String>>>>,
}

impl Stub {
    fn start() -> Stub {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&requests);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let seen = Arc::clone(&seen);
                std::thread::spawn(move || answer(stream, port, &seen));
            }
        });
        Stub { port, requests }
    }
}

fn answer(mut stream: TcpStream, port: u16, seen: &Mutex<Vec<Vec<String>>>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut lines = Vec::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let line = line.trim_end().to_string();
        if line.is_empty() {
            break;
        }
        lines.push(line);
    }
    let path = lines[0].split(' ').nth(1).unwrap_or_default().to_string();
    seen.lock().unwrap().push(lines);
    let send = |stream: &mut TcpStream, head: String, body: &[u8]| {
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(body);
    };
    let ok = |stream: &mut TcpStream, body: &[u8]| {
        send(
            stream,
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            ),
            body,
        )
    };
    let redirect = |stream: &mut TcpStream, to: String| {
        send(
            stream,
            format!(
                "HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            ),
            b"",
        )
    };
    let repo = |case: Case| format!("/{}/resolve/", case.repo());
    if path.starts_with(&repo(Case::Good)) {
        // As Hugging Face does: a redirect to the CDN, with a signed query.
        redirect(
            &mut stream,
            format!("http://127.0.0.1:{port}/cdn/good?X-Signature=secret"),
        );
    } else if path.starts_with("/cdn/good") {
        ok(&mut stream, &Case::Good.bytes());
    } else if path.starts_with(&repo(Case::Mismatch)) {
        ok(&mut stream, &Case::Mismatch.bytes());
    } else if path.starts_with(&repo(Case::Oversize)) {
        ok(&mut stream, &Case::Oversize.bytes());
    } else if path.starts_with(&repo(Case::OffList)) {
        redirect(
            &mut stream,
            format!("http://localhost:{port}/cdn/good?X-Signature=secret"),
        );
    } else if path.starts_with("/fixture/nosize/") {
        let body = good_bytes();
        send(
            &mut stream,
            "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_string(),
            &body,
        );
    } else if path.starts_with("/fixture/slow/") {
        // The size first, the bytes a moment later: two fetches without the lock would both be mid-write.
        let body = good_bytes();
        send(
            &mut stream,
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            ),
            b"",
        );
        std::thread::sleep(std::time::Duration::from_millis(300));
        let _ = stream.write_all(&body);
    } else if path.starts_with("/fixture/chunked/") {
        // Declares the pinned size, then sends a longer chunked body: the stream itself must be cut off.
        let mut body = good_bytes();
        body.extend_from_slice(b"more");
        let mut chunked = format!("{:x}\r\n", body.len()).into_bytes();
        chunked.extend_from_slice(&body);
        chunked.extend_from_slice(b"\r\n0\r\n\r\n");
        send(
            &mut stream,
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                good_bytes().len()
            ),
            &chunked,
        );
    } else if path.starts_with("/fixture/loop/") {
        redirect(&mut stream, format!("http://127.0.0.1:{port}{path}"));
    } else {
        send(
            &mut stream,
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
            b"",
        );
    }
}

struct World {
    port: HfModelFetcher,
    models: String,
    stub: Stub,
}

impl World {
    fn new() -> World {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let stub = Stub::start();
        let dir = std::env::temp_dir().join(format!(
            "umx-hf-fetch-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // Plain http to 127.0.0.1 only here: the stub has no TLS. `new()` is https to the fixed list.
        let port = HfModelFetcher::build(
            format!("http://127.0.0.1:{}", stub.port),
            vec!["127.0.0.1".to_string()],
            false,
        );
        World {
            port,
            models: dir.display().to_string(),
            stub,
        }
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.models);
    }
}

impl FetchWorld for World {
    type Port = HfModelFetcher;
    fn port(&self) -> &HfModelFetcher {
        &self.port
    }
    fn models(&self) -> &str {
        &self.models
    }
}

umx_contracts::model_fetcher_contract!(
    World::new;
    mf_c1_fetch_keeps_each_verified_file_0600_in_a_0700_folder(),
    mf_c2_a_present_verified_file_is_not_downloaded_again(),
    mf_c3_a_hash_mismatch_leaves_nothing(),
    mf_c4_an_oversize_body_is_refused_unwritten(),
    mf_c5_a_redirect_off_the_allow_list_is_refused(),
    mf_c6_a_damaged_file_is_replaced(),
    mf_c7_an_unsafe_file_name_is_refused_before_any_path(),
    mf_c8_a_symlink_at_the_final_name_is_never_trusted(),
);

/// Security: no request ever carries a credential, and every one asks for raw bytes.
#[test]
fn no_request_carries_a_credential_and_all_ask_for_identity() {
    let w = World::new();
    w.port
        .fetch(&source(Case::Good), &w.models, &mut |_| {})
        .unwrap();
    let _ = w.port.fetch(&source(Case::OffList), &w.models, &mut |_| {});
    let requests = w.stub.requests.lock().unwrap().clone();
    assert!(requests.len() >= 3, "{requests:?}");
    for r in &requests {
        let lower: Vec<String> = r.iter().map(|l| l.to_ascii_lowercase()).collect();
        assert!(
            !lower.iter().any(|l| l.starts_with("authorization:")),
            "{r:?}"
        );
        assert!(!lower.iter().any(|l| l.starts_with("cookie:")), "{r:?}");
        assert!(
            lower.iter().any(|l| l == "accept-encoding: identity"),
            "{r:?}"
        );
    }
}

/// A missing Content-Length is refused, not trusted; nothing is written.
#[test]
fn a_missing_size_is_refused() {
    let w = World::new();
    let mut s = source(Case::Good);
    s.repo = "fixture/nosize".into();
    match w.port.fetch(&s, &w.models, &mut |_| {}) {
        Err(FetchError::Size { got: None, .. }) => {}
        other => panic!("{other:?}"),
    }
    let folder = s.folder(&w.models);
    assert_eq!(fs::read_dir(folder).unwrap().count(), 0);
}

/// Security: a symlink at the temp name is refused, never written through, and left for the user to look at.
#[test]
fn a_symlinked_temp_name_is_refused() {
    let w = World::new();
    let s = source(Case::Good);
    let folder = std::path::PathBuf::from(s.folder(&w.models));
    fs::create_dir_all(&folder).unwrap();
    let victim = std::path::Path::new(&w.models).join("victim");
    fs::write(&victim, b"mine").unwrap();
    let name = &s.files[0].path;
    std::os::unix::fs::symlink(&victim, folder.join(format!(".{name}.part"))).unwrap();
    assert!(matches!(
        w.port.fetch(&s, &w.models, &mut |_| {}),
        Err(FetchError::Path { .. })
    ));
    assert_eq!(fs::read(&victim).unwrap(), b"mine");
    assert!(!folder.join(name).exists());
}

/// A body longer than its declared size is refused as it streams, and nothing is kept.
#[test]
fn a_body_longer_than_declared_is_refused() {
    let w = World::new();
    let mut s = source(Case::Good);
    s.repo = "fixture/chunked".into();
    let got = w.port.fetch(&s, &w.models, &mut |_| {});
    assert!(matches!(got, Err(FetchError::Size { .. })), "{got:?}");
    let folder = s.folder(&w.models);
    assert_eq!(fs::read_dir(folder).unwrap().count(), 0);
}

/// The models lock for these tests: what umx_adapter_fs::models_folder_lock does, minus the no-follow open.
fn test_lock(folder: &Path) -> io::Result<File> {
    let f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(folder.join(".models.lock"))?;
    f.lock()?;
    Ok(f)
}

/// Two fetches of one model at once, under the models lock: one downloads, the other waits and finds it there.
#[test]
fn concurrent_fetches_take_turns_under_the_models_lock() {
    let mut w = World::new();
    w.port.models_lock = Some(test_lock);
    let mut s = source(Case::Good);
    s.repo = "fixture/slow".into();
    let results: Vec<_> = std::thread::scope(|scope| {
        let runs: Vec<_> = (0..2)
            .map(|_| scope.spawn(|| w.port.fetch(&s, &w.models, &mut |_| {}).unwrap()))
            .collect();
        runs.into_iter().map(|r| r.join().unwrap()).collect()
    });
    let downloaded: Vec<bool> = results.iter().map(|r| r[0].downloaded).collect();
    assert_eq!(
        downloaded.iter().filter(|d| **d).count(),
        1,
        "{downloaded:?}"
    );
    let slow = w
        .stub
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r[0].contains("/fixture/slow/"))
        .count();
    assert_eq!(slow, 1, "the second fetch must not download");
    let folder = std::path::PathBuf::from(s.folder(&w.models));
    let names: Vec<String> = fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, [s.files[0].path.clone()], "no .part left");
    assert_eq!(
        fs::read(folder.join(&s.files[0].path)).unwrap(),
        good_bytes()
    );
}

/// Reviewer R112-1's witness, under the models lock: A streams the good bytes while B (which would get the
/// mismatching ones) starts. B must wait for the lock instead of replacing A's temp file, so A publishes its own
/// verified bytes and B then finds them there.
#[test]
fn overlapping_fetches_never_publish_the_other_unverified_temp() {
    use std::sync::mpsc;
    let w = World::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for bytes in [Case::Good.bytes(), Case::Mismatch.bytes()] {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
            }
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            );
            let _ = stream.write_all(&bytes);
        }
    });
    let mut port =
        HfModelFetcher::build(format!("http://{address}"), vec!["127.0.0.1".into()], false);
    port.models_lock = Some(test_lock);
    let spec = source(Case::Good);
    let target = PathBuf::from(spec.folder(&w.models)).join(&spec.files[0].path);
    let (a_ready, a_progress) = mpsc::channel();
    let (a_release, a_resume) = mpsc::channel::<()>();
    let (b_ready, b_progress) = mpsc::channel();
    let (a, b, published, overlapped) = std::thread::scope(|scope| {
        let (port, spec, models) = (&port, &spec, w.models.as_str());
        let a = scope.spawn(move || {
            port.fetch(spec, models, &mut |_| {
                let _ = a_ready.send(());
                let _ = a_resume.recv();
            })
        });
        a_progress.recv_timeout(Duration::from_secs(10)).unwrap();
        let b = scope.spawn(move || {
            port.fetch(spec, models, &mut |_| {
                let _ = b_ready.send(());
            })
        });
        let overlapped = b_progress.recv_timeout(Duration::from_millis(500)).is_ok();
        drop(a_release);
        let a = a.join().unwrap();
        let published = fs::read(&target);
        let b = b.join().unwrap();
        (a, b, published, overlapped)
    });
    assert!(!overlapped, "the second writer must wait for the lock");
    assert!(a.as_ref().is_ok_and(|f| f[0].downloaded), "{a:?}");
    assert!(b.as_ref().is_ok_and(|f| !f[0].downloaded), "{b:?}");
    assert_eq!(
        umx_core::models::sha256_hex(&published.unwrap()),
        spec.files[0].sha256,
        "a successful fetch published bytes it did not verify"
    );
}

/// The space check runs under the lock, before any request; a file already there needs no space.
#[test]
fn too_little_space_is_refused_before_any_request() {
    let mut w = World::new();
    w.port.available = Some(|_| Ok(10));
    let s = source(Case::Good);
    match w.port.fetch(&s, &w.models, &mut |_| {}) {
        Err(FetchError::Space {
            needed,
            available: 10,
        }) => assert_eq!(needed, s.files[0].size),
        other => panic!("{other:?}"),
    }
    assert!(w.stub.requests.lock().unwrap().is_empty());
    // A same-size file that does not verify is no reason to skip the check (reviewer R112-2).
    let folder = std::path::PathBuf::from(s.folder(&w.models));
    fs::create_dir_all(&folder).unwrap();
    fs::write(
        folder.join(&s.files[0].path),
        vec![0u8; s.files[0].size as usize],
    )
    .unwrap();
    assert!(matches!(
        w.port.fetch(&s, &w.models, &mut |_| {}),
        Err(FetchError::Space { .. })
    ));
    assert!(w.stub.requests.lock().unwrap().is_empty());
    w.port.available = None;
    w.port.fetch(&s, &w.models, &mut |_| {}).unwrap();
    w.port.available = Some(|_| Ok(0));
    assert!(!w.port.fetch(&s, &w.models, &mut |_| {}).unwrap()[0].downloaded);
}

/// Redirects are followed only so far.
#[test]
fn a_redirect_loop_stops() {
    let w = World::new();
    let mut s = source(Case::Good);
    s.repo = "fixture/loop".into();
    assert!(matches!(
        w.port.fetch(&s, &w.models, &mut |_| {}),
        Err(FetchError::Status { .. })
    ));
}

/// The real adapter: https only, to the fixed list by exact host on port 443; user info refused; errors name the
/// host only.
#[test]
fn only_https_to_the_fixed_hosts() {
    let real = HfModelFetcher::new(None, None);
    for ok in [
        "https://huggingface.co/a/b/resolve/c/f",
        "https://us.aws.cdn.hf.co/xet-bridge-us/abc?X-Amz-Signature=s",
        "HTTPS://HuggingFace.co:443/x",
    ] {
        assert!(real.allowed_host(ok).is_ok(), "{ok}");
    }
    for (bad, host) in [
        ("http://huggingface.co/x", "huggingface.co"),
        (
            "https://huggingface.co.evil.example/x",
            "huggingface.co.evil.example",
        ),
        ("https://evil.example/huggingface.co", "evil.example"),
        ("https://user:pw@huggingface.co/x", "huggingface.co"),
        ("ftp://huggingface.co/x", "huggingface.co"),
        ("https://huggingface.co:4443/x", "huggingface.co"),
        ("https://us.aws.cdn.hf.co:80/x", "us.aws.cdn.hf.co"),
        ("https://huggingface.co:/x", "huggingface.co"),
        ("https://huggingface.co:0443/x", "huggingface.co"),
    ] {
        match real.allowed_host(bad) {
            Err(FetchError::Redirect { host: h }) => assert_eq!(h, host, "{bad}"),
            other => panic!("{bad}: {other:?}"),
        }
    }
    assert_eq!(real.adapter_id(), "hf.model-fetcher");
}

/// No error text holds a URL or a query string: the off-list refusal names the host only.
#[test]
fn errors_never_print_a_url() {
    let w = World::new();
    let e = w
        .port
        .fetch(&source(Case::OffList), &w.models, &mut |_| {})
        .unwrap_err();
    let text = e.to_string();
    assert!(
        !text.contains("://") && !text.contains('?') && !text.contains("Signature"),
        "{text}"
    );
    assert!(text.contains("localhost"), "{text}");
}

#[test]
fn a_location_is_made_absolute() {
    assert_eq!(
        absolute("https://huggingface.co/a/resolve/c/f", "/b/x?q=1"),
        "https://huggingface.co/b/x?q=1"
    );
    assert_eq!(
        absolute("https://huggingface.co/a", "https://us.aws.cdn.hf.co/y"),
        "https://us.aws.cdn.hf.co/y"
    );
}

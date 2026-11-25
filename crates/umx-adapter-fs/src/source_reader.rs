//! fs.source-reader (c-port-source-reader): confines evidence and resource sources to the roots (security ruling
//! ws006 S1). Roots are canonicalised once, here, as in [`crate::FsGraphFiles`]; every check resolves symlinks on
//! the real file system. `digest` reads one regular file as a stream (usage 220); nothing else is read.

use std::fs;
use std::io::{ErrorKind, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use umx_core::ports::{GraphDir, SourceError, SourceReader};
use umx_core::source_hash::{Digester, SOURCE_MAX_BYTES, SourceDigest, Transcript};

use crate::{dir_of, resolve};

pub struct FsSourceReader {
    roots: Vec<GraphDir>,
}

impl FsSourceReader {
    pub fn new(roots: Vec<GraphDir>) -> FsSourceReader {
        let roots = roots
            .into_iter()
            .map(|r| {
                resolve(Path::new(r.as_str()))
                    .ok()
                    .and_then(dir_of)
                    .unwrap_or(r)
            })
            .collect();
        FsSourceReader { roots }
    }

    fn within_roots(&self, dir: &GraphDir) -> bool {
        self.roots.iter().any(|r| dir.is_within(r))
    }
}

fn io(op: &str, e: &std::io::Error) -> SourceError {
    SourceError::io(format!("{op}: {}", e.kind()))
}

impl SourceReader for FsSourceReader {
    fn adapter_id(&self) -> &'static str {
        "fs.source-reader"
    }

    fn canonical_root(&self, folder: &GraphDir) -> Result<GraphDir, SourceError> {
        let canonical =
            resolve(Path::new(folder.as_str())).map_err(|e| io("resolve folder", &e))?;
        // Outside first, so nothing is said about whether a folder outside the roots exists.
        let dir = dir_of(canonical.clone()).ok_or(SourceError::OutsideRoots)?;
        if !self.within_roots(&dir) {
            return Err(SourceError::OutsideRoots);
        }
        match fs::metadata(&canonical) {
            Ok(m) if m.is_dir() => Ok(dir),
            Ok(_) => Err(SourceError::NotFolder),
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
                Err(SourceError::NotFolder)
            }
            Err(e) => Err(io("stat folder", &e)),
        }
    }

    fn check_path(&self, root: &GraphDir, path: &str) -> Result<(), SourceError> {
        if path.starts_with('/') || path.split('/').any(|p| p == "..") {
            return Err(SourceError::OutsideRoots);
        }
        let base = PathBuf::from(root.as_str());
        let inside = |p: &Path| dir_of(p.to_path_buf()).is_some_and(|d| d.is_within(root));
        let mut at = base.clone();
        for part in path.split('/').filter(|p| !p.is_empty() && *p != ".") {
            at.push(part);
            let meta = match fs::symlink_metadata(&at) {
                Ok(m) => m,
                Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
                    return Ok(());
                }
                Err(e) => return Err(io("stat path", &e)),
            };
            if meta.file_type().is_symlink() {
                // A dangling link cannot be shown to stay inside, so it is refused like one that leads out.
                let target = fs::canonicalize(&at).map_err(|_| SourceError::OutsideRoots)?;
                if !inside(&target) {
                    return Err(SourceError::OutsideRoots);
                }
                at = target;
            }
        }
        Ok(())
    }

    fn digest(
        &self,
        root: &GraphDir,
        path: &str,
        transcript: Option<Transcript>,
        prefix: Option<u64>,
    ) -> Result<SourceDigest, SourceError> {
        self.check_path(root, path)?;
        let missing = |e: &std::io::Error, op: &str| match e.kind() {
            ErrorKind::NotFound | ErrorKind::NotADirectory => SourceError::Missing,
            _ => io(op, e),
        };
        let file = fs::canonicalize(Path::new(root.as_str()).join(path))
            .map_err(|e| missing(&e, "resolve file"))?;
        if !dir_of(file.clone()).is_some_and(|d| d.is_within(root)) {
            return Err(SourceError::OutsideRoots);
        }
        // A fifo or device is never opened (it could block); O_NONBLOCK covers one swapped in after this check.
        let meta = fs::metadata(&file).map_err(|e| missing(&e, "stat file"))?;
        if !meta.is_file() {
            return Err(SourceError::NotFile);
        }
        // With a prefix under the cap only that many bytes are read, so a file that grew past the cap is still compared.
        let limit = prefix.filter(|n| *n <= SOURCE_MAX_BYTES);
        if limit.is_none() && meta.len() > SOURCE_MAX_BYTES {
            return Err(SourceError::TooLarge);
        }
        let mut f = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
            .open(&file)
            .map_err(|e| missing(&e, "open file"))?;
        let opened = f.metadata().map_err(|e| io("stat file", &e))?;
        if !opened.is_file() {
            return Err(SourceError::NotFile);
        }
        let mut digester = Digester::with_prefix(transcript, prefix);
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let want = limit.map_or(buf.len(), |n| {
                (n - digester.length()).min(buf.len() as u64) as usize
            });
            if want == 0 {
                break;
            }
            match f.read(&mut buf[..want]) {
                Ok(0) => break,
                Ok(n) => {
                    digester.update(&buf[..n]);
                    // A file that grew past the cap since the stat is not read further.
                    if limit.is_none() && digester.length() > SOURCE_MAX_BYTES {
                        return Err(SourceError::TooLarge);
                    }
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(io("read file", &e)),
            }
        }
        let mut d = digester.finish();
        if limit.is_some() {
            d.length = d.length.max(opened.len());
        }
        Ok(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Temp(PathBuf);

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// The contract's layout under `<temp>/work`, with `<temp>/outside` outside the roots.
    fn source_reader() -> (FsSourceReader, GraphDir, GraphDir, Temp) {
        static N: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "umx-src-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&base);
        let work = base.join("work");
        let outside = base.join("outside");
        fs::create_dir_all(work.join("src/sub")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(work.join("src/a.md"), "a\n").unwrap();
        fs::write(work.join("file.txt"), "f\n").unwrap();
        fs::write(
            work.join("src/t.ndjson"),
            umx_contracts::source_reader::TRANSCRIPT,
        )
        .unwrap();
        symlink(work.join("src/sub"), work.join("src/link_in")).unwrap();
        symlink(&outside, work.join("src/link_out")).unwrap();
        symlink(outside.join("missing"), work.join("src/dangling")).unwrap();
        symlink(&outside, work.join("link_root")).unwrap();
        let root = dir_of(work).unwrap();
        (
            FsSourceReader::new(vec![root.clone()]),
            root,
            dir_of(outside).unwrap(),
            Temp(base),
        )
    }

    /// A file over the cap is refused from its size, before any byte is read (a sparse file: nothing is written).
    #[test]
    fn a_file_over_the_cap_is_too_large() {
        let (port, root, _outside, _guard) = source_reader();
        let big = std::path::Path::new(root.as_str()).join("src/big.ndjson");
        fs::File::create(&big)
            .unwrap()
            .set_len(SOURCE_MAX_BYTES + 1)
            .unwrap();
        let src = root.join("src").unwrap();
        assert_eq!(
            port.digest(&src, "big.ndjson", Some(Transcript::Ndjson), None),
            Err(SourceError::TooLarge)
        );
    }

    /// A file cited small that grew past the cap: a prefix read takes only the stored bytes, and its length is the
    /// file's (review #74 note 1).
    #[test]
    fn a_prefix_read_of_a_file_grown_past_the_cap_reads_only_the_prefix() {
        let (port, root, _outside, _guard) = source_reader();
        let big = std::path::Path::new(root.as_str()).join("src/grown.md");
        fs::write(&big, b"cited\n").unwrap();
        let src = root.join("src").unwrap();
        let cited = port.digest(&src, "grown.md", None, None).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&big)
            .unwrap()
            .set_len(SOURCE_MAX_BYTES + 1)
            .unwrap();
        let now = port
            .digest(&src, "grown.md", None, Some(cited.length))
            .unwrap();
        assert_eq!(
            (now.prefix, now.length),
            (Some(cited.hash), SOURCE_MAX_BYTES + 1)
        );
    }

    umx_contracts::source_reader_contract!(
        source_reader;
        sr_c1_a_root_inside_is_canonical(),
        sr_c2_a_root_outside_the_roots_is_refused(),
        sr_c3_a_root_that_is_a_file_or_missing_is_not_a_folder(),
        sr_c4_dotdot_and_absolute_paths_are_refused(),
        sr_c5_a_symlink_out_is_refused(),
        sr_c6_a_missing_tail_is_fine(),
        sr_c7_a_file_is_digested(),
        sr_c8_a_digest_reads_nothing_it_may_not(),
    );
}

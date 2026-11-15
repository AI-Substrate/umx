//! An F32 copy of a BF16 GGUF, for the CPU path (ws004 s16). llama.cpp's CPU kernels run BF16 weights about 16 times
//! slower than F32 ones on Apple silicon, and no upstream F32 file exists, so the adapter derives it from the pinned
//! BF16 file. BF16 is the top half of an F32, so the widening is exact: every weight keeps its value and the vectors
//! keep their key. The output is a pure function of the input (the KV section is copied byte for byte with only
//! `general.file_type` changed, tensors keep their order, offsets are packed at the file's alignment), so it is pinned
//! by its own sha256, and tools/embed-bakeoff/widen_f32.py (gguf-py's writer) gives the same bytes.

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use sha2::{Digest, Sha256};

const GGUF_MAGIC: &[u8; 4] = b"GGUF";
const TYPE_F32: u32 = 0;
const TYPE_BF16: u32 = 30;
const KV_STRING: u32 = 8;
const KV_ARRAY: u32 = 9;
const ALL_F32: u32 = 0;
const DEFAULT_ALIGNMENT: u64 = 32;

fn bad(what: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.into())
}

/// Reads from the source and keeps a copy of every byte read, so the header can be written back unchanged.
struct Tee<R> {
    inner: R,
    copy: Vec<u8>,
    /// Bytes left in the file; no length read from it may exceed this.
    left: u64,
}

impl<R: Read> Tee<R> {
    fn bytes(&mut self, n: u64) -> io::Result<&[u8]> {
        if n > self.left {
            return Err(bad("a length runs past the end of the file"));
        }
        let start = self.copy.len();
        let n = usize::try_from(n).map_err(|_| bad("a length is too large"))?;
        self.copy.resize(start + n, 0);
        self.inner.read_exact(&mut self.copy[start..])?;
        self.left -= n as u64;
        Ok(&self.copy[start..])
    }

    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(
            self.bytes(4)?.try_into().unwrap_or_default(),
        ))
    }

    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(
            self.bytes(8)?.try_into().unwrap_or_default(),
        ))
    }

    fn string(&mut self) -> io::Result<String> {
        let n = self.u64()?;
        Ok(String::from_utf8_lossy(self.bytes(n)?).into_owned())
    }

    /// Skips one value of a KV type, returning the u32 when it is one.
    fn value(&mut self, ty: u32) -> io::Result<Option<u32>> {
        let fixed = match ty {
            0 | 1 | 7 => 1,
            2 | 3 => 2,
            4 => return self.u32().map(Some),
            5 | 6 => 4,
            10..=12 => 8,
            KV_STRING => {
                let n = self.u64()?;
                self.bytes(n)?;
                return Ok(None);
            }
            KV_ARRAY => {
                let item = self.u32()?;
                let n = self.u64()?;
                if item == KV_STRING || item == KV_ARRAY {
                    for _ in 0..n {
                        self.value(item)?;
                    }
                } else {
                    let size = match item {
                        0 | 1 | 7 => 1,
                        2 | 3 => 2,
                        4..=6 => 4,
                        10..=12 => 8,
                        _ => return Err(bad(format!("unknown array item type {item}"))),
                    };
                    self.bytes(n.checked_mul(size).ok_or_else(|| bad("array too large"))?)?;
                }
                return Ok(None);
            }
            _ => return Err(bad(format!("unknown value type {ty}"))),
        };
        self.bytes(fixed)?;
        Ok(None)
    }
}

struct Tensor {
    /// Where the tensor's type and offset sit in the copied header.
    at: usize,
    elements: u64,
    ty: u32,
    offset: u64,
}

fn pad(n: u64, align: u64) -> u64 {
    n.div_ceil(align) * align
}

/// Hashes everything written through it.
struct Hashing<W> {
    inner: W,
    hash: Sha256,
}

impl<W: Write> Write for Hashing<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hash.update(&buf[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Creates `path` new, readable and writable by its owner only.
pub(crate) fn create_owner_only(path: &Path) -> io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)
}

/// Writes the F32 copy of the GGUF at `src` to `dst` (created new, mode 0600) and returns its sha256 (lower-case hex). Every
/// tensor must be BF16 or F32.
pub fn widen_bf16_to_f32(src: &Path, dst: &Path) -> io::Result<String> {
    widen_into(src, &create_owner_only(dst)?)
}

/// [`widen_bf16_to_f32`] into an open, empty file, which the caller keeps (to check it is still the one at its path).
pub(crate) fn widen_into(src: &Path, dst: &File) -> io::Result<String> {
    let file = File::open(src)?;
    let size = file.metadata()?.len();
    let mut r = Tee {
        inner: BufReader::with_capacity(1 << 20, file),
        copy: Vec::new(),
        left: size,
    };
    if r.bytes(4)? != GGUF_MAGIC {
        return Err(bad("not a GGUF file"));
    }
    let version = r.u32()?;
    if version != 3 {
        return Err(bad(format!("GGUF version {version}, not 3")));
    }
    let tensor_count = r.u64()?;
    let kv_count = r.u64()?;
    let mut alignment = DEFAULT_ALIGNMENT;
    let mut file_type_at = None;
    for _ in 0..kv_count {
        let key = r.string()?;
        let ty = r.u32()?;
        let at = r.copy.len();
        let value = r.value(ty)?;
        match (key.as_str(), value) {
            ("general.file_type", Some(_)) => file_type_at = Some(at),
            ("general.alignment", Some(a)) if a > 0 && a.is_power_of_two() => alignment = a.into(),
            ("general.alignment", _) => return Err(bad("general.alignment is not a power of two")),
            _ => {}
        }
    }
    let mut tensors = Vec::new();
    for _ in 0..tensor_count {
        r.string()?;
        let dims = r.u32()?;
        let mut elements: u64 = 1;
        for _ in 0..dims {
            elements = elements
                .checked_mul(r.u64()?)
                .ok_or_else(|| bad("tensor too large"))?;
        }
        let at = r.copy.len();
        let ty = r.u32()?;
        let offset = r.u64()?;
        if ty != TYPE_BF16 && ty != TYPE_F32 {
            return Err(bad(format!("a tensor has type {ty}, neither BF16 nor F32")));
        }
        tensors.push(Tensor {
            at,
            elements,
            ty,
            offset,
        });
    }
    let data_start = (r.copy.len() as u64)
        .checked_next_multiple_of(alignment)
        .ok_or_else(|| bad("header too large"))?;
    let mut header = r.copy;
    if let Some(at) = file_type_at {
        header[at..at + 4].copy_from_slice(&ALL_F32.to_le_bytes());
    }
    let mut next = 0u64;
    for t in &tensors {
        header[t.at..t.at + 4].copy_from_slice(&TYPE_F32.to_le_bytes());
        header[t.at + 4..t.at + 12].copy_from_slice(&next.to_le_bytes());
        next = t
            .elements
            .checked_mul(4)
            .and_then(|b| b.checked_add(next))
            .and_then(|end| end.checked_next_multiple_of(alignment))
            .ok_or_else(|| bad("tensor sizes overflow"))?;
    }

    let mut src = BufReader::with_capacity(1 << 20, File::open(src)?);
    let mut out = Hashing {
        inner: BufWriter::with_capacity(1 << 20, dst),
        hash: Sha256::new(),
    };
    out.write_all(&header)?;
    let zeros = vec![0u8; alignment as usize];
    out.write_all(&zeros[..(data_start - header.len() as u64) as usize])?;
    let mut written = 0u64;
    let (mut inb, mut outb) = (Vec::new(), Vec::new());
    for t in &tensors {
        let width: u64 = if t.ty == TYPE_BF16 { 2 } else { 4 };
        let bytes = t
            .elements
            .checked_mul(width)
            .ok_or_else(|| bad("tensor size overflows"))?;
        let end = data_start
            .checked_add(t.offset)
            .and_then(|start| start.checked_add(bytes));
        if end.is_none_or(|end| end > size) {
            return Err(bad("a tensor runs past the end of the file"));
        }
        src.seek(SeekFrom::Start(data_start + t.offset))?;
        let mut left = bytes;
        while left > 0 {
            let n = left.min(1 << 20) as usize;
            inb.resize(n, 0);
            src.read_exact(&mut inb)?;
            if t.ty == TYPE_BF16 {
                outb.clear();
                for &[lo, hi] in inb.as_chunks::<2>().0 {
                    outb.extend_from_slice(&[0, 0, lo, hi]);
                }
                out.write_all(&outb)?;
            } else {
                out.write_all(&inb)?;
            }
            left -= n as u64;
        }
        // Cannot overflow: the same sums were checked when the offsets were laid out above.
        let end = written + t.elements * 4;
        written = pad(end, alignment);
        out.write_all(&zeros[..(written - end) as usize])?;
    }
    out.flush()?;
    let Hashing { inner, hash } = out;
    inner.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

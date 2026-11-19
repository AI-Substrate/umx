//! Local embedder (blueprint 001, c-adapter-llamacpp): llama.cpp over a pinned GGUF model read from disk.
//!
//! No network, by construction: this crate depends on no HTTP library, and llama.cpp's download code (in its `common`
//! library, built by llama-cpp-sys-2) is never called and is not linked; a test checks the linked binary. The model
//! file is put in place by a separate, deliberate step (`umx models fetch`, ws004 s14), and its
//! sha256 is checked before every load, so a swapped or damaged file is refused by name before llama.cpp parses it.
//! llama.cpp then opens the file again by path; the window between the check and that open is accepted (architect,
//! C33): the models folder is the user's own. A missing file makes `embed` and `ready` return `Unavailable`
//! with the fetch step as `next`; the model loads on first use and again after a failure.
//!
//! Each session is a worker thread that owns its model and llama.cpp context (a context borrows its model, so the
//! two live together on that thread); `embed` hands it the texts and waits. Metal runs the graph on macOS (about 70
//! texts a second for embeddinggemma-2); a build with the `cuda` feature runs it on an NVIDIA GPU when llama.cpp finds
//! one (ws004 s19), from the same BF16 file under the same key. Elsewhere it runs on the CPU. There it loads the spec's CPU file when it has
//! one: for embeddinggemma-2 an exact F32 copy of the BF16 file, made beside it on first use (about 4 s, 1.1 GB) and
//! pinned by its own sha256, because llama.cpp's CPU kernels run BF16 about 16 times slower (ws004 s16). The floors
//! differ: 10 texts a second on Metal, 2 on the CPU (embeddings-expert, s16). The F32 file runs at about 3.6 at 4
//! threads, so it meets the CPU floor: `backend()` reports `cpu` and `cpu_note()` (`local_embedder_cpu`) says so.
//! `slow_warning()` (`local_embedder_slow`) fires only under the CPU floor: with no CPU file, or a measured rate
//! under 2. The configuration is built by compose (s05) and passed in; this crate reads no
//! environment.

mod spec;
mod widen;

use std::fmt;
use std::fs::File;
use std::io::Read;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, mpsc};
use std::time::Duration;

use llama_cpp_2::LogOptions;
use llama_cpp_2::context::params::{LlamaContextParams, LlamaPoolingType};
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::model::params::LlamaModelParams;
use sha2::{Digest, Sha256};
use umx_core::ports::{Credential, EmbedError, Embedded, Embedder, Locality, ModelKey, TextKind};

pub use spec::{ModelFile, ModelSpec, Pooling};
pub use widen::widen_bf16_to_f32;

/// Tokens one graph run may hold (llama.cpp's n_ctx, n_batch and n_ubatch). Texts are packed into runs of up to
/// `batch_size` texts and this many tokens; a text is never longer than `max_tokens`, which may not exceed this, so a
/// run never overflows and one long text cannot fail a whole batch.
const RUN_TOKENS: usize = 8192;

/// Where the graph runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Execution {
    /// CPU only: no layers or operations on a GPU.
    Cpu,
    /// The GPU llama.cpp was built for: Metal on macOS, CUDA with the `cuda` feature. Builds without a GPU backend, and
    /// machines without a GPU device, run on the CPU.
    Gpu,
}

/// What runs the graph, as doctor and status should show it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Metal,
    Cuda,
    Cpu,
}

impl Backend {
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::Metal => "metal",
            Backend::Cuda => "cuda",
            Backend::Cpu => "cpu",
        }
    }
}

/// The GPU backend this build of llama.cpp has: Metal on macOS, CUDA with the `cuda` feature, else none.
pub const BUILT_GPU: Option<Backend> = if cfg!(target_os = "macos") {
    Some(Backend::Metal)
} else if cfg!(feature = "cuda") {
    Some(Backend::Cuda)
} else {
    None
};

/// Warning code for a local embedder running on the CPU only. llama.cpp's CPU path embeds this model at about 3 texts
/// a second at 4 threads from the F32 file (ws004 s16; 0.2 from the BF16 file), under the floor of 10, so it is never
/// silent. Faster CPU kernels (KleidiAI, Accelerate) wait on an architect ruling; CUDA is the Linux GPU route.
pub const LOCAL_EMBEDDER_SLOW: &str = "local_embedder_slow";

/// Code of the note for a local embedder on the CPU at or above the CPU floor: not a warning, but large ingests are slow.
pub const LOCAL_EMBEDDER_CPU: &str = "local_embedder_cpu";

/// Code in the refusal when the disk has no room for the CPU file.
pub const MODEL_DISK_SPACE: &str = "model_disk_space";

/// The CPU-only floor (embeddings-expert, ws004 s16): 2 texts of about 300 tokens a second at 4 threads. The F32 file
/// runs at about 3; the BF16 file at about 0.2. Metal keeps the floor of 10.
pub const CPU_FLOOR: f64 = 2.0;

/// Document texts embedded before the measured rate is trusted.
const RATE_MIN_TEXTS: usize = 32;

/// The CPU rate per thread from the F32 file, used until a rate is measured: 3.6 texts of 300 tokens a second at 4
/// threads on Apple M4 (ws004 s16, plan 051).
pub const CPU_TEXTS_PER_S_PER_THREAD: f64 = 0.9;

#[derive(Clone, Debug)]
pub struct LlamaConfig {
    pub spec: ModelSpec,
    /// The models folder, normally `~/.cache/umx/models`; compose resolves it.
    pub models_dir: PathBuf,
    /// The graph's vector width: one of `spec.dims`.
    pub dims: usize,
    /// CPU threads per session.
    pub threads: usize,
    /// Sessions in the pool, each loading the model once. 1 (no pool) by default.
    pub sessions: usize,
    /// Texts per graph run.
    pub batch_size: usize,
    pub execution: Execution,
    /// How long one `embed` waits for its worker: this times (texts + 1). Past it the call fails with `BadResponse`,
    /// the late worker is set aside (it stops after its job; dropping the embedder waits for it) and the next call loads
    /// the model again.
    pub timeout_per_text: Duration,
    /// Free bytes on the disk holding a path, checked before the CPU file is written (compose passes the fs adapter's
    /// `available_bytes`). None skips the check.
    pub available_bytes: Option<fn(&Path) -> std::io::Result<u64>>,
    /// Takes the models folder's exclusive lock (compose passes the fs adapter's `models_folder_lock`, which the model
    /// fetcher also takes); held while the CPU file is written. None takes no lock and removes no stale temporary file.
    pub models_lock: Option<fn(&Path) -> std::io::Result<File>>,
    /// The CPU-only floor in texts a second (embeddings-expert, ws004 s16): below it `slow_warning` fires.
    pub cpu_floor: f64,
}

impl LlamaConfig {
    /// The embeddings-expert's defaults: width 256, one session of 4 threads, batches of 16, the GPU when this build
    /// has a GPU backend (Metal on macOS, CUDA with the `cuda` feature).
    pub fn new(spec: ModelSpec, models_dir: PathBuf) -> LlamaConfig {
        LlamaConfig {
            spec,
            models_dir,
            dims: 256,
            threads: 4,
            sessions: 1,
            batch_size: 16,
            execution: if BUILT_GPU.is_some() {
                Execution::Gpu
            } else {
                Execution::Cpu
            },
            timeout_per_text: Duration::from_secs(60),
            available_bytes: None,
            models_lock: None,
            cpu_floor: CPU_FLOOR,
        }
    }

    /// `<models dir>/<org>--<repo>/<commit>/`.
    pub fn model_dir(&self) -> PathBuf {
        self.models_dir
            .join(self.spec.repo.replace('/', "--"))
            .join(&self.spec.commit)
    }
}

pub struct LlamaEmbedder {
    config: LlamaConfig,
    key: ModelKey,
    workers: Mutex<Option<Arc<Workers>>>,
    next_worker: AtomicUsize,
    sent: AtomicUsize,
    hold: Arc<Hold>,
    /// Workers that missed a deadline, their queues closed when they were set aside: they finish the jobs already
    /// queued and stop. Joined by [`LlamaEmbedder::release`], so none is still freeing llama.cpp state at exit.
    late: Mutex<Vec<std::thread::JoinHandle<()>>>,
    /// Document texts embedded and the time their calls took, for the measured rate.
    rate: Mutex<(usize, Duration)>,
    /// Set by [`LlamaEmbedder::release`]: the model is never loaded again.
    released: AtomicBool,
    /// Worker threads that have not yet freed their model, of every generation.
    alive: Arc<AtomicUsize>,
}

/// Counts one worker thread from its start until it has freed its context and model (it drops last).
struct Alive(Arc<AtomicUsize>);

impl Alive {
    fn new(count: Arc<AtomicUsize>) -> Alive {
        count.fetch_add(1, Ordering::SeqCst);
        Alive(count)
    }
}

impl Drop for Alive {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Drop for LlamaEmbedder {
    fn drop(&mut self) {
        self.release();
    }
}

/// Lets a test stop the workers before their next job, so a missed deadline can be tested without racing the clock.
#[derive(Default)]
struct Hold {
    held: Mutex<bool>,
    released: Condvar,
}

impl Hold {
    fn wait_while_held(&self) {
        let held = self.held.lock().unwrap_or_else(|p| p.into_inner());
        drop(
            self.released
                .wait_while(held, |h| *h)
                .unwrap_or_else(|p| p.into_inner()),
        );
    }
}

/// While alive, workers wait before starting a job. Tests only.
#[doc(hidden)]
pub struct HeldWorkers<'a>(&'a Hold);

impl Drop for HeldWorkers<'_> {
    fn drop(&mut self) {
        *self.0.held.lock().unwrap_or_else(|p| p.into_inner()) = false;
        self.0.released.notify_all();
    }
}

struct Workers {
    /// One queue per session; emptied to stop them.
    jobs: Mutex<Vec<mpsc::Sender<Job>>>,
    threads: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

impl Workers {
    /// Closes the job queues: each worker finishes the jobs already in its queue, frees its context and model, and
    /// ends, whoever still holds these workers. A call that comes later finds no queue.
    fn close(&self) {
        self.jobs.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }

    /// Closes the queues and waits for each worker to free its context, then its model, so llama.cpp and the GPU
    /// device are never torn down by process exit while a worker still holds them (F207).
    fn stop(&self) {
        self.close();
        let threads = std::mem::take(&mut *self.threads.lock().unwrap_or_else(|p| p.into_inner()));
        for t in threads {
            let _ = t.join();
        }
    }
}

impl Drop for Workers {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Job {
    texts: Vec<String>,
    reply: mpsc::Sender<Result<Embedded, EmbedError>>,
}

impl fmt::Debug for LlamaEmbedder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LlamaEmbedder")
            .field("key", &self.key.to_string())
            .field("model_dir", &self.config.model_dir())
            .field("sessions", &self.config.sessions)
            .field("threads", &self.config.threads)
            .field("execution", &self.config.execution)
            .finish()
    }
}

/// llama.cpp's process-wide backend, initialised once. Its logs are switched off first, before the backend and the
/// GPU device start: they can quote prompt and token text, which must never reach stderr or daemon.log (req-48).
static BACKEND: OnceLock<Result<LlamaBackend, String>> = OnceLock::new();

/// Whether this process has started llama.cpp's backend (tests: an absent model must fail before it, CI).
#[doc(hidden)]
pub fn backend_started() -> bool {
    BACKEND.get().is_some()
}

fn llama() -> Result<&'static LlamaBackend, EmbedError> {
    BACKEND
        .get_or_init(|| {
            llama_cpp_2::send_logs_to_tracing(LogOptions::default().with_logs_enabled(false));
            LlamaBackend::init().map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|e| EmbedError::Config(format!("llama.cpp backend: {e}")))
}

impl LlamaEmbedder {
    /// Checks the configuration only; the model loads on the first `embed` or `ready`.
    pub fn new(config: LlamaConfig) -> Result<LlamaEmbedder, EmbedError> {
        let spec = &config.spec;
        if !spec.dims.contains(&config.dims) {
            return Err(EmbedError::Config(format!(
                "{} gives widths {:?}, not {}",
                spec.name, spec.dims, config.dims
            )));
        }
        for (what, n) in [
            ("threads", config.threads),
            ("sessions", config.sessions),
            ("batch_size", config.batch_size),
        ] {
            if n == 0 {
                return Err(EmbedError::Config(format!("{what} must be at least 1")));
            }
        }
        if spec.max_tokens < 2 || spec.max_tokens > RUN_TOKENS {
            return Err(EmbedError::Config(format!(
                "max_tokens must be 2 to {RUN_TOKENS}, not {}",
                spec.max_tokens
            )));
        }
        let key = ModelKey::new(&spec.name, config.dims)
            .map_err(|e| EmbedError::Config(format!("model name {:?}: {e}", spec.name)))?
            .with_prompt_version(spec.prompt_version);
        Ok(LlamaEmbedder {
            config,
            key,
            workers: Mutex::new(None),
            next_worker: AtomicUsize::new(0),
            sent: AtomicUsize::new(0),
            hold: Arc::default(),
            late: Mutex::default(),
            rate: Mutex::default(),
            released: AtomicBool::new(false),
            alive: Arc::default(),
        })
    }

    /// Frees the model for good: closes every session's queue, waits for each worker to finish the jobs already
    /// queued and free its context and then its model (workers set aside after a missed deadline too), and refuses to
    /// load it again; later calls get `Unavailable`. Call it before the process exits while other threads may still
    /// hold this embedder: exit runs ggml's static Metal teardown, which aborts while a model is loaded (F207).
    /// It joins while holding no lock, so a call timing out meanwhile, which needs `late`, never waits on it.
    /// Idempotent; `Drop` calls it too.
    pub fn release(&self) {
        self.released.store(true, Ordering::SeqCst);
        let current = self
            .workers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        if let Some(w) = current {
            w.stop();
        }
        // After the current workers: a call that set them aside first moved their threads here under `late`, and
        // one that comes later finds none to move. Their queues were closed when they were set aside.
        let late = std::mem::take(&mut *self.late.lock().unwrap_or_else(|p| p.into_inner()));
        for t in late {
            let _ = t.join();
        }
    }

    /// Worker threads of any generation that have not yet freed their model. Tests: zero once `release` returns.
    #[doc(hidden)]
    pub fn sessions_alive(&self) -> usize {
        self.alive.load(Ordering::SeqCst)
    }

    fn released_error(&self) -> EmbedError {
        EmbedError::Unavailable {
            reason: format!(
                "the local model {} was released: this process is stopping",
                self.config.spec.name
            ),
            next: "run the command again once the daemon is back".to_string(),
        }
    }

    /// What actually runs the graph: the build's GPU backend (Metal on macOS, CUDA with the `cuda` feature) only when
    /// the GPU is chosen and llama.cpp finds a GPU device it can offload to; otherwise the CPU, which llama.cpp falls
    /// back to on its own (no Metal device, a VM, no NVIDIA driver or device).
    pub fn backend(&self) -> Backend {
        match BUILT_GPU {
            Some(gpu)
                if self.config.execution == Execution::Gpu
                    && llama().is_ok_and(LlamaBackend::supports_gpu_offload) =>
            {
                gpu
            }
            _ => Backend::Cpu,
        }
    }

    /// The configuration this embedder was built with.
    pub fn config(&self) -> &LlamaConfig {
        &self.config
    }

    /// Texts a second over the document embeds so far, once at least 32 texts have been embedded.
    pub fn measured_texts_per_s(&self) -> Option<f64> {
        let (texts, took) = *self.rate.lock().unwrap_or_else(|p| p.into_inner());
        (texts >= RATE_MIN_TEXTS && !took.is_zero()).then(|| texts as f64 / took.as_secs_f64())
    }

    /// `local_embedder_slow` when the model runs on the CPU under the CPU floor: the spec has no CPU file (BF16 on the
    /// CPU, about 0.2 texts a second), or [`Self::cpu_texts_per_s`] is under the floor (measured, or estimated from
    /// too few threads before anything is measured).
    pub fn slow_warning(&self) -> Option<String> {
        if self.backend() != Backend::Cpu {
            return None;
        }
        let floor = self.config.cpu_floor;
        if self.config.spec.cpu_file.is_none() {
            return Some(format!(
                "{LOCAL_EMBEDDER_SLOW}: the local embedder runs on the cpu only, from {} (this model has no faster CPU \
                 file), well under the CPU floor of {floor} texts a second; use a GPU",
                self.config.spec.file.path
            ));
        }
        let rate = self.cpu_texts_per_s().filter(|r| *r < floor)?;
        Some(format!(
            "{LOCAL_EMBEDDER_SLOW}: the local embedder runs on the cpu at about {rate:.1} texts a second, under the \
             CPU floor of {floor}; raise threads or use a GPU"
        ))
    }

    /// On the CPU with a CPU file: the measured rate once there is one, else the estimate from the threads.
    pub fn cpu_texts_per_s(&self) -> Option<f64> {
        (self.backend() == Backend::Cpu && self.config.spec.cpu_file.is_some()).then(|| {
            self.measured_texts_per_s()
                .unwrap_or(CPU_TEXTS_PER_S_PER_THREAD * self.config.threads as f64)
        })
    }

    /// Why a cuda build runs on the CPU though the GPU was chosen, so a missing driver or device is not mistaken for a
    /// slow GPU: None elsewhere (Metal builds keep their existing report).
    pub fn fallback_reason(&self) -> Option<&'static str> {
        (BUILT_GPU == Some(Backend::Cuda)
            && self.config.execution == Execution::Gpu
            && self.backend() == Backend::Cpu)
            .then_some("this cuda build found no CUDA device")
    }

    /// `local_embedder_cpu`, a note rather than a warning, when the model runs on the CPU at or above the CPU floor.
    pub fn cpu_note(&self) -> Option<String> {
        if self.backend() != Backend::Cpu || self.slow_warning().is_some() {
            return None;
        }
        let rate = self.cpu_texts_per_s()?;
        let why = self
            .fallback_reason()
            .map_or(String::new(), |r| format!(" ({r})"));
        Some(format!(
            "{LOCAL_EMBEDDER_CPU}: the local embedder runs on the cpu{why} at about {rate:.1} texts a second with {} \
             threads; large ingests take hours; raise threads to speed it up",
            self.config.threads
        ))
    }

    /// Makes the workers wait before their next job until the guard is dropped. Tests only: it lets a test miss the
    /// reply deadline for certain.
    #[doc(hidden)]
    pub fn hold_workers_for_test(&self) -> HeldWorkers<'_> {
        *self.hold.held.lock().unwrap_or_else(|p| p.into_inner()) = true;
        HeldWorkers(&self.hold)
    }

    /// Texts handed to the model so far. Nothing leaves the machine; tests use it to show `ready` embeds nothing.
    pub fn texts_embedded(&self) -> usize {
        self.sent.load(Ordering::Relaxed)
    }

    fn fetch_hint(&self) -> String {
        format!(
            "run `umx models fetch {}` (it puts {} at commit {} in {})",
            self.config.spec.name,
            self.config.spec.repo,
            self.config.spec.commit,
            self.config.model_dir().display()
        )
    }

    fn workers(&self) -> Result<Arc<Workers>, EmbedError> {
        let mut slot = self.workers.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(w) = slot.as_ref() {
            return Ok(w.clone());
        }
        // Checked under the slot's lock: a release that set the flag takes the slot after this load, or none starts.
        if self.released.load(Ordering::SeqCst) {
            return Err(self.released_error());
        }
        let w = Arc::new(self.start()?);
        *slot = Some(w.clone());
        Ok(w)
    }

    /// Stops handing out `workers`, so the next call starts fresh ones; workers another call has started since stay.
    fn forget(&self, workers: &Arc<Workers>) {
        let mut slot = self.workers.lock().unwrap_or_else(|p| p.into_inner());
        if slot.as_ref().is_some_and(|w| Arc::ptr_eq(w, workers)) {
            *slot = None;
        }
    }

    /// Checks the file, then starts one worker per session, each loading the model on its own thread.
    fn start(&self) -> Result<Workers, EmbedError> {
        let path = self.model_path()?;
        let backend = llama()?;
        let started = Workers {
            jobs: Mutex::default(),
            threads: Mutex::default(),
        };
        for _ in 0..self.config.sessions {
            let (job_tx, job_rx) = mpsc::channel::<Job>();
            let (ready_tx, ready_rx) = mpsc::channel();
            let config = self.config.clone();
            let hold = self.hold.clone();
            let alive = self.alive.clone();
            let path = path.clone();
            let thread = std::thread::Builder::new()
                .name("umx-llamacpp".to_string())
                .spawn(move || {
                    let _alive = Alive::new(alive);
                    worker(backend, &config, &path, &hold, ready_tx, job_rx);
                })
                .map_err(|e| EmbedError::Config(format!("starting a llama.cpp worker: {e}")))?;
            let loaded = ready_rx.recv().unwrap_or_else(|_| {
                Err(EmbedError::BadResponse(
                    "the llama.cpp worker stopped while loading".to_string(),
                ))
            });
            if let Err(e) = loaded {
                let _ = thread.join();
                // Sessions already loaded stop and free their models as `started` drops.
                return Err(e);
            }
            started
                .jobs
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(job_tx);
            started
                .threads
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(thread);
        }
        Ok(started)
    }

    /// The checked file to load: on the CPU, the spec's CPU file when it has one, made from the main file on first use;
    /// otherwise the main file.
    fn model_path(&self) -> Result<PathBuf, EmbedError> {
        let spec = &self.config.spec;
        let dir = self.config.model_dir();
        let main = dir.join(&spec.file.path);
        // No model file at all: say so before asking which backend runs it, which starts llama.cpp (Metal on a Mac,
        // seconds on a CI VM). A probe of an instance whose model was never fetched answers at once (CI).
        let cpu_there = spec
            .cpu_file
            .as_ref()
            .is_some_and(|c| dir.join(&c.path).exists());
        if !cpu_there && !main.exists() {
            self.verify(&main, &spec.file.sha256, None)?;
        }
        let Some(cpu) = spec
            .cpu_file
            .as_ref()
            .filter(|_| self.backend() == Backend::Cpu)
        else {
            self.verify(&main, &spec.file.sha256, None)?;
            return Ok(main);
        };
        let path = dir.join(&cpu.path);
        if !path.exists() {
            // The models folder's lock, shared with the model fetcher (s14): one writer at a time across processes,
            // held from before the free-space check until after the rename.
            let lock =
                match self.config.models_lock {
                    Some(lock) => Some(lock(&self.config.models_dir).map_err(|e| {
                        EmbedError::Unavailable {
                            reason: format!(
                                "cannot lock the models folder {}: {e}",
                                self.config.models_dir.display()
                            ),
                            next: format!(
                                "check that {} is yours and writable",
                                self.config.models_dir.display()
                            ),
                        }
                    })?),
                    None => None,
                };
            if !path.exists() {
                self.verify(&main, &spec.file.sha256, None)?;
                self.widen(&main, &path, &cpu.sha256, lock.is_some())?;
            }
        }
        self.verify(&path, &cpu.sha256, Some(&main))?;
        Ok(path)
    }

    /// Writes the exact F32 copy of `main` at `path`, through a temporary file renamed into place only when its sha256
    /// is the pinned one.
    fn widen(&self, main: &Path, path: &Path, want: &str, locked: bool) -> Result<(), EmbedError> {
        static ATTEMPT: AtomicUsize = AtomicUsize::new(0);
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if locked {
            // Every writer holds the lock while its temporary file exists, so any found now were left by a crash.
            remove_parts(path);
        }
        let part = path.with_file_name(format!(
            "{name}.{}.{}.part",
            std::process::id(),
            ATTEMPT.fetch_add(1, Ordering::Relaxed)
        ));
        let failed = |reason: String| {
            let _ = std::fs::remove_file(&part);
            EmbedError::Unavailable {
                reason,
                next: format!(
                    "make room for {name} (about {} GB) in {} and try again, or use a GPU",
                    std::fs::metadata(main)
                        .map_or(0, |m| m.len() * 2)
                        .div_ceil(1 << 30),
                    path.parent().unwrap_or(path).display()
                ),
            }
        };
        if let Some(available) = self.config.available_bytes {
            let need = std::fs::metadata(main).map_or(0, |m| m.len() * 2) + (64 << 20);
            let dir = path.parent().unwrap_or(path);
            match available(dir) {
                Ok(free) if free < need => {
                    return Err(EmbedError::Unavailable {
                        reason: format!(
                            "{MODEL_DISK_SPACE}: {name} needs {} MB in {} and {} MB are free; nothing was written",
                            need >> 20,
                            dir.display(),
                            free >> 20
                        ),
                        next: format!(
                            "free {} MB on that disk, or use a GPU",
                            (need - free).div_ceil(1 << 20)
                        ),
                    });
                }
                Ok(_) => {}
                Err(e) => {
                    return Err(failed(format!(
                        "cannot read the free space in {}: {e}",
                        dir.display()
                    )));
                }
            }
        }
        let file = widen::create_owner_only(&part).map_err(|e| EmbedError::Unavailable {
            reason: format!("cannot create {}: {e}", part.display()),
            next: format!(
                "check that {} is yours and writable",
                path.parent().unwrap_or(path).display()
            ),
        })?;
        match widen::widen_into(main, &file) {
            Ok(got) if got == want && !same_file(&file, &part) => Err(failed(format!(
                "{} was replaced while it was written; refusing to keep it",
                part.display()
            ))),
            Ok(got) if got == want => std::fs::rename(&part, path).map_err(|e| {
                failed(format!(
                    "cannot move the CPU model file into {}: {e}",
                    path.display()
                ))
            }),
            Ok(got) => Err(failed(format!(
                "the F32 copy of {} has sha256 {got}, not the pinned {want}; refusing to keep it",
                main.display()
            ))),
            Err(e) => Err(failed(format!(
                "cannot write the F32 copy of {} at {}: {e}",
                main.display(),
                part.display()
            ))),
        }
    }

    /// Refuses a missing file or one whose sha256 is not the spec's, naming it. `made_from` names the file a derived
    /// file is made from: deleting the derived one is then the whole fix.
    fn verify(&self, path: &Path, want: &str, made_from: Option<&Path>) -> Result<(), EmbedError> {
        let unreadable = |e: std::io::Error| EmbedError::Unavailable {
            reason: format!("model file {} cannot be read: {e}", path.display()),
            next: self.fetch_hint(),
        };
        let mut file = File::open(path).map_err(unreadable)?;
        let mut hash = Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = file.read(&mut buf).map_err(unreadable)?;
            if n == 0 {
                break;
            }
            hash.update(&buf[..n]);
        }
        let got: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if got != want {
            return Err(EmbedError::Unavailable {
                reason: format!(
                    "model file {} has sha256 {got}, not the pinned {want}; refusing to load it",
                    path.display()
                ),
                next: match made_from {
                    Some(main) => format!("delete it; umx makes it again from {}", main.display()),
                    None => format!("delete it, then {}", self.fetch_hint()),
                },
            });
        }
        Ok(())
    }
}

/// Removes the `<file>.<pid>.<n>.part` files beside `path`. Called only by the holder of the models folder's lock,
/// when no other widening can be running, so each one is left from a crash.
fn remove_parts(path: &Path) {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return;
    };
    let prefix = format!("{}.", name.to_string_lossy());
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let file = entry.file_name();
        let file = file.to_string_lossy();
        if file.starts_with(&prefix) && file.ends_with(".part") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Whether `path` is still the file `file` was opened as (device and inode), so a rename moves what was written.
fn same_file(file: &File, path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (file.metadata(), std::fs::symlink_metadata(path)) {
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (file, path);
        true
    }
}

/// One session: load the model and a context on this thread, report, then embed jobs until the embedder is dropped.
fn worker(
    backend: &LlamaBackend,
    config: &LlamaConfig,
    path: &Path,
    hold: &Hold,
    ready: mpsc::Sender<Result<(), EmbedError>>,
    jobs: mpsc::Receiver<Job>,
) {
    let gpu_layers = match config.execution {
        Execution::Gpu => 999,
        Execution::Cpu => 0,
    };
    let model = match LlamaModel::load_from_file(
        backend,
        path,
        &LlamaModelParams::default().with_n_gpu_layers(gpu_layers),
    ) {
        Ok(m) => m,
        Err(e) => {
            let _ = ready.send(Err(EmbedError::Config(format!(
                "llama.cpp cannot load {}: {e}",
                path.display()
            ))));
            return;
        }
    };
    let native = config.spec.dims[0];
    if usize::try_from(model.n_embd_out()).ok() != Some(native) {
        let _ = ready.send(Err(EmbedError::Config(format!(
            "{} gives {}-wide vectors, not the spec's {native}",
            path.display(),
            model.n_embd_out()
        ))));
        return;
    }
    let threads = i32::try_from(config.threads).unwrap_or(i32::MAX);
    let params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(RUN_TOKENS as u32))
        .with_n_batch(RUN_TOKENS as u32)
        .with_n_ubatch(RUN_TOKENS as u32)
        .with_n_seq_max(u32::try_from(config.batch_size).unwrap_or(u32::MAX))
        .with_n_threads(threads)
        .with_n_threads_batch(threads)
        .with_embeddings(true)
        .with_op_offload(config.execution == Execution::Gpu)
        .with_pooling_type(match config.spec.pooling {
            Pooling::Mean => LlamaPoolingType::Mean,
        });
    let mut ctx = match model.new_context(backend, params) {
        Ok(c) => c,
        Err(e) => {
            let _ = ready.send(Err(EmbedError::Config(format!(
                "llama.cpp context for {}: {e}",
                path.display()
            ))));
            return;
        }
    };
    if ready.send(Ok(())).is_err() {
        return;
    }
    let vocab = model.vocab();
    let max = config.spec.max_tokens;
    for job in jobs {
        hold.wait_while_held();
        let mut out = Embedded::default();
        let mut tokens = Vec::with_capacity(job.texts.len());
        for (i, text) in job.texts.iter().enumerate() {
            let mut t = vocab.tokenize(text.as_bytes(), true, false);
            if t.len() > max {
                if vocab.should_add_eos() {
                    // Keep the closing special token, as the model's own tokenizer does when it truncates.
                    let last = t[t.len() - 1];
                    t.truncate(max - 1);
                    t.push(last);
                } else {
                    t.truncate(max);
                }
                out.truncated.push(i);
            }
            tokens.push(t);
        }
        let result = (|| {
            let mut i = 0;
            while i < tokens.len() {
                let (mut j, mut n) = (i, 0);
                while j < tokens.len()
                    && j - i < config.batch_size
                    && n + tokens[j].len() <= RUN_TOKENS
                {
                    n += tokens[j].len();
                    j += 1;
                }
                let mut batch = LlamaBatch::new(n, i32::try_from(j - i).unwrap_or(i32::MAX));
                for (seq, t) in tokens[i..j].iter().enumerate() {
                    batch
                        .add_sequence(t, seq as i32, true)
                        .map_err(|e| EmbedError::BadResponse(format!("llama.cpp batch: {e}")))?;
                }
                ctx.clear_kv_cache();
                ctx.decode(&mut batch)
                    .map_err(|e| EmbedError::BadResponse(format!("llama.cpp decode: {e}")))?;
                for seq in 0..(j - i) {
                    let v = ctx.embeddings_seq_ith(seq as i32).map_err(|e| {
                        EmbedError::BadResponse(format!("llama.cpp embeddings: {e}"))
                    })?;
                    if v.len() != native {
                        return Err(EmbedError::BadResponse(format!(
                            "llama.cpp gave a {}-wide vector, not {native}",
                            v.len()
                        )));
                    }
                    out.vectors.push(cut(v, config.dims)?);
                }
                i = j;
            }
            Ok(())
        })();
        let _ = job.reply.send(result.map(|()| out));
    }
}

/// Most texts a background (document) call sends to a CPU session at once.
pub const CPU_DOCUMENT_CHUNK: usize = 4;

/// `embed` over `texts` in chunks of `size`, one call after another, as one result: vectors in input order and
/// truncated indexes shifted to the whole input. An error stops at once; an input index in it is shifted too.
fn in_chunks(
    texts: &[String],
    size: usize,
    embed: impl Fn(&[String]) -> Result<Embedded, EmbedError>,
) -> Result<Embedded, EmbedError> {
    let mut out = Embedded::default();
    for (n, chunk) in texts.chunks(size).enumerate() {
        let at = n * size;
        let got = embed(chunk).map_err(|e| match e {
            EmbedError::EmptyInput { index } => EmbedError::EmptyInput { index: at + index },
            EmbedError::InputTooLong { index } => EmbedError::InputTooLong { index: at + index },
            e => e,
        })?;
        out.vectors.extend(got.vectors);
        out.truncated
            .extend(got.truncated.into_iter().map(|i| at + i));
    }
    Ok(out)
}

/// The first `dims` values, L2-normalised.
fn cut(v: &[f32], dims: usize) -> Result<Vec<f32>, EmbedError> {
    let mut v = v[..dims.min(v.len())].to_vec();
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if !norm.is_finite() || norm == 0.0 {
        return Err(EmbedError::BadResponse(
            "the model gave a zero or non-finite vector".to_string(),
        ));
    }
    v.iter_mut().for_each(|x| *x /= norm);
    Ok(v)
}

impl Embedder for LlamaEmbedder {
    fn embed(&self, texts: &[String], kind: TextKind) -> Result<Embedded, EmbedError> {
        if texts.is_empty() {
            return Ok(Embedded::default());
        }
        if let Some(index) = texts.iter().position(|t| t.trim().is_empty()) {
            return Err(EmbedError::EmptyInput { index });
        }
        // On the CPU a batch of 16 takes about 5 s, the whole query bound: background batches go in chunks of
        // CPU_DOCUMENT_CHUNK, one at a time, so a search's query waits behind one chunk at most (embeddings-expert, s08).
        if kind == TextKind::Document
            && texts.len() > CPU_DOCUMENT_CHUNK
            && self.backend() == Backend::Cpu
        {
            return in_chunks(texts, CPU_DOCUMENT_CHUNK, |chunk| self.embed(chunk, kind));
        }
        let workers = self.workers()?;
        let prefix = match kind {
            TextKind::Query => &self.config.spec.query_prefix,
            TextKind::Document => &self.config.spec.document_prefix,
        };
        let (reply, answer) = mpsc::channel();
        let job = Job {
            texts: texts.iter().map(|t| format!("{prefix}{t}")).collect(),
            reply,
        };
        let stopped = || {
            if self.released.load(Ordering::SeqCst) {
                return self.released_error();
            }
            self.forget(&workers);
            EmbedError::BadResponse("the llama.cpp worker stopped".to_string())
        };
        // A clone of one queue, dropped once the job is in it, so a release that empties the queues ends the workers.
        let queue = {
            let jobs = workers.jobs.lock().unwrap_or_else(|p| p.into_inner());
            (!jobs.is_empty()).then(|| {
                jobs[self.next_worker.fetch_add(1, Ordering::Relaxed) % jobs.len()].clone()
            })
        };
        queue
            .ok_or_else(stopped)?
            .send(job)
            .map_err(|_| stopped())?;
        self.sent.fetch_add(texts.len(), Ordering::Relaxed);
        let wait = self
            .config
            .timeout_per_text
            .saturating_mul(u32::try_from(texts.len() + 1).unwrap_or(u32::MAX));
        let started = std::time::Instant::now();
        match answer.recv_timeout(wait) {
            Ok(result) => {
                if kind == TextKind::Document && result.is_ok() {
                    let mut rate = self.rate.lock().unwrap_or_else(|p| p.into_inner());
                    rate.0 += texts.len();
                    rate.1 += started.elapsed();
                }
                result
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Stop using these workers: the next call starts fresh ones. Their queues close now, so they finish
                // the jobs already in them and stop even while another call still holds these workers, and release
                // can join them without waiting on that call.
                workers.close();
                // Taken and moved under `late`, so a release that finds the threads gone finds them there.
                let mut late = self.late.lock().unwrap_or_else(|p| p.into_inner());
                late.extend(std::mem::take(
                    &mut *workers.threads.lock().unwrap_or_else(|p| p.into_inner()),
                ));
                drop(late);
                self.forget(&workers);
                Err(EmbedError::BadResponse(format!(
                    "llama.cpp did not answer within {} s; the model loads again on the next call",
                    wait.as_secs()
                )))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(stopped()),
        }
    }

    fn key(&self) -> ModelKey {
        self.key.clone()
    }

    /// One call per session at a time.
    fn concurrency_ceiling(&self) -> usize {
        self.config.sessions
    }

    /// The texts one run packs (`batch_size`, llama.cpp's n_seq_max).
    fn max_batch_inputs(&self) -> usize {
        self.config.batch_size.max(1)
    }

    fn max_input_tokens(&self) -> usize {
        self.config.spec.max_tokens
    }

    fn locality(&self) -> Locality {
        Locality::Local
    }

    /// Checks the model file (sha256) and loads it, embedding nothing.
    fn ready(&self) -> Result<(), EmbedError> {
        self.workers().map(drop)
    }

    /// None: a local model needs no credential (its file is checked by sha256 instead).
    fn credential(&self) -> Option<Credential> {
        None
    }

    fn adapter_id(&self) -> &'static str {
        "llamacpp.embedder"
    }
}

#[cfg(test)]
mod chunk_tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn chunks_go_one_at_a_time_and_join_in_order_with_shifted_indexes() {
        let texts: Vec<String> = (0..10).map(|i| i.to_string()).collect();
        let calls = RefCell::new(Vec::new());
        let got = in_chunks(&texts, CPU_DOCUMENT_CHUNK, |chunk| {
            calls.borrow_mut().push(chunk.len());
            Ok(Embedded {
                vectors: chunk
                    .iter()
                    .map(|t| vec![t.parse::<f32>().unwrap()])
                    .collect(),
                truncated: vec![0],
                tokens: None,
            })
        })
        .unwrap();
        assert_eq!(*calls.borrow(), [4, 4, 2]);
        let firsts: Vec<f32> = got.vectors.iter().map(|v| v[0]).collect();
        assert_eq!(firsts, (0..10).map(|i| i as f32).collect::<Vec<_>>());
        assert_eq!(got.truncated, [0, 4, 8]);
        let failed = in_chunks(&texts, 4, |chunk| {
            if chunk[0] == "4" {
                Err(EmbedError::InputTooLong { index: 1 })
            } else {
                Ok(Embedded {
                    vectors: vec![vec![0.0]; chunk.len()],
                    truncated: Vec::new(),
                    tokens: None,
                })
            }
        });
        assert_eq!(failed, Err(EmbedError::InputTooLong { index: 5 }));
    }
}

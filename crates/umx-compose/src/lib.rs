//! umx-compose: the composition root (blueprint 001, c-compose). The only crate that names adapters, and
//! the only crate that reads env (d-9).

mod identity;
pub use identity::{actor_from_sources, client_actor};

/// Finish client tool cleanup after its reply, without adding an adapter dependency to the binary.
pub fn finish_client_tools(by: std::time::Instant) -> bool {
    umx_adapter_system::finish_tools(by)
}

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use umx_adapter_azure_openai::{AzureAuth, AzureOpenAiConfig, AzureOpenAiEmbedder, CliTokenSource};
use umx_adapter_fs::{FsConfigFiles, FsGraphFiles, FsLocalState, FsSecrets};
use umx_adapter_llamacpp::{Execution, LlamaConfig, LlamaEmbedder, ModelSpec};
use umx_adapter_system::{SystemClock, SystemIds};
use umx_core::config::{
    self as umx_config, ConfigValue, Entries, INSTANCES, Resolved, SECRETS_FILE_NAME, Source,
};
use umx_core::ports::{Embedder, GraphDir, ServiceManager};
use umx_services::config::{Base, ConfigServices};
use umx_services::{Ports, Services};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Real,
    /// Fakes for every port. Test-only (`stub` feature); never in a release build.
    #[cfg(feature = "stub")]
    Stub,
}

/// Process-launch budgets supplied by the entry point, never read from user configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientBudgets {
    pub identity_work: Duration,
    pub identity_hard: Duration,
    pub service_tool: Duration,
    pub token_tool: Duration,
}

impl Default for ClientBudgets {
    fn default() -> Self {
        Self {
            identity_work: umx_adapter_system::agent_identity::WORK_TIMEOUT,
            identity_hard: umx_adapter_system::agent_identity::HARD_TIMEOUT,
            service_tool: umx_adapter_system::service::TOOL_TIMEOUT,
            token_tool: umx_adapter_azure_openai::CLI_TIMEOUT,
        }
    }
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Real => "real",
            #[cfg(feature = "stub")]
            Mode::Stub => "stub",
        }
    }
}

/// The environment as read once at start, for fs.secrets only. Debug prints no values.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct EnvSnapshot(BTreeMap<String, String>);

impl fmt::Debug for EnvSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EnvSnapshot(<{} redacted>)", self.0.len())
    }
}

/// Everything umx reads from the environment and its machine config file, read once at start.
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub client_budgets: ClientBudgets,
    /// `UMX_HOME`, else `$HOME/.umx`.
    pub home: PathBuf,
    /// Owner-only directory that holds the socket.
    pub run_dir: PathBuf,
    /// The daemon socket. CLI and daemon both take it from here, so they cannot disagree.
    pub socket: PathBuf,
    /// `UMX_OUTPUT=json|human` pins the output shape (req-63, D22a); `None` lets the CLI follow the reader.
    pub json: Option<bool>,
    /// Colour in human output (req-64): `Some(false)` when `NO_COLOR` is set or `TERM=dumb`, else `Some(true)`
    /// when `CLICOLOR_FORCE` is set and not `0`, else `None` (only on a terminal).
    pub color: Option<bool>,
    /// Folders graph files may live in (req-47). Absolute, normalised.
    pub roots: Vec<GraphDir>,
    /// Who writes when `--by` is not given: `UMX_AUTHOR`, else the OS user (workshop 007 rule 8).
    pub author: String,
    /// `UMX_GRAPH`: the graph alias commands use when `--graph` is not given (workshop 007 rule 6).
    pub graph: Option<String>,
    /// `daemon.idle_unload`, or `umx daemon start --idle-unload <dur>`: a graph unused this long unloads itself;
    /// 0 means never (req-03).
    pub idle_unload_ms: u64,
    /// The config inputs that are not files: catalogue env values, flags and host defaults (req-72).
    pub settings: Base,
    /// The config as resolved at start: the values this process runs with.
    pub started: Resolved,
    /// Set while graph folders are closed because the config is invalid: the key or file to name.
    pub config_closed: Option<String>,
    /// `PATH`'s absolute folders, where `az` and `azd` are looked up once when the embedders are built.
    pub search_path: Vec<PathBuf>,
    secrets_env: EnvSnapshot,
    /// Where `umx daemon install` puts the service's file (req-86): `~/Library/LaunchAgents` on macOS,
    /// `$XDG_CONFIG_HOME/systemd/user` (else `~/.config/systemd/user`) on Linux. [`Config::at`] puts it under the
    /// umx home, so a test never touches the real one.
    pub service_dir: PathBuf,
    /// Where `umx models fetch` puts a model and the local embedder reads it (ws004 s14): `$XDG_CACHE_HOME/umx/models`
    /// when that is absolute, else `$HOME/.cache/umx/models`. [`Config::at`] puts it under the umx home, so a test
    /// never touches the real one.
    pub models_dir: PathBuf,
    /// Where `umx doctor` looks for the installed /umx agent skill (F77): `$HOME/.agents/skills/umx` when `HOME` is
    /// absolute, else none (no skill row). [`Config::at`] puts it under the umx home, so a test never reads the
    /// real one.
    pub skill_dir: Option<PathBuf>,
    /// flowspace3's config folder, which `umx config import --from flowspace3` reads `config.toml` from (ws004 s27):
    /// `$FS3_CONFIG_DIR` when absolute, else `$HOME/.config/flowspace3`, as flowspace3 finds it. [`Config::at`] puts
    /// it under the umx home, so a test never reads the real one.
    pub flowspace3_dir: PathBuf,
    /// The launchctl or systemctl `umx daemon install` runs: `UMX_SERVICE_TOOL` if set, else `/bin/launchctl` on
    /// macOS or `/usr/bin/systemctl` (else `/bin/systemctl`) on Linux; never a PATH lookup.
    pub service_tool: String,
    /// True when `UMX_SERVICE_TOOL` is set and `UMX_HOME` is not `$HOME/.umx`: the service then uses a sandbox
    /// label (`dev.umx.daemon.sandbox`, `umx-sandbox.service`), so an override can never rewrite the real service.
    pub service_sandbox: bool,
    /// This process's effective uid, read once (the daemon's owner checks, launchd's `gui/<uid>` domain).
    pub uid: Option<u32>,
}

/// The service tool when `UMX_SERVICE_TOOL` is not set.
pub fn default_service_tool() -> &'static str {
    if cfg!(target_os = "macos") {
        "/bin/launchctl"
    } else if !std::path::Path::new("/usr/bin/systemctl").exists()
        && std::path::Path::new("/bin/systemctl").exists()
    {
        "/bin/systemctl"
    } else {
        // By absolute path, so PATH order never picks the tool; a missing one fails with its name.
        "/usr/bin/systemctl"
    }
}

fn clean_roots<S: AsRef<str>>(roots: impl IntoIterator<Item = S>) -> Vec<GraphDir> {
    let mut roots: Vec<GraphDir> = roots
        .into_iter()
        .filter_map(|r| GraphDir::parse(r.as_ref().trim()).ok())
        .collect();
    roots.sort();
    roots.dedup();
    roots
}

fn roots_value(roots: &[GraphDir]) -> ConfigValue {
    ConfigValue::List(roots.iter().map(|r| r.as_str().to_owned()).collect())
}

/// Where `umx ui` looks for the graph view when neither UMX_UI_DIR nor `ui.dir` says: apps/ui in the source tree this
/// umx was built from (architect ruling, ws009 s2).
pub fn default_ui_dir() -> String {
    let compose = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    compose
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap_or(compose)
        .join("apps")
        .join("ui")
        .display()
        .to_string()
}

/// The default for `--idle-unload`: 30 minutes.
pub const IDLE_UNLOAD_MS: u64 = 30 * 60 * 1000;

impl Config {
    /// A config under `home` with no graph roots (add them with `roots`).
    pub fn at(home: PathBuf, json: Option<bool>) -> Self {
        let run_dir = home.join("run");
        let socket = run_dir.join("umx.sock");
        let service_dir = home.join("service");
        let models_dir = home.join("models");
        let skill_dir = Some(home.join("skills").join("umx"));
        let flowspace3_dir = home.join("flowspace3");
        let settings = Base {
            defaults: Entries::from([("ui.dir".to_owned(), ConfigValue::Text(default_ui_dir()))]),
            ..Base::default()
        };
        Config {
            client_budgets: ClientBudgets::default(),
            service_dir,
            models_dir,
            skill_dir,
            flowspace3_dir,
            service_tool: default_service_tool().to_owned(),
            service_sandbox: false,
            uid: umx_adapter_system::effective_uid(),
            home,
            run_dir,
            socket,
            json,
            color: None,
            roots: Vec::new(),
            author: "unknown".to_owned(),
            graph: None,
            idle_unload_ms: IDLE_UNLOAD_MS,
            started: umx_config::resolve(&settings),
            settings,
            config_closed: None,
            search_path: Vec::new(),
            secrets_env: EnvSnapshot::default(),
        }
    }

    /// The folders `az` and `azd` are looked up in (from_env takes `PATH`). Relative folders are dropped.
    pub fn search_path(mut self, dirs: impl IntoIterator<Item = PathBuf>) -> Self {
        self.search_path = dirs.into_iter().filter(|d| d.is_absolute()).collect();
        self
    }

    /// Sets the graph roots, dropping entries that are not absolute folders, sorted and deduplicated. They count
    /// as the host default in `config show`.
    pub fn roots<S: AsRef<str>>(mut self, roots: impl IntoIterator<Item = S>) -> Self {
        self.roots = clean_roots(roots);
        self.settings
            .defaults
            .insert("roots".to_owned(), roots_value(&self.roots));
        self.started = umx_config::resolve(&self.settings);
        self
    }

    /// `umx daemon start --idle-unload <dur>`: beats the config file.
    pub fn idle_unload_flag(mut self, ms: u64) -> Self {
        self.idle_unload_ms = ms;
        let text = if ms == 0 {
            "0".to_owned()
        } else {
            format!("{ms}ms")
        };
        self.settings.flags.insert(
            "daemon.idle_unload".to_owned(),
            ConfigValue::Text(text.clone()),
        );
        if let Some(s) = self
            .started
            .settings
            .iter_mut()
            .find(|s| s.key == "daemon.idle_unload")
        {
            (s.value, s.source) = (Some(ConfigValue::Text(text)), Source::Flag);
        }
        self
    }

    /// The environment fs.secrets looks secrets up in (from_env takes the process environment).
    pub fn secrets_env(mut self, env: BTreeMap<String, String>) -> Self {
        self.secrets_env = EnvSnapshot(env);
        self
    }

    /// The machine config file, `$UMX_HOME/config.toml`.
    pub fn config_file(&self) -> PathBuf {
        self.home.join(umx_config::FILE_NAME)
    }

    /// This config with the config file read again (roots, settings in force, whether folders are closed), env and
    /// flags kept: what a process started now would run with. `umx doctor` checks against it, again after `--fix`
    /// repairs the file.
    pub fn reread(&self) -> Config {
        let startup = ConfigServices::startup(
            &FsConfigFiles::new(self.home.clone(), Vec::new()),
            &self.settings,
        );
        Config {
            roots: clean_roots(startup.resolved.roots()),
            idle_unload_ms: startup.resolved.idle_unload_ms(),
            started: startup.resolved,
            config_closed: startup.closed,
            ..self.clone()
        }
    }

    /// The one env and config loader (d-9, r-env-in-compose). Graph roots: `UMX_ROOTS` (colon-separated absolute
    /// folders), else `roots` in `$UMX_HOME/config.toml`, else `HOME` and the per-user temp folder. Shared folders
    /// such as `/tmp` are not defaults: other users can write there. The fs adapter resolves symlinks in roots and
    /// graph folders before it checks them. An invalid roots value, or a machine config file that is refused or
    /// does not parse, leaves no roots at all (fail closed) and sets `config_closed`.
    #[expect(clippy::disallowed_methods, reason = "the one env loader (d-9)")]
    pub fn from_env() -> Self {
        let home = std::env::var_os("UMX_HOME")
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".umx")))
            .unwrap_or_else(|| PathBuf::from(".umx"));
        let json = match std::env::var("UMX_OUTPUT").as_deref() {
            Ok("json") => Some(true),
            Ok("human") => Some(false),
            _ => None,
        };
        let default_roots = clean_roots(
            std::env::var("HOME")
                .ok()
                .into_iter()
                .chain([std::env::temp_dir().to_string_lossy().into_owned()]),
        );
        let env: BTreeMap<String, String> = umx_services::config::env_names()
            .into_iter()
            .filter_map(|name| {
                let v = std::env::var(name).ok().filter(|v| !v.trim().is_empty())?;
                Some((name.to_owned(), v))
            })
            .collect();
        let secrets_env: BTreeMap<String, String> = std::env::vars_os()
            .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
            .collect();
        let user = std::env::var("USER").ok();
        let logname = std::env::var("LOGNAME").ok();
        let author = std::env::var("UMX_AUTHOR")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| umx_core::provenance::login_user(user.as_deref(), logname.as_deref()))
            .unwrap_or_else(|| "unknown".to_owned());
        let graph = std::env::var("UMX_GRAPH")
            .ok()
            .filter(|g| !g.trim().is_empty());
        let absolute = |k: &str| {
            std::env::var_os(k)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
        };
        let service_dir = if cfg!(target_os = "macos") {
            absolute("HOME").map(|h| h.join("Library/LaunchAgents"))
        } else {
            absolute("XDG_CONFIG_HOME")
                .or_else(|| absolute("HOME").map(|h| h.join(".config")))
                .map(|c| c.join("systemd/user"))
        };
        let models_dir = absolute("XDG_CACHE_HOME")
            .or_else(|| absolute("HOME").map(|h| h.join(".cache")))
            .map(|c| c.join("umx").join("models"))
            .unwrap_or_else(|| home.join("models"));
        let flowspace3_dir = absolute("FS3_CONFIG_DIR")
            .or_else(|| absolute("HOME").map(|h| h.join(".config").join("flowspace3")))
            .unwrap_or_else(|| home.join("flowspace3"));
        let set = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
        let color = if set("NO_COLOR") || std::env::var_os("TERM").is_some_and(|t| t == "dumb") {
            Some(false)
        } else if std::env::var_os("CLICOLOR_FORCE").is_some_and(|v| !v.is_empty() && v != "0") {
            Some(true)
        } else {
            None
        };
        let settings = Base {
            defaults: Entries::from([
                ("roots".to_owned(), roots_value(&default_roots)),
                ("ui.dir".to_owned(), ConfigValue::Text(default_ui_dir())),
            ]),
            env,
            ..Base::default()
        };
        let search_path: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        let startup =
            ConfigServices::startup(&FsConfigFiles::new(home.clone(), Vec::new()), &settings);
        let tool = std::env::var("UMX_SERVICE_TOOL")
            .ok()
            .filter(|t| !t.trim().is_empty());
        let default_home = absolute("HOME").map(|h| h.join(".umx"));
        let service_sandbox = tool.is_some() && default_home.as_ref() != Some(&home);
        let fallback_dir = home.join("service");
        Config {
            author,
            graph,
            color,
            roots: clean_roots(startup.resolved.roots()),
            idle_unload_ms: startup.resolved.idle_unload_ms(),
            settings,
            started: startup.resolved,
            config_closed: startup.closed,
            service_dir: service_dir.unwrap_or(fallback_dir),
            models_dir,
            skill_dir: absolute("HOME").map(|h| h.join(".agents").join("skills").join("umx")),
            flowspace3_dir,
            service_tool: tool.unwrap_or_else(|| default_service_tool().to_owned()),
            service_sandbox,
            ..Config::at(home, json)
                .secrets_env(secrets_env)
                .search_path(search_path)
        }
    }
}

/// (port id, adapter id) for what `services` and `config` were built with, read from the adapters themselves
/// (E-1), so r-wiring-matches-docs and `umx daemon status` check what was wired, not a hand-kept table.
/// The config files port is wired into both (the graph services read the repo Graphfile through it, ws002 s17):
/// one row.
pub fn describe(services: &Services, config: &ConfigServices) -> Vec<(&'static str, &'static str)> {
    let mut rows = services.adapters();
    rows.extend(config.adapters());
    rows.sort_unstable();
    rows.dedup();
    rows
}

/// The stub's vector cache root.
#[cfg(feature = "stub")]
const STUB_VECTOR_CACHE: &str = "/umx-vector-cache";

/// Where secured graphs' vector shards go: `$UMX_HOME/vectors`, outside every graph folder (req-46).
fn vector_cache_root(config: &Config) -> PathBuf {
    config.home.join("vectors")
}

fn as_graph_dir(path: &std::path::Path) -> GraphDir {
    path.to_str()
        .and_then(|p| GraphDir::parse(p).ok())
        .unwrap_or_else(|| GraphDir::parse("/").expect("root"))
}

/// Builds the services for a mode, with their own config service for `root:` sources. A caller that also runs
/// config commands uses [`compose_with_config`], so both read one machine config.
pub fn compose(mode: Mode, config: &Config) -> Services {
    compose_with_config(mode, config).0
}

/// Builds the services and the config service for a mode, as one composition: graph writes look up `root:` sources
/// through the same config service `umx root add` and `root remove` write through. In stub mode that is one fake
/// machine file, not one per service.
pub fn compose_with_config(mode: Mode, config: &Config) -> (Services, Arc<ConfigServices>) {
    let settings = Arc::new(compose_config(mode, config));
    let ports = match mode {
        Mode::Real => Ports {
            files: Arc::new(FsGraphFiles::new(config.roots.clone())),
            // Its own root, $UMX_HOME/vectors, made 0700 and owner-checked; graph roots are not widened. If the
            // folder is not trusted the cache refuses every call, so no secured vector is written anywhere.
            vector_cache: Arc::new(FsGraphFiles::vector_cache(&vector_cache_root(config)).0),
            vector_cache_root: as_graph_dir(&vector_cache_root(config)),
            clock: Arc::new(SystemClock),
            ids: Arc::new(SystemIds::new()),
            state: Arc::new(FsLocalState::new(config.home.clone())),
            scopes: Arc::new(
                FsLocalState::named(config.home.clone(), "", umx_core::scopes::SCOPES_FILE)
                    .with_absent_ok(true),
            ),
            allows: Arc::new(FsLocalState::named(
                config.home.clone(),
                umx_core::secret_allows::ALLOWS_DIR,
                umx_core::secret_allows::ALLOWS_FILE,
            )),
            // F139: this machine's lint terms, its own owner-only folder; absent is no term scan.
            lint_terms: Arc::new(
                FsLocalState::named(
                    config.home.clone(),
                    umx_core::lint_terms::LINT_DIR,
                    umx_core::lint_terms::LINT_FILE,
                )
                .with_absent_ok(true),
            ),
            // This machine's graph registry (F72), its own owner-only folder; absent is an empty registry.
            registry: Arc::new(
                FsLocalState::named(
                    config.home.clone(),
                    umx_core::registry::REGISTRY_DIR,
                    umx_core::registry::REGISTRY_FILE,
                )
                .with_absent_ok(true),
            ),
            sources: source_reader(mode, config),
        },
        #[cfg(feature = "stub")]
        Mode::Stub => Ports {
            files: Arc::new(umx_fakes::FakeGraphFiles::new(
                GraphDir::parse("/").expect("root"),
            )),
            vector_cache: Arc::new(umx_fakes::FakeGraphFiles::new(
                GraphDir::parse(STUB_VECTOR_CACHE).expect("root"),
            )),
            vector_cache_root: GraphDir::parse(STUB_VECTOR_CACHE).expect("root"),
            clock: Arc::new(umx_fakes::FakeClock::default()),
            ids: Arc::new(umx_fakes::FakeIds::new()),
            state: Arc::new(umx_fakes::FakeLocalState::new()),
            scopes: Arc::new(umx_fakes::FakeLocalState::new().with_absent_ok(true)),
            allows: Arc::new(umx_fakes::FakeLocalState::new()),
            lint_terms: Arc::new(umx_fakes::FakeLocalState::new().with_absent_ok(true)),
            registry: Arc::new(umx_fakes::FakeLocalState::new().with_absent_ok(true)),
            sources: source_reader(mode, config),
        },
    };
    let roots = match mode {
        Mode::Real => config.roots.clone(),
        #[cfg(feature = "stub")]
        Mode::Stub => vec![GraphDir::parse("/").expect("root")],
    };
    let services = Services::new(ports)
        .with_roots(roots)
        .with_source_roots(Arc::clone(&settings) as Arc<dyn umx_services::SourceRoots>)
        // The repo Graphfile (ws002 s17), through the one config files adapter, in both modes (architect).
        .with_config_files(settings.files());
    let services = match mode {
        // Real mode wires the instance embedder.active names, from [`embedders`]; none until then.
        Mode::Real => services,
        #[cfg(feature = "stub")]
        Mode::Stub => services.with_embedder(Arc::new(stub_fake(STUB_KEY, true))),
    };
    let services = services.with_model_fetcher(model_fetcher(mode));
    let services = match service_manager(mode, config) {
        Ok(manager) => services.with_service_manager(manager),
        Err(why) => services.without_service_manager(why),
    };
    (services, settings)
}

/// What every writer in the models folder gets (architect ruling): the folder's one lock and the free space.
/// The model fetcher and the local embedder's CPU file both take these, so they never write at once.
#[derive(Clone, Copy, Debug)]
pub struct ModelsHooks {
    pub lock: Option<fn(&std::path::Path) -> std::io::Result<std::fs::File>>,
    pub available: Option<fn(&std::path::Path) -> std::io::Result<u64>>,
}

/// Lists a folder of an installed skill, at most the given number of entries: each entry's name and whether it is
/// a folder.
pub type SkillFolder = fn(&std::path::Path, usize) -> std::io::Result<Vec<(String, bool)>>;

/// How `umx doctor` reads an installed /umx skill (F94, architect ruling 2026-11-12): the fs adapter's checked
/// folder listing and capped file read (O_NOFOLLOW, O_NONBLOCK, checked on the handle), handed to the daemon as
/// plain functions, so umx-daemon needs no libc and no adapter edge.
/// Looks at a skill's root without following it: Ok(Some(target)) for a link, Ok(None) otherwise, Err when nothing
/// is there.
pub type SkillRoot = fn(&std::path::Path) -> std::io::Result<Option<std::path::PathBuf>>;

#[derive(Clone, Copy, Debug)]
pub struct SkillReads {
    /// The root: whether it is a link, and its target (F111).
    pub root: SkillRoot,
    /// A folder's entries, at most the budget: name and whether it is a folder; Err for a link, a special file, a
    /// swap, or more entries than the budget.
    pub folder: SkillFolder,
    /// A regular file's bytes, at most the cap; None otherwise.
    pub file: fn(&std::path::Path, u64) -> Option<Vec<u8>>,
}

/// The real skill reads: `umx_adapter_fs::skill_root`, `skill_folder` and `skill_file`.
pub fn skill_reads() -> SkillReads {
    SkillReads {
        root: umx_adapter_fs::skill_root,
        folder: umx_adapter_fs::skill_folder,
        file: umx_adapter_fs::skill_file,
    }
}

/// The real hooks: `umx_adapter_fs::models_folder_lock` and `available_bytes`.
pub fn models_hooks() -> ModelsHooks {
    ModelsHooks {
        lock: Some(umx_adapter_fs::models_folder_lock),
        available: Some(umx_adapter_fs::available_bytes),
    }
}

/// The model fetcher (req-74): Hugging Face over https in real mode, the fake (which serves nothing) in stub mode.
fn model_fetcher(mode: Mode) -> Arc<dyn umx_core::ports::ModelFetcher> {
    match mode {
        Mode::Real => Arc::new(hf_model_fetcher(models_hooks())),
        #[cfg(feature = "stub")]
        Mode::Stub => Arc::new(umx_fakes::FakeModelFetcher::new()),
    }
}

/// The Hugging Face fetcher with these hooks.
pub fn hf_model_fetcher(hooks: ModelsHooks) -> umx_adapter_model_fetch::HfModelFetcher {
    umx_adapter_model_fetch::HfModelFetcher::new(hooks.lock, hooks.available)
}

/// Bytes free for this user on the file system holding `path`, or its nearest existing ancestor.
pub fn available_bytes(path: &std::path::Path) -> std::io::Result<u64> {
    let mut at = Some(path);
    while let Some(p) = at {
        if p.exists() {
            return umx_adapter_fs::available_bytes(p);
        }
        at = p.parent();
    }
    umx_adapter_fs::available_bytes(std::path::Path::new("/"))
}

/// The per-user service manager for this OS (req-86): launchd or systemd, run as `config.service_tool`; the fake in
/// stub mode; none elsewhere (n-13).
fn service_manager(mode: Mode, config: &Config) -> Result<Arc<dyn ServiceManager>, &'static str> {
    use umx_adapter_system::service::{Launchd, Systemd};
    let (label, unit) = if config.service_sandbox {
        (Launchd::SANDBOX_LABEL, Systemd::SANDBOX_UNIT)
    } else {
        (Launchd::LABEL, Systemd::UNIT)
    };
    let tool = config.service_tool.clone();
    let dir = config.service_dir.clone();
    match mode {
        Mode::Real if cfg!(target_os = "macos") => match config.uid {
            Some(uid) => Ok(Arc::new(
                Launchd::new(dir, tool, uid, label)
                    .with_timeout(config.client_budgets.service_tool),
            )),
            None => {
                Err("this process's user id could not be read, so its launchd domain is unknown")
            }
        },
        Mode::Real if cfg!(target_os = "linux") => Ok(Arc::new(
            Systemd::new(dir, tool, unit).with_timeout(config.client_budgets.service_tool),
        )),
        Mode::Real => Err("umx runs as a service only on macOS (launchd) and Linux (systemd)"),
        #[cfg(feature = "stub")]
        Mode::Stub => Ok(Arc::new(umx_fakes::FakeServiceManager::new())),
    }
}

/// (port id, adapter id) for the ports only the client process wires, never part of [`Services`] (so the daemon
/// holds no process-launching capability it does not use; architect B1). The compose describe test merges
/// these with [`describe`], so r-wiring-matches-docs covers them.
pub fn client_adapters(mode: Mode, config: &Config) -> Vec<(&'static str, &'static str)> {
    let mut rows = vec![("ui-host", ui_host(mode, config).adapter_id())];
    rows.extend(identity::client_adapters(mode, &config.search_path));
    rows
}

/// What `umx ui` runs the graph view with (req-104), built only in the client that runs `umx ui`: system.ui-host in
/// real mode, the fake in stub mode. The browser is opened without the link in argv: osascript on macOS, a redirect
/// page in the run folder and xdg-open (found once on PATH) on Linux, and nothing elsewhere.
pub fn ui_host(mode: Mode, config: &Config) -> Arc<dyn umx_core::ports::UiHost> {
    ui_host_opening_with(mode, config, None)
}

/// [`ui_host`], with `opener` (a program given the AppleScript `open location "<url>"` on stdin) in place of the
/// system's. Only umx's test driver passes one (ws009 M1b2 evidence), so a drive never opens the person's browser.
pub fn ui_host_opening_with(
    mode: Mode,
    config: &Config,
    opener: Option<PathBuf>,
) -> Arc<dyn umx_core::ports::UiHost> {
    use umx_adapter_system::ui_host::{Opener, SystemUiHost};
    match mode {
        Mode::Real => {
            let opener = if let Some(program) = opener {
                Opener::AppleScript(program)
            } else if cfg!(target_os = "macos") {
                let osascript = PathBuf::from("/usr/bin/osascript");
                if osascript.is_file() {
                    Opener::AppleScript(osascript)
                } else {
                    Opener::None("/usr/bin/osascript is missing")
                }
            } else if cfg!(target_os = "linux") {
                match config
                    .search_path
                    .iter()
                    .map(|d| d.join("xdg-open"))
                    .find(|p| p.is_file())
                {
                    Some(program) => Opener::RedirectPage {
                        program,
                        dir: config.run_dir.clone(),
                    },
                    None => Opener::None("no xdg-open on PATH"),
                }
            } else {
                Opener::None("umx opens a browser only on macOS and Linux")
            };
            Arc::new(
                SystemUiHost::new(opener, SystemUiHost::default_kill(), config.uid)
                    .with_run_dir(config.run_dir.clone()),
            )
        }
        #[cfg(feature = "stub")]
        Mode::Stub => {
            let _ = opener;
            Arc::new(umx_fakes::FakeUiHost::new())
        }
    }
}

/// Where `umx ui` records the view it runs (ws009 M1b2): `$UMX_HOME/run/ui.json` through fs.local-state (owner-only,
/// replaced by rename, never through a symlink); absent means none runs. The fake in stub mode.
pub fn ui_state(mode: Mode, config: &Config) -> Arc<dyn umx_core::ports::LocalState> {
    match mode {
        Mode::Real => Arc::new(
            FsLocalState::named(config.home.clone(), "run", "ui.json").with_absent_ok(true),
        ),
        #[cfg(feature = "stub")]
        Mode::Stub => {
            let _ = config;
            Arc::new(umx_fakes::FakeLocalState::new().with_absent_ok(true))
        }
    }
}

/// The source reader (req-47, ws006 S1): fs.source-reader over the graph roots, or in stub mode the fake with every
/// folder present.
fn source_reader(mode: Mode, config: &Config) -> Arc<dyn umx_core::ports::SourceReader> {
    match mode {
        Mode::Real => Arc::new(umx_adapter_fs::FsSourceReader::new(config.roots.clone())),
        #[cfg(feature = "stub")]
        Mode::Stub => {
            let _ = config;
            Arc::new(umx_fakes::FakeSourceReader::open(
                GraphDir::parse("/").expect("root"),
            ))
        }
    }
}

/// Builds the config services for a mode (req-72): its own service over the config-files and secrets ports, with
/// the source reader that checks source roots.
pub fn compose_config(mode: Mode, config: &Config) -> ConfigServices {
    compose_config_files(mode, config).with_sources(source_reader(mode, config))
}

fn compose_config_files(mode: Mode, config: &Config) -> ConfigServices {
    let secrets_path = config.home.join(SECRETS_FILE_NAME);
    let as_dir = |p: &std::path::Path| {
        p.to_str()
            .and_then(|p| GraphDir::parse(p).ok())
            .unwrap_or_else(|| GraphDir::parse("/").expect("root"))
    };
    match mode {
        Mode::Real => ConfigServices::new(
            Arc::new(
                FsConfigFiles::new(config.home.clone(), config.roots.clone())
                    .with_flowspace3(config.flowspace3_dir.clone()),
            ),
            Arc::new(FsSecrets::new(
                config.secrets_env.0.clone(),
                secrets_path.clone(),
            )),
            config.settings.clone(),
            as_dir(&secrets_path),
            config.started.clone(),
        ),
        #[cfg(feature = "stub")]
        Mode::Stub => ConfigServices::new(
            Arc::new(stub_config_files()),
            Arc::new(umx_fakes::FakeSecrets::new(BTreeMap::new())),
            config.settings.clone(),
            GraphDir::parse("/fake-home/secrets.env").expect("path"),
            config.started.clone(),
        ),
    }
}

/// The stub's own embedder instance, active in the stub's machine config (fakes for every port).
#[cfg(feature = "stub")]
pub const STUB_INSTANCE: &str = "fake";

#[cfg(feature = "stub")]
const STUB_KEY: &str = "fake-embedder@64";

/// The stub's config files: a machine config that defines [`STUB_INSTANCE`] (kind local, the fake's model and dims)
/// and makes it active, so stub mode embeds with the fake through the same config path as real mode.
#[cfg(feature = "stub")]
fn stub_config_files() -> umx_fakes::FakeConfigFiles {
    use umx_core::ports::ConfigFiles;
    let files = umx_fakes::FakeConfigFiles::new(
        GraphDir::parse("/fake-home").expect("home"),
        GraphDir::parse("/").expect("root"),
    );
    let key = umx_core::ports::ModelKey::parse(STUB_KEY).expect("a model key");
    let text = |v: &str| ConfigValue::Text(v.to_owned());
    let entries: Entries = [
        ("embedder.active".to_owned(), text(STUB_INSTANCE)),
        (format!("{INSTANCES}.{STUB_INSTANCE}.kind"), text("local")),
        (
            format!("{INSTANCES}.{STUB_INSTANCE}.model"),
            text(key.model()),
        ),
        (
            format!("{INSTANCES}.{STUB_INSTANCE}.dims"),
            ConfigValue::Int(key.dims() as i64),
        ),
    ]
    .into_iter()
    .collect();
    files
        .write(
            &umx_core::ports::ConfigFile::Machine,
            &umx_config::encode(&entries),
        )
        .expect("the fake config files take a write");
    files
}

#[cfg(feature = "stub")]
fn stub_fake(key: &str, local: bool) -> umx_fakes::FakeEmbedder {
    let key = umx_core::ports::ModelKey::parse(key).expect("a model key");
    let locality = if local {
        umx_core::ports::Locality::Local
    } else {
        umx_core::ports::Locality::Hosted
    };
    umx_fakes::FakeEmbedder::new(key).with_locality(locality)
}

/// An instance name and its built adapter.
pub type Chosen = (String, Arc<dyn Embedder>);

/// One configured embedder instance (`[embedders.<name>]`), built once at start.
pub struct Instance {
    pub name: String,
    pub kind: String,
    /// The adapter, or why it could not be built: keys and env var NAMES only, never a value.
    pub built: Result<Arc<dyn Embedder>, String>,
    /// The same adapter when it is the local llama.cpp one, for what only it can say (backend, slow warning).
    pub local: Option<Arc<LlamaEmbedder>>,
    /// How fast background embedding may send to it, in estimated tokens a minute (`tokens_per_minute`, ws004 s25):
    /// the configured value, else [`AZURE_TOKENS_PER_MIN`] for azure_openai; None (unpaced) for a local instance
    /// that sets none.
    pub tokens_per_minute: Option<u64>,
    /// A hosted instance's endpoint host, `host[:port]` only (D83, security W2), parsed here once; None for a local
    /// instance or an endpoint that cannot be shown safely.
    pub host: Option<umx_core::problem::Host>,
}

/// An azure_openai instance's default pace: umx shares the deployment's quota with other tools (Flowspace measured
/// 6.85M tokens a minute and uses about 4M), so it takes a modest share unless the machine config says otherwise.
pub const AZURE_TOKENS_PER_MIN: u64 = 1_500_000;

impl Instance {
    /// For a local instance: what runs its model (`metal`, `cuda` or `cpu`) and, on the CPU only, the `local_embedder_slow`
    /// reason. Asks llama.cpp which backend it has (no model is loaded and nothing is sent).
    pub fn local_backend(&self) -> Option<(&'static str, bool)> {
        self.local
            .as_ref()
            .map(|l| (l.backend().as_str(), l.slow_warning().is_some()))
    }

    /// For a local instance built with cuda that fell back to the CPU: why (no CUDA device), for doctor's embedder row.
    pub fn local_fallback(&self) -> Option<&'static str> {
        self.local.as_ref().and_then(|l| l.fallback_reason())
    }

    /// For a local instance on the CPU at or above the CPU floor: about how many texts a second it embeds (measured,
    /// or estimated from its threads), for doctor's `doctor_local_embedder_cpu` note.
    pub fn local_cpu_rate(&self) -> Option<f64> {
        self.local
            .as_ref()
            .filter(|l| l.cpu_note().is_some())
            .and_then(|l| l.cpu_texts_per_s())
    }

    /// For a local instance: frees its model for good (its sessions finish their job, free their contexts and model,
    /// and it never loads again). The daemon calls it after its last write, right before it exits, because exit runs
    /// ggml's static Metal teardown, which aborts while a model is loaded (F207). Idempotent; nothing for a hosted one.
    pub fn release_local(&self) {
        if let Some(l) = &self.local {
            l.release();
        }
    }
}

/// Every named embedder instance, built once when the daemon starts and never during a query. Building does no
/// I/O beyond reading a named api key through the Secrets port: no token is fetched, no `az` runs and nothing is
/// sent (an Entra token is fetched on first use). An instance that cannot be built is kept with its reason and
/// does not stop the daemon from starting.
pub struct Embedders {
    instances: Vec<Instance>,
}

impl Embedders {
    /// Instances built elsewhere, for tests that need an embedder that blocks or counts. Real-mode rules.
    pub fn from_instances(instances: Vec<Instance>) -> Embedders {
        Embedders { instances }
    }

    /// The name `embedder.active` gives in `resolved`, or `None` for none.
    pub fn active_name(&self, resolved: &Resolved) -> Option<String> {
        resolved
            .value("embedder.active")
            .and_then(ConfigValue::as_text)
            .filter(|a| *a != "none")
            .map(str::to_owned)
    }

    /// The instance `embedder.active` names in `resolved` (it applies now; only the instances are fixed at start):
    /// `Ok(None)` when it is none, `Err` with a reason when it names no built instance.
    pub fn active(&self, resolved: &Resolved) -> Result<Option<Chosen>, String> {
        let Some(name) = self.active_name(resolved) else {
            return Ok(None);
        };
        let instance = self.get(&name).ok_or_else(|| {
            format!("embedder.active names {name}, which was not defined when the daemon started")
        })?;
        match &instance.built {
            Ok(embedder) => Ok(Some((name, Arc::clone(embedder)))),
            Err(why) => Err(why.clone()),
        }
    }

    pub fn get(&self, name: &str) -> Option<&Instance> {
        self.instances.iter().find(|i| i.name == name)
    }

    pub fn instances(&self) -> &[Instance] {
        &self.instances
    }

    /// The machine config's floors for each built instance's model (`search_floor`, `similar_floor`; ws004 s26).
    /// Instances go in name order, so of two instances of one model that set a floor, the first name's is used.
    /// Machine only: the resolver drops these keys from a repo layer.
    pub fn floors(&self, resolved: &Resolved) -> umx_core::similarity::Floors {
        let mut floors = umx_core::similarity::Floors::default();
        let mut instances: Vec<&Instance> = self.instances.iter().collect();
        instances.sort_by(|a, b| a.name.cmp(&b.name));
        for i in instances {
            let Ok(embedder) = &i.built else { continue };
            let number = |leaf: &str| {
                resolved
                    .value(&format!("{INSTANCES}.{}.{leaf}", i.name))
                    .and_then(ConfigValue::as_number)
                    .map(|n| n as f32)
            };
            floors.set(
                &embedder.key(),
                number("search_floor"),
                number("similar_floor"),
            );
        }
        floors
    }

    /// Each built instance's related-check cap (req-123, embeddings-expert ws004 s28): what it embeds in about 2 s,
    /// so [`umx_services::related::RELATED_CAP_CPU`] for a local model on the CPU and
    /// [`umx_services::related::RELATED_CAP`] for Metal, CUDA or a hosted instance.
    pub fn related_caps(&self) -> std::collections::BTreeMap<String, usize> {
        self.instances
            .iter()
            .filter(|i| i.built.is_ok())
            .map(|i| {
                let cpu = i.local_backend().is_some_and(|(b, _)| b == "cpu");
                let cap = if cpu {
                    umx_services::related::RELATED_CAP_CPU
                } else {
                    umx_services::related::RELATED_CAP
                };
                (i.name.clone(), cap)
            })
            .collect()
    }

    /// Each built instance's background pace in tokens a minute (ws004 s25); an unpaced one is left out.
    pub fn pace(&self) -> std::collections::BTreeMap<String, u64> {
        self.instances
            .iter()
            .filter(|i| i.built.is_ok())
            .filter_map(|i| Some((i.name.clone(), i.tokens_per_minute?)))
            .collect()
    }

    /// Each built hosted instance's endpoint host (D83): what a lowered policy's warning names, never more.
    pub fn hosts(&self) -> std::collections::BTreeMap<String, umx_core::problem::Host> {
        self.instances
            .iter()
            .filter(|i| i.built.is_ok())
            .filter_map(|i| Some((i.name.clone(), i.host.clone()?)))
            .collect()
    }
}

/// The first `program` in `dirs` that is an executable file, so `az` runs by absolute path and the path can be
/// reported. Only stats files.
fn which(program: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    dirs.iter().map(|d| d.join(program)).find(|p| {
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

fn program(name: &str, dirs: &[PathBuf]) -> String {
    which(name, dirs).map_or_else(|| name.to_owned(), |p| p.display().to_string())
}

/// `az` and `azd` as they will be run: absolute paths when found in `search_path`, else the bare names (which
/// then fail with "not found on PATH").
pub fn az_programs(config: &Config) -> (String, String) {
    (
        program("az", &config.search_path),
        program("azd", &config.search_path),
    )
}

/// Builds every named embedder instance from the machine config (req-72, req-73). Stub mode builds a fake for each
/// instance (hosted for `azure_openai`, local otherwise; its config defines [`STUB_INSTANCE`]), so no network is
/// ever used.
pub fn embedders(mode: Mode, config: &Config, settings: &ConfigServices) -> Embedders {
    let resolved = settings.resolve(None);
    let instances: Vec<Instance> = resolved
        .instances()
        .into_iter()
        .map(|name| build_instance(mode, config, settings, &resolved, name))
        .collect();
    Embedders { instances }
}

/// One instance, built from the machine config as it is NOW (`umx embeddings probe` of an instance added or changed
/// after the daemon started, ws004 s27): the same builder and credential path as [`embedders`]. `None` when the
/// config defines no such instance. The caller uses it once and drops it; nothing keeps it.
pub fn embedder_now(
    mode: Mode,
    config: &Config,
    settings: &ConfigServices,
    name: &str,
) -> Option<Instance> {
    let resolved = settings.resolve(None);
    resolved
        .instances()
        .into_iter()
        .find(|n| n == name)
        .map(|name| build_instance(mode, config, settings, &resolved, name))
}

/// Whether the machine config defines instance `name` now exactly as it did in `started` (every
/// `embedders.<name>.*` value): when it does, the instance the daemon built at start is the one the config names.
pub fn instance_unchanged(started: &Resolved, now: &Resolved, name: &str) -> bool {
    let prefix = format!("{INSTANCES}.{name}.");
    let of = |r: &Resolved| -> Vec<(String, Option<ConfigValue>)> {
        r.settings
            .iter()
            .filter(|s| s.key.starts_with(&prefix))
            .map(|s| (s.key.clone(), s.value.clone()))
            .collect()
    };
    of(started) == of(now)
}

fn build_instance(
    mode: Mode,
    config: &Config,
    settings: &ConfigServices,
    resolved: &Resolved,
    name: String,
) -> Instance {
    let kind = text(resolved, &name, "kind").unwrap_or_default();
    let (built, local) = match mode {
        Mode::Real if kind == "local" => match build_local(resolved, &name, config) {
            Ok(l) => (Ok(Arc::clone(&l) as Arc<dyn Embedder>), Some(l)),
            Err(why) => (Err(why), None),
        },
        Mode::Real => (build(resolved, &name, &kind, config, settings), None),
        #[cfg(feature = "stub")]
        Mode::Stub => (stub_instance(resolved, &name, &kind), None),
    };
    let tokens_per_minute = resolved
        .value(&format!("{INSTANCES}.{name}.tokens_per_minute"))
        .and_then(|v| match v {
            ConfigValue::Int(n) => u64::try_from(*n).ok(),
            _ => None,
        })
        .or((kind == "azure_openai").then_some(AZURE_TOKENS_PER_MIN));
    let host = (kind != "local")
        .then(|| text(resolved, &name, "endpoint"))
        .flatten()
        .and_then(|e| umx_core::problem::Host::from_endpoint(&e));
    Instance {
        name,
        kind,
        built,
        local,
        tokens_per_minute,
        host,
    }
}

fn text(resolved: &Resolved, name: &str, leaf: &str) -> Option<String> {
    resolved
        .value(&format!("{INSTANCES}.{name}.{leaf}"))
        .and_then(ConfigValue::as_text)
        .map(str::to_owned)
}

fn dims(resolved: &Resolved, name: &str) -> Result<usize, String> {
    match resolved.value(&format!("{INSTANCES}.{name}.dims")) {
        Some(ConfigValue::Int(n)) if *n > 0 => usize::try_from(*n).map_err(|_| unset(name, "dims")),
        _ => Err(unset(name, "dims")),
    }
}

fn unset(name: &str, leaf: &str) -> String {
    format!("{INSTANCES}.{name}.{leaf} is not set")
}

#[cfg(feature = "stub")]
fn stub_instance(resolved: &Resolved, name: &str, kind: &str) -> Result<Arc<dyn Embedder>, String> {
    let model = text(resolved, name, "model").ok_or_else(|| unset(name, "model"))?;
    let key = format!("{model}@{}", dims(resolved, name)?);
    umx_core::ports::ModelKey::parse(&key).map_err(|e| format!("{INSTANCES}.{name}.model: {e}"))?;
    Ok(Arc::new(stub_fake(&key, kind != "azure_openai")))
}

/// One real instance. Errors name keys and env var names, never a value.
fn build(
    resolved: &Resolved,
    name: &str,
    kind: &str,
    config: &Config,
    settings: &ConfigServices,
) -> Result<Arc<dyn Embedder>, String> {
    let need = |leaf: &str| text(resolved, name, leaf).ok_or_else(|| unset(name, leaf));
    match kind {
        "azure_openai" => {
            let mut azure = AzureOpenAiConfig::new(
                need("endpoint")?,
                need("deployment")?,
                need("model")?,
                dims(resolved, name)?,
            );
            if let Some(version) = text(resolved, name, "api_version") {
                azure.api_version = version;
            }
            let embedder = if text(resolved, name, "auth").as_deref() == Some("api_key") {
                azure.auth = AzureAuth::ApiKey {
                    env_var_name: need("api_key_env")?,
                };
                // The key comes through the Secrets port (env, then secrets.env) and goes only into the adapter.
                AzureOpenAiEmbedder::new(azure, |var| {
                    settings.secret(var).map(|(_, s)| s.expose().to_owned())
                })
            } else {
                let (az, azd) = az_programs(config);
                AzureOpenAiEmbedder::with_token_source(
                    azure,
                    Box::new(
                        CliTokenSource::with_programs(az, azd)
                            .with_timeout(config.client_budgets.token_tool),
                    ),
                )
            };
            let embedder = embedder.map_err(|e| format!("{INSTANCES}.{name}: {e}"))?;
            Ok(Arc::new(embedder))
        }
        // `local` is built by build_local.
        other => Err(format!(
            "{INSTANCES}.{name} is kind {other}, which this umx cannot run yet"
        )),
    }
}

/// The instance `embedder.active` names in the machine config, built by [`embedders`] (the one builder), for
/// `umx doctor`'s readiness check: doctor calls `ready()` on it and never `embed`. `Ok(None)` when none is chosen;
/// `Err` says why it cannot be built, naming keys and env var names, never a value.
pub fn configured_embedder(
    mode: Mode,
    config: &Config,
    settings: &ConfigServices,
) -> Result<Option<Arc<dyn Embedder>>, String> {
    embedders(mode, config, settings)
        .active(&settings.resolve(None))
        .map(|chosen| chosen.map(|(_, e)| e))
}

fn int(resolved: &Resolved, name: &str, leaf: &str, default: usize) -> usize {
    match resolved.value(&format!("{INSTANCES}.{name}.{leaf}")) {
        Some(ConfigValue::Int(n)) => usize::try_from(*n).unwrap_or(default),
        _ => default,
    }
}

/// A `local` instance: the llama.cpp adapter over a model umx ships (`model`, by name), with the s04 defaults
/// (one session of 4 threads, batches of 16, the GPU on macOS) unless `threads`, `sessions`, `batch_size` or
/// `execution` say otherwise. Checks the configuration only: the model file is read (and its sha256 checked) on
/// first use, never here.
fn build_local(
    resolved: &Resolved,
    name: &str,
    config: &Config,
) -> Result<Arc<LlamaEmbedder>, String> {
    let model = text(resolved, name, "model").ok_or_else(|| unset(name, "model"))?;
    let spec = ModelSpec::by_name(&model).ok_or_else(|| {
        format!("{INSTANCES}.{name}.model {model} is not a local model this umx ships (embeddinggemma-2)")
    })?;
    let mut local = local_config(spec, config.models_dir.clone(), models_hooks());
    local.dims = dims(resolved, name)?;
    local.threads = int(resolved, name, "threads", local.threads);
    local.sessions = int(resolved, name, "sessions", local.sessions);
    local.batch_size = int(resolved, name, "batch_size", local.batch_size);
    if text(resolved, name, "execution").as_deref() == Some("cpu") {
        local.execution = Execution::Cpu;
    }
    local_embedder(local).map_err(|e| format!("{INSTANCES}.{name}: {e}"))
}

/// The llama.cpp adapter's starting configuration: its defaults, with the models folder's hooks, which it holds
/// while it writes the CPU file.
pub fn local_config(
    spec: ModelSpec,
    models_dir: std::path::PathBuf,
    hooks: ModelsHooks,
) -> LlamaConfig {
    let mut local = LlamaConfig::new(spec, models_dir);
    local.models_lock = hooks.lock;
    local.available_bytes = hooks.available;
    local
}

/// The llama.cpp adapter for a configuration (also what compose's tests build over the stub model).
pub fn local_embedder(
    config: LlamaConfig,
) -> Result<Arc<LlamaEmbedder>, umx_core::ports::EmbedError> {
    LlamaEmbedder::new(config).map(Arc::new)
}

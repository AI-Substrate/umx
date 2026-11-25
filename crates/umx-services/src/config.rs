//! Config use cases (req-72) over the ConfigFiles and Secrets ports: show, set, unset and check, plus the resolve
//! later steps use. Its own service, separate from the graph `Services` (d-12): it holds only its two ports.

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use umx_core::Problem;
use umx_core::config::{
    self, Applies, ConfigValue, Entries, FileLayer, INSTANCES, Inputs, Key, KeySpec, Resolved,
    SOURCE_ROOTS, Source, Target, problems,
};
use umx_core::ports::{
    ConfigFile, ConfigFiles, ConfigFilesError, GraphDir, SecretSource, Secrets, SecretsIssue,
    SourceError, SourceReader,
};

use crate::problems::ImportAsk;
use crate::{Done, Outcome, SourceRoots};

/// What the process was started with: the env values of catalogue env names, flag values and host defaults.
/// `machine` and `repo` are always empty here; they are read from the files on each call.
pub type Base = Inputs;

pub struct ConfigServices {
    files: Arc<dyn ConfigFiles>,
    secrets: Arc<dyn Secrets>,
    base: Base,
    secrets_file: GraphDir,
    /// The values this process runs with (roots, idle unload), resolved once at start.
    running: Resolved,
    /// Checks a source root's folder (req-47, ws006 S1); None refuses every source root.
    sources: Option<Arc<dyn SourceReader>>,
}

/// The machine layer as read at start, for compose.
#[derive(Clone, Debug, PartialEq)]
pub struct Startup {
    pub resolved: Resolved,
    /// Set when graph folders must stay closed: the fail-closed key, or `config.toml` when the machine file is
    /// refused or does not parse.
    pub closed: Option<String>,
}

/// The machine file's entries and what is wrong with it. A refused or unparseable file reads as empty.
fn read_layer(
    files: &dyn ConfigFiles,
    file: &ConfigFile,
    from_check: bool,
) -> (Entries, Option<Problem>) {
    match files.read(file) {
        Ok(None) => (Entries::new(), None),
        Ok(Some(bytes)) => match config::decode(&bytes) {
            Ok(e) => (e, None),
            Err(e) => (
                Entries::new(),
                Some(problems::file_invalid(&files.path(file), &e, from_check)),
            ),
        },
        Err(e) => (Entries::new(), Some(file_problem(files, file, &e))),
    }
}

/// A machine file that is refused or does not parse closes the fail-closed settings, whatever env says: the
/// same rule at start ([`ConfigServices::startup`]) and in every later resolve, so `show` never lists roots that
/// are not in force.
fn close(resolved: &mut Resolved) {
    for s in resolved.settings.iter_mut().filter(|s| s.spec.fail_closed) {
        (s.value, s.source) = (None, Source::Invalid);
        if !resolved.closed.contains(&s.key) {
            resolved.closed.push(s.key.clone());
        }
    }
}

fn file_problem(files: &dyn ConfigFiles, file: &ConfigFile, e: &ConfigFilesError) -> Problem {
    match e {
        ConfigFilesError::Refused { file, reason } => problems::file_refused(file, reason),
        ConfigFilesError::OutsideRoots(_) => problems::repo_none(),
        ConfigFilesError::Io(_) => crate::problems::config_write_failed(&files.path(file), e),
    }
}

/// One setting as `config show` prints it.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub key: String,
    pub value: Option<ConfigValue>,
    pub source: Source,
    /// The file it came from, for machine and repo.
    pub file: Option<GraphDir>,
    pub env: Option<&'static str>,
    pub flag: Option<&'static str>,
    pub applies: Applies,
    pub doc: &'static str,
    /// For a restart setting whose value differs from what this process runs with.
    pub running: Option<ConfigValue>,
    /// For api_key_env: `set (env)`, `set (secrets.env)` or `unset`. Never the value.
    pub secret: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConfigShown {
    pub machine_file: GraphDir,
    pub repo_file: Option<GraphDir>,
    pub secrets_file: GraphDir,
    pub settings: Vec<Shown>,
    /// How many problems `config check` would report.
    pub problems: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConfigSet {
    pub key: String,
    pub value: ConfigValue,
    /// The value it replaced, only if that was valid: an invalid old value may be a pasted secret.
    pub was: Option<ConfigValue>,
    /// True if the key had a value before, shown or not.
    pub replaced: bool,
    pub layer: FileLayer,
    pub file: GraphDir,
    pub applies: Applies,
}

/// `root add`: the root as stored, and where its folder resolves now.
#[derive(Clone, Debug, PartialEq)]
pub struct RootAdded {
    pub name: String,
    /// As given, and as stored.
    pub folder: String,
    pub canonical: GraphDir,
    /// The folder it replaced, if the name was set.
    pub was: Option<String>,
    pub file: GraphDir,
}

/// One row of `root list`: `status` is `ok`, `outside_roots`, `not_a_folder` or `unreadable`, checked now.
#[derive(Clone, Debug, PartialEq)]
pub struct RootRow {
    pub name: String,
    pub folder: String,
    /// Where the folder resolves, when it is usable.
    pub canonical: Option<GraphDir>,
    pub status: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RootRemoved {
    pub name: String,
    pub file: GraphDir,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConfigUnset {
    pub key: String,
    /// How many keys were removed (an instance removes all of its keys).
    pub removed: usize,
    pub layer: FileLayer,
    pub file: GraphDir,
}

/// What `umx config import` wrote, or would write with `dry_run` (ws004 s27).
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigImported {
    pub from: &'static str,
    /// The other tool's file it read.
    pub source: GraphDir,
    pub file: GraphDir,
    pub instance: String,
    pub provider: String,
    /// Machine-layer keys and values, in catalogue order. None of them is a secret.
    pub entries: Vec<(String, ConfigValue)>,
    pub dry_run: bool,
    /// The machine config defined the instance before (only with `force`).
    pub replaced: bool,
    /// The command that writes exactly this import: a dry run's next step.
    pub write: umx_core::Command,
    /// The vector space the instance makes, `model@dims` (F4): what a graph must record to use it.
    pub key: Option<umx_core::ports::ModelKey>,
    pub source_uses_key: bool,
    pub model_inferred: bool,
}

impl ConfigServices {
    pub fn new(
        files: Arc<dyn ConfigFiles>,
        secrets: Arc<dyn Secrets>,
        base: Base,
        secrets_file: GraphDir,
        running: Resolved,
    ) -> ConfigServices {
        ConfigServices {
            files,
            secrets,
            base,
            secrets_file,
            running,
            sources: None,
        }
    }

    /// The config files port, which compose also gives the graph services for the repo Graphfile (ws002 s17), so
    /// both read and write through the one adapter.
    pub fn files(&self) -> Arc<dyn ConfigFiles> {
        Arc::clone(&self.files)
    }

    /// The port that checks source roots' folders (compose).
    pub fn with_sources(mut self, sources: Arc<dyn SourceReader>) -> ConfigServices {
        self.sources = Some(sources);
        self
    }

    /// The graph roots this process runs with, for `path_outside_roots`.
    fn graph_roots(&self) -> Vec<GraphDir> {
        self.running
            .roots()
            .iter()
            .filter_map(|r| GraphDir::parse(r).ok())
            .collect()
    }

    /// A source root's folder, canonical and inside the roots now (security ruling ws006 S1, rule 1).
    fn source_folder(&self, name: &str, folder: &GraphDir) -> Result<GraphDir, Problem> {
        if self.sources.is_none() {
            return Err(crate::problems::path_outside_roots(folder, &[]));
        }
        self.canonical(folder).map_err(|e| match e {
            SourceError::OutsideRoots => {
                crate::problems::path_outside_roots(folder, &self.graph_roots())
            }
            SourceError::NotFolder
            | SourceError::Missing
            | SourceError::NotFile
            | SourceError::TooLarge => {
                crate::problems::root_unavailable(name, crate::problems::ROOT_NOT_FOLDER)
            }
            SourceError::Io(_) => {
                crate::problems::root_unavailable(name, crate::problems::ROOT_UNREADABLE)
            }
        })
    }

    fn canonical(&self, folder: &GraphDir) -> Result<GraphDir, SourceError> {
        match &self.sources {
            Some(reader) => reader.canonical_root(folder),
            None => Err(SourceError::OutsideRoots),
        }
    }

    /// `umx root add <name> <folder>`: `source_roots.<name>` in the machine config, once the folder is an existing
    /// folder inside the roots.
    pub fn root_add(&self, name: &str, folder: &str) -> Outcome<RootAdded> {
        if !config::is_root_name(name) {
            return Err(crate::problems::root_name_invalid(name));
        }
        let key = alloc::format!("{SOURCE_ROOTS}.{name}");
        // An invalid folder text gets set's config_value_invalid; a valid one is checked here first.
        let canonical = match GraphDir::parse(folder) {
            Ok(f) => Some(self.source_folder(name, &f)?),
            Err(_) => None,
        };
        let set = self.set(None, &key, folder, FileLayer::Machine)?;
        let canonical = canonical.expect("set refuses a folder that does not parse");
        Ok(Done {
            value: RootAdded {
                name: name.to_string(),
                folder: folder.to_string(),
                canonical,
                was: set.value.was.and_then(|v| v.as_text().map(str::to_string)),
                file: set.value.file,
            },
            notes: set.notes,
        })
    }

    /// `umx root list`: the machine's source roots by name, each checked now. A machine file umx cannot trust or
    /// parse is the error, never an empty list; a `source_roots.*` key or value it cannot use is a warning, and is
    /// not listed.
    pub fn root_list(&self) -> Outcome<Vec<RootRow>> {
        let machine = self.entries_for_write(&ConfigFile::Machine)?;
        let resolved = config::resolve(&Inputs {
            machine,
            ..self.base.clone()
        });
        let prefix = alloc::format!("{SOURCE_ROOTS}.");
        let warnings = resolved
            .findings
            .iter()
            .filter(|p| p.slot("key").is_some_and(|k| k.starts_with(&prefix)))
            .cloned()
            .collect();
        let rows = resolved
            .source_roots()
            .into_iter()
            .map(|(name, folder)| {
                let checked = GraphDir::parse(&folder).map(|f| self.canonical(&f));
                let (canonical, status) = match checked {
                    Ok(Ok(c)) => (Some(c), "ok"),
                    Ok(Err(SourceError::OutsideRoots)) => (None, "outside_roots"),
                    Ok(Err(SourceError::NotFolder)) => (None, "not_a_folder"),
                    _ => (None, "unreadable"),
                };
                RootRow {
                    name,
                    folder,
                    canonical,
                    status,
                }
            })
            .collect();
        Ok(Done {
            value: rows,
            notes: warnings,
        })
    }

    /// `umx root remove <name>`: refuses a name the machine config does not set. A machine file umx cannot trust or
    /// parse is reported as itself, and left as it is.
    pub fn root_remove(&self, name: &str) -> Outcome<RootRemoved> {
        if !config::is_root_name(name) {
            return Err(crate::problems::root_name_invalid(name));
        }
        let key = alloc::format!("{SOURCE_ROOTS}.{name}");
        if !self
            .entries_for_write(&ConfigFile::Machine)?
            .contains_key(&key)
        {
            return Err(crate::problems::root_unknown(name));
        }
        let unset = self.unset(None, &key, FileLayer::Machine)?;
        Ok(Done {
            value: RootRemoved {
                name: name.to_string(),
                file: unset.value.file,
            },
            notes: unset.notes,
        })
    }

    /// Reads the machine layer at start (compose). Roots fail closed: an invalid roots value, or a machine file
    /// that is refused or does not parse, leaves no roots at all, never the wider default.
    pub fn startup(files: &dyn ConfigFiles, base: &Base) -> Startup {
        let (machine, problem) = read_layer(files, &ConfigFile::Machine, true);
        let mut resolved = config::resolve(&Inputs {
            machine,
            ..base.clone()
        });
        let closed = if problem.is_some() {
            close(&mut resolved);
            Some(config::FILE_NAME.to_string())
        } else {
            resolved.closed.first().cloned()
        };
        resolved.findings.splice(0..0, problem);
        Startup { resolved, closed }
    }

    /// The folder holding `.git` at or above `dir`, within the roots (`graph list --all`, req-116).
    pub fn repo_root(&self, dir: &GraphDir) -> Option<GraphDir> {
        self.files.repo_root(dir).ok().flatten()
    }

    fn repo(&self, cwd: Option<&GraphDir>) -> Option<GraphDir> {
        cwd.and_then(|d| self.files.repo_root(d).ok().flatten())
            .filter(|r| {
                self.files.path(&ConfigFile::Repo(r.clone()))
                    != self.files.path(&ConfigFile::Machine)
            })
    }

    /// Whether the repo layer that applies to `dir`, if any, can be read and parsed. False when its file is refused
    /// (not owned, a symlink, over the 64 KiB cap) or does not parse: then `resolve` leaves it out and the
    /// machine's choice applies, so a caller deciding whether that folder may use the wired embedder must treat it
    /// as naming none (security, s11).
    pub fn repo_layer_ok(&self, dir: &GraphDir) -> bool {
        let Ok(root) = self.files.repo_root(dir) else {
            return false;
        };
        match root.filter(|r| {
            self.files.path(&ConfigFile::Repo(r.clone())) != self.files.path(&ConfigFile::Machine)
        }) {
            Some(r) => read_layer(&*self.files, &ConfigFile::Repo(r), true)
                .1
                .is_none(),
            None => true,
        }
    }

    /// Whether the repo file that applies to `dir` itself names `key`, whether or not `resolve` kept it. A repo
    /// value that `resolve` drops (one that would widen, an invalid one, one that looks like a secret) still says
    /// what the repo wanted, so a caller that must fail closed asks this rather than the resolved source (F1).
    pub fn repo_layer_sets(&self, dir: &GraphDir, key: &str) -> bool {
        self.repo(Some(dir)).is_some_and(|r| {
            read_layer(&*self.files, &ConfigFile::Repo(r), true)
                .0
                .contains_key(key)
        })
    }

    /// Every layer for a folder: the machine file, the repo file of the repository `dir` is in (for a graph, its
    /// folder; for a command, the caller's working folder), env, flags and defaults. Never fails: what is wrong is
    /// in `findings`.
    pub fn resolve(&self, dir: Option<&GraphDir>) -> Resolved {
        self.resolve_with(dir).0
    }

    fn resolve_with(&self, dir: Option<&GraphDir>) -> (Resolved, Option<GraphDir>) {
        let (machine, machine_problem) = read_layer(&*self.files, &ConfigFile::Machine, true);
        let repo = self.repo(dir);
        let (repo_entries, repo_problem) = match &repo {
            Some(r) => {
                let (e, p) = read_layer(&*self.files, &ConfigFile::Repo(r.clone()), true);
                (Some(e), p)
            }
            None => (None, None),
        };
        let mut resolved = config::resolve(&Inputs {
            machine,
            repo: repo_entries,
            ..self.base.clone()
        });
        if machine_problem.is_some() {
            close(&mut resolved);
        }
        let file_problems: Vec<Problem> = machine_problem.into_iter().chain(repo_problem).collect();
        resolved.findings.splice(0..0, file_problems);
        (resolved, repo)
    }

    pub fn show(&self, cwd: Option<&GraphDir>) -> Outcome<ConfigShown> {
        let (resolved, repo) = self.resolve_with(cwd);
        let machine_file = self.files.path(&ConfigFile::Machine);
        let repo_file = repo.map(|r| self.files.path(&ConfigFile::Repo(r)));
        let settings = resolved
            .settings
            .iter()
            .map(|s| {
                let running = (s.spec.applies == Applies::Restart)
                    .then(|| self.running.get(&s.key).and_then(|r| r.value.clone()))
                    .flatten()
                    .filter(|r| Some(r) != s.value.as_ref());
                let secret = (s.spec.key == "api_key_env").then(|| {
                    match s.value.as_ref().and_then(ConfigValue::as_text) {
                        Some(name) => match self.secrets.lookup(name) {
                            Some((SecretSource::Env, _)) => "set (env)",
                            Some((SecretSource::SecretsFile, _)) => "set (secrets.env)",
                            None => "unset",
                        },
                        None => "unset",
                    }
                });
                Shown {
                    key: s.key.clone(),
                    value: s.value.clone(),
                    source: s.source,
                    file: match s.source {
                        Source::Machine => Some(machine_file.clone()),
                        Source::Repo => repo_file.clone(),
                        _ => None,
                    },
                    env: s.spec.env,
                    flag: s.spec.flag,
                    applies: s.spec.applies,
                    doc: s.spec.doc,
                    running,
                    secret,
                }
            })
            .collect();
        let problems = self.check_problems(cwd).len();
        Ok(Done {
            value: ConfigShown {
                machine_file,
                repo_file,
                secrets_file: self.secrets_file.clone(),
                settings,
                problems,
            },
            notes: Vec::new(),
        })
    }

    /// The file a layer means for `cwd`, or why there is none.
    fn layer_file(&self, layer: FileLayer, cwd: Option<&GraphDir>) -> Result<ConfigFile, Problem> {
        match layer {
            FileLayer::Machine => Ok(ConfigFile::Machine),
            FileLayer::Repo => self
                .repo(cwd)
                .map(ConfigFile::Repo)
                .ok_or_else(problems::repo_none),
        }
    }

    /// The file's entries for a rewrite or a root command: refused while it is untrusted or does not parse, so a flat
    /// rewrite never erases what umx could not read, and such a file never reads as an empty one.
    fn entries_for_write(&self, file: &ConfigFile) -> Result<Entries, Problem> {
        match self.files.read(file) {
            Ok(None) => Ok(Entries::new()),
            Ok(Some(bytes)) => config::decode(&bytes)
                .map_err(|e| problems::file_invalid(&self.files.path(file), &e, false)),
            Err(e) => Err(file_problem(&*self.files, file, &e)),
        }
    }

    fn machine_entries(&self) -> Entries {
        read_layer(&*self.files, &ConfigFile::Machine, true).0
    }

    pub fn set(
        &self,
        cwd: Option<&GraphDir>,
        key_text: &str,
        value_text: &str,
        layer: FileLayer,
    ) -> Outcome<ConfigSet> {
        let key = config::key(key_text).ok_or_else(|| problems::key_unknown(key_text, None))?;
        let spec: &KeySpec = key.spec();
        let full = key.full();
        if layer == FileLayer::Repo {
            let why = if config::secret_like_key(&full) || config::looks_like_secret(value_text) {
                Some(problems::WHY_SECRET)
            } else if !spec.repo {
                Some(problems::WHY_MACHINE_ONLY)
            } else {
                None
            };
            if let Some(why) = why {
                return Err(problems::repo_refused(&full, why, spec));
            }
        }
        let value = config::parse_value(spec, value_text)
            .map_err(|why| problems::value_invalid(&full, layer, why, spec))?;
        if let Key::Root(name) = &key
            && let Ok(folder) = GraphDir::parse(value_text)
        {
            self.source_folder(name, &folder)?;
        }
        if full == "embedder.active" && value_text != "none" {
            let machine = self.machine_entries();
            let kind = machine.get(&alloc::format!("{INSTANCES}.{value_text}.kind"));
            match (layer, kind.and_then(ConfigValue::as_text)) {
                (_, None) => return Err(problems::instance_unknown("embedder.active")),
                (FileLayer::Repo, Some(k)) if k != "local" => {
                    return Err(problems::repo_refused(&full, problems::WHY_WIDENS, spec));
                }
                _ => {}
            }
        }
        let file = self.layer_file(layer, cwd)?;
        let mut entries = self.entries_for_write(&file)?;
        let old = entries.insert(full.clone(), value.clone());
        let replaced = old.is_some();
        let was = old.filter(|v| config::validate(spec, v).is_ok());
        self.files
            .write(&file, &config::encode(&entries))
            .map_err(|e| file_problem(&*self.files, &file, &e))?;
        let warnings = match spec.applies {
            Applies::Restart => alloc::vec![problems::restart_needed(&full)],
            Applies::Now => Vec::new(),
        };
        Ok(Done {
            value: ConfigSet {
                key: full,
                value,
                was,
                replaced,
                layer,
                file: self.files.path(&file),
                applies: spec.applies,
            },
            notes: warnings,
        })
    }

    /// `umx config import --from flowspace3` (ws004 s27, D27): copies only the embedder flowspace3 uses (kind,
    /// endpoint, deployment, model, dims, api version; auth always `cli`) into the machine layer as one instance,
    /// in one write. Reads only flowspace3's `config.toml` through the port, never a secret; writes no trust and does
    /// not change `embedder.active`. Refuses to replace an instance the machine config defines unless `force`.
    pub fn import(&self, from: &str, ask: &ImportAsk) -> Outcome<ConfigImported> {
        let source_file = match from {
            "flowspace3" => ConfigFile::Flowspace3,
            _ => {
                return Err(crate::problems::import_refused(
                    crate::problems::WHY_ONLY_FS3,
                    None,
                    ask,
                ));
            }
        };
        let source = self.files.path(&source_file);
        let entries = match self.files.read(&source_file) {
            Ok(Some(bytes)) => {
                config::decode(&bytes).map_err(|e| problems::file_invalid(&source, &e, true))?
            }
            Ok(None) => {
                return Err(crate::problems::import_refused(
                    crate::problems::WHY_FS3_MISSING,
                    None,
                    ask,
                ));
            }
            Err(e) => return Err(file_problem(&*self.files, &source_file, &e)),
        };
        let import = umx_core::config_import::flowspace3(&entries, ask.name, ask.model, ask.dims)
            .map_err(|r| {
            let fix = match r {
                umx_core::config_import::Refused::NoModel => Some("--model"),
                umx_core::config_import::Refused::BadName => Some("--as"),
                _ => None,
            };
            crate::problems::import_refused(r.why(), fix, ask)
        })?;
        let file = ConfigFile::Machine;
        let mut machine = self.entries_for_write(&file)?;
        let prefix = alloc::format!("{INSTANCES}.{}.", import.instance);
        let replaced = machine.keys().any(|k| k.starts_with(&prefix));
        if replaced && !ask.force {
            return Err(crate::problems::import_exists(&import.instance, ask));
        }
        let leaf = |l: &str| {
            let k = alloc::format!("{prefix}{l}");
            import
                .entries
                .iter()
                .find(|(e, _)| *e == k)
                .map(|(_, v)| v.clone())
        };
        let key = match (leaf("model"), leaf("dims")) {
            (Some(ConfigValue::Text(m)), Some(ConfigValue::Int(d))) => {
                umx_core::ports::ModelKey::parse(&alloc::format!("{m}@{d}")).ok()
            }
            _ => None,
        };
        let write = ImportAsk {
            name: Some(&import.instance),
            dry_run: false,
            ..*ask
        }
        .command(None);
        if !ask.dry_run {
            machine.retain(|k, _| !k.starts_with(&prefix));
            machine.extend(import.entries.iter().cloned());
            self.files
                .write(&file, &config::encode(&machine))
                .map_err(|e| file_problem(&*self.files, &file, &e))?;
        }
        Ok(Done {
            value: ConfigImported {
                from: "flowspace3",
                source,
                file: self.files.path(&file),
                instance: import.instance,
                provider: import.provider,
                entries: import.entries,
                dry_run: ask.dry_run,
                replaced,
                write,
                key,
                source_uses_key: import.source_uses_key,
                model_inferred: import.model_inferred,
            },
            notes: Vec::new(),
        })
    }

    pub fn unset(
        &self,
        cwd: Option<&GraphDir>,
        key_text: &str,
        layer: FileLayer,
    ) -> Outcome<ConfigUnset> {
        let file = self.layer_file(layer, cwd)?;
        let mut entries = self.entries_for_write(&file)?;
        // An unknown key that is in the file may be removed: that is how check's fix for it works.
        let removed = match config::target(key_text) {
            Some(Target::Instance(name)) => {
                let prefix = alloc::format!("{INSTANCES}.{name}.");
                let before = entries.len();
                entries.retain(|k, _| !k.starts_with(&prefix));
                before - entries.len()
            }
            Some(Target::Key(k)) => usize::from(entries.remove(&k.full()).is_some()),
            None if entries.contains_key(key_text) => {
                usize::from(entries.remove(key_text).is_some())
            }
            None => return Err(problems::key_unknown(key_text, None)),
        };
        let key = match config::target(key_text) {
            Some(Target::Key(k)) => k.full(),
            Some(Target::Instance(name)) => alloc::format!("{INSTANCES}.{name}"),
            None => key_text.to_string(),
        };
        if removed > 0 {
            self.files
                .write(&file, &config::encode(&entries))
                .map_err(|e| file_problem(&*self.files, &file, &e))?;
        }
        let restart = config::key(&key).is_some_and(|k| k.spec().applies == Applies::Restart);
        Ok(Done {
            value: ConfigUnset {
                key: key.clone(),
                removed,
                layer,
                file: self.files.path(&file),
            },
            notes: if restart && removed > 0 {
                alloc::vec![problems::restart_needed(&key)]
            } else {
                Vec::new()
            },
        })
    }

    fn check_problems(&self, cwd: Option<&GraphDir>) -> Vec<Problem> {
        let resolved = self.resolve(cwd);
        let mut out = resolved.findings.clone();
        for name in resolved.instances() {
            let key = alloc::format!("{INSTANCES}.{name}.api_key_env");
            let auth = resolved.value(&alloc::format!("{INSTANCES}.{name}.auth"));
            if auth.and_then(ConfigValue::as_text) != Some("api_key") {
                continue;
            }
            if let Some(env) = resolved.value(&key).and_then(ConfigValue::as_text)
                && self.secrets.lookup(env).is_none()
            {
                out.push(problems::secret_missing(&key, env));
            }
        }
        for issue in self.secrets.issues() {
            out.push(match issue {
                SecretsIssue::Refused { file, reason } => problems::file_refused(&file, reason),
                SecretsIssue::BadLine { file, line } => problems::secrets_line_invalid(&file, line),
            });
        }
        // A source root that cannot be used now: the same check every evidence write makes.
        for (name, folder) in self.resolve(None).source_roots() {
            if let Ok(f) = GraphDir::parse(&folder)
                && let Err(p) = self.source_folder(&name, &f)
            {
                out.push(p);
            }
        }
        // A restart setting whose machine value changed since this process started. The machine layer, not this
        // folder's: a repo narrowing embedder.active is read per graph, so it needs no restart.
        let machine = self.resolve(None);
        for s in machine
            .settings
            .iter()
            .filter(|s| s.spec.applies == Applies::Restart)
        {
            let running = self.running.get(&s.key).map(|r| (&r.value, r.source));
            if running.is_some_and(|(v, src)| {
                *v != s.value || (src == Source::Invalid) != (s.source == Source::Invalid)
            }) {
                out.push(problems::restart_needed(&s.key));
            }
        }
        out
    }

    /// What is wrong, each with a runnable fix; empty when the config is fine.
    pub fn check(&self, cwd: Option<&GraphDir>) -> Vec<Problem> {
        self.check_problems(cwd)
    }

    /// (port id, adapter id) for the two ports this service runs over (E-1).
    pub fn adapters(&self) -> [(&'static str, &'static str); 2] {
        [
            ("config-files", self.files.adapter_id()),
            ("secrets", self.secrets.adapter_id()),
        ]
    }

    /// A secret by env var name, env first, then the secrets file. Expose its value only at the point of use.
    pub fn secret(&self, name: &str) -> Option<(SecretSource, umx_core::ports::Secret)> {
        self.secrets.lookup(name)
    }
}

/// Source roots come from the machine layer only: `resolve(None)` reads no repo file.
impl SourceRoots for ConfigServices {
    fn root_folder(&self, name: &str) -> Option<GraphDir> {
        self.resolve(None)
            .source_roots()
            .into_iter()
            .find(|(n, _)| n == name)
            .and_then(|(_, folder)| GraphDir::parse(&folder).ok())
    }

    fn repo_top(&self, dir: &GraphDir) -> Option<GraphDir> {
        self.files.repo_root(dir).ok().flatten()
    }
}

/// The env names the catalogue reads, for compose.
pub fn env_names() -> Vec<&'static str> {
    config::SETTINGS
        .iter()
        .chain(config::INSTANCE_KEYS)
        .filter_map(|s| s.env)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use umx_fakes::{FakeConfigFiles, FakeSecrets, FakeSourceReader};

    fn dir(p: &str) -> GraphDir {
        GraphDir::parse(p).unwrap()
    }

    fn services() -> (ConfigServices, Arc<FakeConfigFiles>) {
        let files = Arc::new(FakeConfigFiles::new(dir("/home"), dir("/")));
        let cs = ConfigServices::new(
            files.clone(),
            Arc::new(FakeSecrets::new(Default::default())),
            Base::default(),
            dir("/home/secrets.env"),
            config::resolve(&Inputs::default()),
        )
        .with_sources(Arc::new(FakeSourceReader::open(dir("/"))));
        (cs, files)
    }

    fn machine(files: &FakeConfigFiles) -> Option<Vec<u8>> {
        files.read(&ConfigFile::Machine).unwrap()
    }

    fn names(cs: &ConfigServices) -> (Vec<String>, Vec<&'static str>) {
        let done = cs.root_list().unwrap();
        (
            done.value.into_iter().map(|r| r.name).collect(),
            done.notes.iter().map(|p| p.code).collect(),
        )
    }

    /// R1 (round 1): root list and root remove report a machine file they cannot use as itself, never as
    /// an empty config, and never rewrite it.
    #[test]
    fn root_commands_report_the_config_they_could_not_read() {
        let (cs, files) = services();
        assert_eq!(names(&cs), (vec![], vec![]), "healthy and empty");

        cs.root_add("acme", "/work/acme").unwrap();
        assert_eq!(names(&cs), (vec!["acme".to_string()], vec![]));

        // An invalid value is a warning naming its key; the good root is still listed.
        let mut entries = config::decode(&machine(&files).unwrap()).unwrap();
        entries.insert(alloc::format!("{SOURCE_ROOTS}.bad"), ConfigValue::Int(7));
        files
            .write(&ConfigFile::Machine, &config::encode(&entries))
            .unwrap();
        assert_eq!(
            names(&cs),
            (vec!["acme".to_string()], vec!["config_value_invalid"])
        );
        let warning = &cs.root_list().unwrap().notes[0];
        assert_eq!(warning.slot("key").as_deref(), Some("source_roots.bad"));

        // Malformed, then refused: the file's own problem, from list and from remove, with its bytes left alone.
        let malformed = b"source_roots.acme = \"/work/acme\"\n[[[\n".to_vec();
        files.write(&ConfigFile::Machine, &malformed).unwrap();
        assert_eq!(cs.root_list().unwrap_err().code, "config_file_invalid");
        assert_eq!(
            cs.root_remove("acme").unwrap_err().code,
            "config_file_invalid"
        );
        assert_eq!(machine(&files), Some(malformed));

        cs.root_add("acme", "/work/acme").unwrap_err();
        files.write(&ConfigFile::Machine, b"").unwrap();
        cs.root_add("acme", "/work/acme").unwrap();
        let healthy = machine(&files);
        files.refuse(ConfigFile::Machine, Some("it is open to others"));
        assert_eq!(cs.root_list().unwrap_err().code, "config_file_refused");
        assert_eq!(
            cs.root_remove("acme").unwrap_err().code,
            "config_file_refused"
        );
        files.refuse(ConfigFile::Machine, None);
        assert_eq!(machine(&files), healthy);

        cs.root_remove("acme").unwrap();
        assert_eq!(cs.root_remove("acme").unwrap_err().code, "root_unknown");
    }
}

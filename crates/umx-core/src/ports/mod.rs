//! Port traits (blueprint 001, d-5): synchronous, Send + Sync, owned values, typed errors.

pub mod agent_identity;
pub mod clock;
pub mod config_files;
pub mod embedder;
pub mod graph_files;
pub mod id;
pub mod local_state;
pub mod model_fetcher;
pub mod secrets;
pub mod service_manager;
pub mod source_reader;
pub mod ui_host;

pub use agent_identity::{AgentIdentity, IdentityError};
pub use clock::Clock;
pub use config_files::{ConfigFile, ConfigFiles, ConfigFilesError, Swapped};
pub use embedder::{
    Credential, EmbedError, Embedded, Embedder, Locality, ModelKey, ModelKeyError, TextKind,
};
pub use graph_files::{
    Change, Committed, CrashPoint, EntryKind, FileRef, FileStamp, FilesError, FindLimits, Found,
    FoundGraph, GraphDir, GraphFiles, GraphStamps, RELOCATE_INSIDE_ITSELF, RELOCATE_PARENT_MISSING,
    RELOCATE_TARGET_EXISTS, RuleSetFile, RuleSetListing, Written,
};
pub use id::IdGenerator;
pub use local_state::{LocalState, LocalStateError, StateWritten};
pub use model_fetcher::{
    FetchError, FetchProgress, FileFetched, ModelFetcher, ModelFileSpec, ModelSource,
};
pub use secrets::{Secret, SecretSource, Secrets, SecretsIssue};
pub use service_manager::{ServiceError, ServiceManager, ServiceSpec, ServiceState, unit_safe};
pub use source_reader::{SourceError, SourceReader};
pub use ui_host::{UiCodeUse, UiDetached, UiEnd, UiError, UiHost, UiLaunch, UiProcess, UiServed};

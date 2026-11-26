//! Domain types and port traits. Pure: no std (blueprint 001, d-3).
#![no_std]

extern crate alloc;

pub mod address;
pub mod adopt;
pub mod batch;
pub mod codec;
pub mod config;
pub mod config_import;
pub mod cut;
pub mod entity;
pub mod evidence;
pub mod graphfile;
pub mod id;
pub mod lint_terms;
pub mod local_state;
pub mod locator;
pub mod model;
pub mod models;
pub mod name;
pub mod ontology;
pub mod place;
pub mod ports;
pub mod problem;
pub mod provenance;
pub mod receipts;
pub mod registry;
pub mod role_advice;
pub mod rules;
pub mod scan;
pub mod scopes;
pub mod secret_allows;
pub mod secrets;
pub mod settings;
/// The unchecked graph-file writes (F144): outside `ports`, re-exported nowhere.
pub mod setup;
pub mod similar;
pub mod similarity;
pub mod source_hash;
pub mod spine;
pub mod standard;
pub mod status;
pub mod tags;
pub mod time;
pub mod ui;
pub mod value;
pub mod vectors;

/// Field values are JSON values; re-exported so inner crates need no JSON dependency of their own.
pub use serde_json::Value;

pub use id::Id;
pub use locator::{Format, Locator, LocatorKind};
pub use model::{Edge, EmbedderRule, GraphEmbedding, GraphMeta, Legacy, Node, Policy, Visibility};
pub use ontology::{FieldDecl, FieldKind, Ontology, Role, TypeDecl};
pub use problem::{Command, IdText, Problem, SafeText, Slot, ValidName};

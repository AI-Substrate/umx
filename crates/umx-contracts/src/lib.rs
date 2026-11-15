//! One contract suite per port (blueprint 001, c-contracts). Each clause is a public function named by its clause
//! id (`<port>_c<N>_<what>`). A macro per port turns a list of clause names into one `#[test]` each and adds a test
//! that the list equals the suite's `CLAUSES`, so no fake or adapter can skip a clause, and umx-arch's
//! r-fake-per-port finds every name in the adapter's source. Dev-dependency only.

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

/// Fails unless `listed` names exactly the clauses in `clauses`.
pub fn assert_listed(clauses: &[&str], listed: &[&str]) {
    let mut want: Vec<&str> = clauses.to_vec();
    let mut got: Vec<&str> = listed.to_vec();
    want.sort_unstable();
    got.sort_unstable();
    assert_eq!(
        got, want,
        "the contract macro must list every clause of the suite, once"
    );
}

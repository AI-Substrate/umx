//! similar_to rebuilds off the writer (architect, ws004 s09). A full rebuild is O(n^2 x dims), so it never holds a
//! lock while it computes: it copies the whole-node block under a short shared lock, computes every list with no
//! lock held, and hands the lists to the writer, which applies them only to the same load of the graph whose vectors
//! and list settings did not change meanwhile. If anything did, it starts again.

use crate::Job;
use crate::fair::FairLock;
use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};
use umx_core::Problem;
use umx_services::Services;
use umx_services::SimilarityJob;

/// What the writer answers a `Job::SimilarityApply`: files written and warnings, `None` when the vectors moved.
pub(crate) type Applied = Result<Option<(usize, Vec<Problem>)>, Problem>;

/// Times a rebuild starts again when vectors keep arriving during its compute, before it gives up for now (the next
/// load or `umx similarity rebuild` tries again; until then results say `similar_to_incomplete`).
const TRIES: usize = 5;

pub(crate) enum Rebuilt {
    Done {
        files_written: usize,
        warnings: Vec<Problem>,
    },
    /// The graph has no lists for its space yet, or no vectors: the writer's one-call rebuild sets them up.
    NotSetUp,
    /// Vectors changed during every try.
    Moved,
    Failed(Problem),
    /// The writer is gone (stopping, or failed).
    Stopped,
}

/// Rebuilds `alias`'s lists, starting from `first` when the caller already took the snapshot.
pub(crate) fn rebuild(
    services: &FairLock<Services>,
    writer: &Sender<Job>,
    alias: &str,
    mut first: Option<SimilarityJob>,
) -> Rebuilt {
    for _ in 0..TRIES {
        let taken = match first.take() {
            Some(s) => Some(s),
            None => match services.read() {
                Ok(s) => s.similarity_snapshot(alias),
                Err(_) => return Rebuilt::Stopped,
            },
        };
        let Some(job) = taken else {
            return Rebuilt::NotSetUp;
        };
        let lists = job.compute();
        let (tx, rx) = channel();
        if writer
            .send(Job::SimilarityApply(alias.to_owned(), Box::new(lists), tx))
            .is_err()
        {
            return Rebuilt::Stopped;
        }
        match rx.recv() {
            Ok(Ok(Some((files_written, warnings)))) => {
                return Rebuilt::Done {
                    files_written,
                    warnings,
                };
            }
            Ok(Ok(None)) => continue,
            Ok(Err(problem)) => return Rebuilt::Failed(problem),
            Err(_) => return Rebuilt::Stopped,
        }
    }
    Rebuilt::Moved
}

/// Starts a background rebuild for each graph whose stored lists did not match at load; outcomes go to the daemon's
/// stderr (its log), as the embed worker's do.
pub(crate) fn spawn_rebuilds(
    services: &Arc<FairLock<Services>>,
    writer: &Sender<Job>,
    aliases: Vec<String>,
) {
    for alias in aliases {
        let (services, writer) = (Arc::clone(services), writer.clone());
        std::thread::spawn(move || match rebuild(&services, &writer, &alias, None) {
            Rebuilt::Done { warnings, .. } => {
                for problem in warnings {
                    let issue = crate::verbs::issue(&problem, "similarity rebuild");
                    eprintln!(
                        "umx similarity: graph {alias}: {} ({})",
                        issue.message(),
                        issue.code()
                    );
                }
            }
            Rebuilt::Failed(problem) => {
                let issue = crate::verbs::issue(&problem, "similarity rebuild");
                eprintln!(
                    "umx similarity: graph {alias}: {} ({})",
                    issue.message(),
                    issue.code()
                );
            }
            Rebuilt::Moved => {
                eprintln!(
                    "umx similarity: graph {alias}: vectors kept changing; the lists stay incomplete for now"
                )
            }
            Rebuilt::NotSetUp | Rebuilt::Stopped => {}
        });
    }
}

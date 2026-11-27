//! The graph view's agent channel on the daemon (ws009 S6 and S7, architect W5 and W7). umx-services' `UiChannel`
//! checks, scopes and keeps the events and the selection; here is only the wait and the rendering. A view's wait
//! sleeps on [`Daemon::ui_wake`] holding no services lock and not the writer, so it never delays a read or a write.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use umx_core::ui::{Change, TouchedIds, UiAction, UiCmd as CoreCmd, UiEvent};
use umx_protocol::{Envelope, Op, Request, UiCmd, Value, json};
use umx_services::ui::UiWaited;

use crate::{Daemon, QUIESCE, internal, verbs};

impl Daemon {
    /// `umx ui focus|highlight|filter|fit|clear`, the view's selection (S7) and `umx ui selection`: each under the
    /// services' shared lock, then the channel's own.
    pub(crate) fn ui_op(&self, request: &Request) -> Envelope {
        let Some(op) = &request.op else {
            return Envelope::fail(&request.command, internal("no_operation"));
        };
        let command = op.command();
        if let Some(problem) = verbs::scope_usage(request) {
            return Envelope::fail(command, verbs::issue(&problem, command));
        }
        let ctx = verbs::ctx(request);
        let Ok(services) = self.services.read() else {
            return Envelope::fail(command, internal("writer_stopped"));
        };
        let store = services.graph();
        let mut ui = self.ui.lock().unwrap_or_else(|e| e.into_inner());
        match op {
            Op::UiCommand { command: c } => {
                // The sender as the CLI claimed it, checked as for a write (P2): absent when unknown or refused.
                let by = Some(ctx.actor.clone())
                    .filter(|a| a.fields().tier != umx_core::provenance::Tier::Unknown);
                // An open (ws009 M1b1): each graph checked in services, a secured one always refused (S1); the reply
                // names the graphs sent and the ones refused, never anything from inside a graph. `sent`, not
                // opened: the view opens at most 12 and counts the rest (ui-expert N1). None sent: every
                // graph's refusal, in order (architect).
                if let UiCmd::Open { .. } = c {
                    let outcome = match ui.open(store, core_cmd(c), by) {
                        Ok(done) => done,
                        Err(problems) => return verbs::batch_errors(command, &problems),
                    };
                    self.ui_wake.notify_all();
                    return verbs::reply(command, Ok(outcome), |o| {
                        let sent = match &o.event.command.action {
                            UiAction::Open { graphs, .. } => graphs.clone(),
                            _ => Vec::new(),
                        };
                        let refused: Vec<Value> = o
                            .refused
                            .iter()
                            .map(|(graph, kind)| json!({ "graph": graph, "kind": kind }))
                            .collect();
                        json!({
                            "seq": o.event.seq, "graph": o.event.graph, "command": wire_json(&o.event.command),
                            "sent": sent, "refused": refused,
                        })
                    });
                }
                let outcome = ui.command(store, &ctx, core_cmd(c), by);
                if outcome.is_ok() {
                    self.ui_wake.notify_all();
                }
                verbs::reply(
                    command,
                    outcome,
                    |e| json!({ "seq": e.seq, "graph": e.graph, "command": wire_json(&e.command) }),
                )
            }
            Op::UiSelect { nodes } => verbs::reply(
                command,
                ui.select(store, &ctx, nodes),
                |s| json!({ "graph": s.graph, "selected": s.nodes }),
            ),
            Op::UiSelection => verbs::reply(command, ui.selection(store, &ctx), |rows| {
                // Plan 179 (as F188): a row whose title umx's secret check flags names it in `secret_suspected`; the
                // column shows only when a row has some, as on node list.
                let flagged = rows.iter().any(|r| !r.secret_suspected.is_empty());
                let rows = rows
                    .iter()
                    .map(|r| {
                        let mut row = json!({
                            "graph": r.graph, "id": r.id.to_text(), "address": umx_services::address(&r.graph, r.id),
                            "type": r.kind, "title": r.title, "title_cut": r.cut,
                        });
                        if !r.secret_suspected.is_empty() {
                            row["secret_suspected"] = json!(r.secret_suspected);
                        }
                        row
                    })
                    .collect();
                let mut columns = vec!["graph", "id", "address", "type", "title", "title_cut"];
                if flagged {
                    columns.push("secret_suspected");
                }
                verbs::list(&columns, rows, None)
            }),
            _ => Envelope::fail(command, internal("not_a_ui_op")),
        }
    }

    /// The view's long poll: answers at once with a reset, the events after `since` for its graphs, or what changed
    /// in them since `revs` (S5a), else sleeps until one comes, the daemon closes, or `timeout_ms` passes. At most
    /// four at once (`ui_waiters_busy`), and one `supervisor` wait apart (ws009 M1b2); each holds one of the 64
    /// connections while it sleeps. A wait whose client hung up ends at once (ws009 M1b2): every [`HANGUP_CHECK`]
    /// it asks [`client_gone`], so a view that reconnects never counts twice against the limit.
    pub(crate) fn ui_wait(
        &self,
        epoch: Option<&str>,
        since: u64,
        timeout_ms: u64,
        graphs: &[String],
        revs: &std::collections::BTreeMap<String, u64>,
        supervisor: bool,
    ) -> Envelope {
        let command = "ui wait";
        // A wait refused as busy first wakes every sleeping wait, so one whose client already hung up (a view that
        // reconnected) looks at once and frees its slot, then asks again for a moment.
        let asked = Instant::now();
        let begun = loop {
            // No services lock: a wait reads no graph, so it never queues behind the writer.
            let begun = {
                let mut ui = self.ui.lock().unwrap_or_else(|e| e.into_inner());
                ui.begin_wait(epoch, since, timeout_ms, graphs, revs, supervisor)
            };
            match begun {
                Err(p) if p.code == "ui_waiters_busy" && asked.elapsed() < BUSY_RECHECK => {
                    self.ui_wake.notify_all();
                    std::thread::sleep(Duration::from_millis(10));
                }
                other => break other,
            }
        };
        let waiter = match begun {
            Ok(w) => w,
            Err(problem) => return Envelope::fail(command, verbs::issue(&problem, command)),
        };
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let mut ui = self.ui.lock().unwrap_or_else(|e| e.into_inner());
        let answer = loop {
            if let Some(answer) = ui.poll(&waiter) {
                break answer;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break ui.timed_out(&waiter);
            }
            ui = match self.ui_wake.wait_timeout(ui, left.min(HANGUP_CHECK)) {
                Ok((guard, _)) => guard,
                Err(e) => e.into_inner().0,
            };
            if client_gone() {
                // Nobody reads the answer; the slot is free again now, not when the hold ends.
                break ui.timed_out(&waiter);
            }
        };
        ui.end_wait(&waiter);
        drop(ui);
        Envelope::ok(command, waited_json(&answer))
    }

    /// Before a stop or restart (F64): every view's wait answers at once, empty, and the daemon waits (at most
    /// [`QUIESCE`]) until each reply is written, so a view sees an answer rather than a cut connection.
    pub(crate) fn ui_quiesce(&self) {
        self.ui.lock().unwrap_or_else(|e| e.into_inner()).close();
        self.ui_wake.notify_all();
        let deadline = Instant::now() + QUIESCE;
        while self.ui_replies.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// How often a sleeping wait looks whether its client hung up.
pub(crate) const HANGUP_CHECK: Duration = Duration::from_millis(250);

/// How long a wait refused as busy keeps asking while the sleeping waits look at their clients.
const BUSY_RECHECK: Duration = Duration::from_millis(300);

thread_local! {
    /// The connection the `ui wait` on this thread answers, set by `Daemon::serve` for that request only (a wait
    /// answered in a test has none, and never looks).
    pub(crate) static WAIT_CLIENT: std::cell::RefCell<Option<std::os::unix::net::UnixStream>> =
        const { std::cell::RefCell::new(None) };
}

/// Whether the client of this thread's wait hung up: one space goes to it, which JSON readers skip before the reply,
/// and a closed peer refuses it (EPIPE) at once, while a client that only shut its write half takes it. A slow
/// reader is never mistaken for gone: a full buffer only times out.
pub(crate) fn client_gone() -> bool {
    use std::io::{ErrorKind, Write};
    WAIT_CLIENT.with(|c| {
        c.borrow_mut().as_mut().is_some_and(|s| {
            let _ = s.set_write_timeout(Some(Duration::from_millis(20)));
            match s.write_all(b" ") {
                Ok(_) => false,
                Err(e) => !matches!(
                    e.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ),
            }
        })
    })
}

/// The wire command as the agent sent it, in umx-core's terms (node references still as typed).
pub(crate) fn core_cmd(c: &UiCmd) -> CoreCmd<String> {
    let (action, note) = match c {
        UiCmd::Focus {
            node,
            hops,
            note,
            centre,
            select,
        } => (
            UiAction::Focus {
                node: node.clone(),
                hops: *hops,
                centre: *centre,
                select: *select,
            },
            note,
        ),
        UiCmd::Highlight { nodes, add, note } => (
            UiAction::Highlight {
                nodes: nodes.clone(),
                add: *add,
            },
            note,
        ),
        UiCmd::Filter { show, hide, note } => (
            UiAction::Filter {
                show: show.clone(),
                hide: hide.clone(),
            },
            note,
        ),
        UiCmd::Fit { nodes, note } => (
            UiAction::Fit {
                nodes: nodes.clone(),
            },
            note,
        ),
        UiCmd::Clear { note } => (UiAction::Clear, note),
        // A sender's left_out is never taken: services counts what it refused.
        UiCmd::Open {
            graphs,
            only,
            note,
            raise,
            ..
        } => (
            UiAction::Open {
                graphs: graphs.clone(),
                only: *only,
                left_out: 0,
                raise: *raise,
            },
            note,
        ),
    };
    CoreCmd {
        action,
        note: note.clone(),
    }
}

/// An event's command in the wire's own shape, node ids in place of what was typed.
fn wire(c: &CoreCmd) -> UiCmd {
    let note = c.note.clone();
    match &c.action {
        UiAction::Focus {
            node,
            hops,
            centre,
            select,
        } => UiCmd::Focus {
            node: node.to_text(),
            hops: *hops,
            note,
            centre: *centre,
            select: *select,
        },
        UiAction::Highlight { nodes, add } => UiCmd::Highlight {
            nodes: ids(nodes),
            add: *add,
            note,
        },
        UiAction::Filter { show, hide } => UiCmd::Filter {
            show: show.clone(),
            hide: hide.clone(),
            note,
        },
        UiAction::Fit { nodes } => UiCmd::Fit {
            nodes: ids(nodes),
            note,
        },
        UiAction::Clear => UiCmd::Clear { note },
        UiAction::Open {
            graphs,
            only,
            left_out,
            raise,
        } => UiCmd::Open {
            graphs: graphs.clone(),
            only: *only,
            note,
            left_out: *left_out,
            raise: *raise,
        },
    }
}

fn ids(nodes: &[umx_core::Id]) -> Vec<String> {
    nodes.iter().map(|n| n.to_text()).collect()
}

fn wire_json(c: &CoreCmd) -> Value {
    serde_json::to_value(wire(c)).expect("a ui command serialises")
}

fn event_json(e: &UiEvent) -> Value {
    let mut v = json!({ "seq": e.seq, "at_ms": e.at_ms, "graph": e.graph, "command": wire_json(&e.command) });
    if let Some(by) = &e.by {
        v["by"] = serde_json::to_value(by).expect("an actor serialises");
    }
    v
}

fn ids_json(ids: &TouchedIds) -> Value {
    let text = |s: &std::collections::BTreeSet<umx_core::Id>| {
        s.iter().map(|i| i.to_text()).collect::<Vec<_>>()
    };
    json!({ "added": text(&ids.added), "edited": text(&ids.edited), "removed": text(&ids.removed) })
}

/// One graph's change (ws009 S5a): ids and revisions only, never text or who wrote it (security T2).
fn change_json(c: &Change) -> Value {
    match c {
        Change::Changed {
            graph,
            from_rev,
            to_rev,
            touched,
        } => json!({
            "graph": graph, "from_rev": from_rev, "to_rev": to_rev,
            "nodes": ids_json(&touched.nodes), "edges": ids_json(&touched.edges),
        }),
        Change::Overflow { graph, rev } => json!({ "graph": graph, "rev": rev, "overflow": true }),
    }
}

fn waited_json(w: &UiWaited) -> Value {
    json!({
        "epoch": w.epoch, "seq": w.seq, "reset": w.reset,
        "events": w.events.iter().map(event_json).collect::<Vec<_>>(),
        "changes": w.changes.iter().map(change_json).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PRD 005: `umx ui focus --centre` reaches the view's event as sent, and a plain focus stays plain on the wire.
    #[test]
    fn focus_carries_centre_from_the_request_to_the_event() {
        for centre in [true, false] {
            let sent = UiCmd::Focus {
                node: "01JC8XA4H7ABCDEFGHJKMNPQRS".into(),
                hops: 2,
                note: None,
                centre,
                select: false,
            };
            let resolved = core_cmd(&sent)
                .map(|n| n.parse::<umx_core::Id>())
                .expect("an id");
            assert_eq!(wire(&resolved), sent);
            assert_eq!(wire_json(&resolved).get("centre").is_some(), centre);
        }
    }
}

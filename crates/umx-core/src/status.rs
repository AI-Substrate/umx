//! Status moves (req-12, workshop 007 section 5): the movement label and the record kept in `status_history`.

use alloc::string::String;
use serde_json::{Value, json};

/// Labels a status move may carry.
pub const LABELS: [&str; 3] = ["resolved", "in_contention", "back_on_table"];

/// The label inferred from a move: contested or open to agreed is `resolved`; agreed to contested is
/// `in_contention`; ruled_out to open or proposed is `back_on_table`; any other move has none.
pub fn inferred_label(from: &str, to: &str) -> Option<&'static str> {
    match (from, to) {
        ("contested" | "open", "agreed") => Some("resolved"),
        ("agreed", "contested") => Some("in_contention"),
        ("ruled_out", "open" | "proposed") => Some("back_on_table"),
        _ => None,
    }
}

/// One status move as stored on the node (one record line, so merges stay line-local).
#[derive(Clone, Debug, PartialEq)]
pub struct StatusMove {
    pub at: String,
    pub by: String,
    pub from: String,
    pub to: String,
    pub label: Option<String>,
    pub reason: Option<String>,
    /// Where the move happened (ws008 TM6, D71): the anchor's address, `umx:<graph id>/<node id>`, stored as `in`.
    pub place: Option<String>,
    /// The evidence pointer the move cites (ws008 TM6), as a node's evidence pointer is stored.
    pub evidence: Option<Value>,
}

impl StatusMove {
    /// The stored entry. `in` and `evidence` are written only when given, so a move without them is stored byte for
    /// byte as before, and an older umx keeps them (status_history entries are kept as written).
    pub fn to_value(&self) -> Value {
        let mut v = json!({
            "at": self.at,
            "by": self.by,
            "from": self.from,
            "to": self.to,
            "label": self.label,
            "reason": self.reason,
        });
        if let Some(place) = &self.place {
            v["in"] = Value::String(place.clone());
        }
        if let Some(evidence) = &self.evidence {
            v["evidence"] = evidence.clone();
        }
        v
    }

    /// Reads a stored entry back; entries written by hand with missing parts read as empty text.
    pub fn from_value(v: &Value) -> StatusMove {
        let text = |k: &str| v.get(k).and_then(Value::as_str).map(String::from);
        StatusMove {
            at: text("at").unwrap_or_default(),
            by: text("by").unwrap_or_default(),
            from: text("from").unwrap_or_default(),
            to: text("to").unwrap_or_default(),
            label: text("label"),
            reason: text("reason"),
            place: text("in"),
            evidence: v.get("evidence").cloned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_follow_workshop_007() {
        assert_eq!(inferred_label("open", "agreed"), Some("resolved"));
        assert_eq!(inferred_label("contested", "agreed"), Some("resolved"));
        assert_eq!(inferred_label("agreed", "contested"), Some("in_contention"));
        assert_eq!(inferred_label("ruled_out", "open"), Some("back_on_table"));
        assert_eq!(
            inferred_label("ruled_out", "proposed"),
            Some("back_on_table")
        );
        assert_eq!(inferred_label("open", "proposed"), None);
        assert_eq!(inferred_label("agreed", "agreed"), None);
    }

    #[test]
    fn a_move_round_trips_through_its_stored_value() {
        let m = StatusMove {
            at: "2026-11-09T05:50:12Z".into(),
            by: "tester".into(),
            from: "open".into(),
            to: "agreed".into(),
            label: Some("resolved".into()),
            reason: None,
            place: None,
            evidence: None,
        };
        let v = m.to_value();
        assert_eq!(v["reason"], Value::Null);
        assert_eq!(StatusMove::from_value(&v), m);
        // ws008 TM6: a move without a place or evidence has no such keys, so older entries keep their bytes.
        assert!(v.get("in").is_none() && v.get("evidence").is_none());
        let placed = StatusMove {
            place: Some("umx:01JC8XA4H7B3KT9M2NZ5QW6RDE/01JC8XB2K9B3KT9M2NZ5QW6RDE".into()),
            evidence: Some(json!({"pointer": "notes.md#L1", "kind": "room"})),
            ..m
        };
        let v = placed.to_value();
        assert_eq!(v["in"], placed.place.clone().unwrap().as_str());
        assert_eq!(StatusMove::from_value(&v), placed);
    }
}

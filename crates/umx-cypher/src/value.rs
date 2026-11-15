use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cmp::Ordering;
use core::fmt::Write;

/// A float with a total order (`f64::total_cmp`) and the matching equality and hash, so
/// [`CypherValue`] keeps `Eq`, `Ord` and `Hash` (R-Q6). NaN cannot come from graph files.
#[derive(Debug, Clone, Copy)]
pub struct Float(pub f64);

impl PartialEq for Float {
    fn eq(&self, other: &Self) -> bool {
        self.0.total_cmp(&other.0).is_eq()
    }
}

impl Eq for Float {}

impl PartialOrd for Float {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Float {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

impl core::hash::Hash for Float {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}

/// A scalar or composite value produced by evaluating a Cypher expression or projecting a result.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CypherValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(Float),
    Str(String),
    /// A graph node. `id` is the provider's stable, opaque identity for the node (used for equality
    /// and grouping, never displayed); `label` and `name` are what gets rendered.
    Node {
        id: String,
        label: String,
        name: String,
    },
    /// A graph relationship (edge). `id` is the provider's stable, opaque identity for the edge; `label`
    /// is its type.
    Relationship {
        id: String,
        label: String,
    },
    List(Vec<CypherValue>),
    /// An ordered map, e.g. from a map projection `n { .name, k: expr }`.
    Map(Vec<(String, CypherValue)>),
}

impl CypherValue {
    /// Returns the truthiness of a value for use in `WHERE` filtering.
    /// `NULL` and `false` are falsy; everything else (including bound nodes) is truthy.
    #[must_use]
    pub fn is_truthy(&self) -> bool {
        match self {
            CypherValue::Null => false,
            CypherValue::Bool(b) => *b,
            _ => true,
        }
    }

    /// Returns a numeric view of the value if it is an integer.
    #[must_use]
    pub fn as_int(&self) -> Option<i64> {
        match self {
            CypherValue::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// Returns a numeric view of an integer or a float, as f64.
    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            // Precision past 2^53 is not a concern for graph field values.
            #[allow(clippy::cast_precision_loss)]
            CypherValue::Int(i) => Some(*i as f64),
            CypherValue::Float(f) => Some(f.0),
            _ => None,
        }
    }

    /// Returns a string view of the value if it is a string.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            CypherValue::Str(s) => Some(s),
            _ => None,
        }
    }

    /// Rank of each variant for cross-type ordering, following openCypher orderability:
    /// `Map < Node < Relationship < List < String < Boolean < Number < null` (ascending). `null`
    /// sorts last; strings sort before numbers.
    fn type_rank(&self) -> u8 {
        match self {
            CypherValue::Map(_) => 0,
            CypherValue::Node { .. } => 1,
            CypherValue::Relationship { .. } => 2,
            CypherValue::List(_) => 3,
            CypherValue::Str(_) => 4,
            CypherValue::Bool(_) => 5,
            CypherValue::Int(_) | CypherValue::Float(_) => 6,
            CypherValue::Null => 7,
        }
    }

    /// Total ordering across all value types, following openCypher orderability. Values of the same
    /// type are ordered by value; values of differing types by [`Self::type_rank`], so `null` sorts
    /// last and strings sort before numbers. (Nodes are ordered by name rather than identity.)
    #[must_use]
    pub fn total_cmp(&self, other: &CypherValue) -> Ordering {
        match (self, other) {
            (CypherValue::Bool(a), CypherValue::Bool(b)) => a.cmp(b),
            (CypherValue::Int(a), CypherValue::Int(b)) => a.cmp(b),
            // Integers and floats compare as numbers (`3 = 3.0`); a NaN would sort by total order.
            (
                CypherValue::Int(_) | CypherValue::Float(_),
                CypherValue::Int(_) | CypherValue::Float(_),
            ) => {
                let (a, b) = (self.as_f64().unwrap_or(0.0), other.as_f64().unwrap_or(0.0));
                a.partial_cmp(&b).unwrap_or_else(|| a.total_cmp(&b))
            }
            (CypherValue::Str(a), CypherValue::Str(b))
            | (CypherValue::Node { name: a, .. }, CypherValue::Node { name: b, .. }) => a.cmp(b),
            (
                CypherValue::Relationship {
                    label: la, id: ia, ..
                },
                CypherValue::Relationship {
                    label: lb, id: ib, ..
                },
            ) => la.cmp(lb).then_with(|| ia.cmp(ib)),
            (CypherValue::List(a), CypherValue::List(b)) => {
                for (x, y) in a.iter().zip(b.iter()) {
                    let ordering = x.total_cmp(y);
                    if ordering != Ordering::Equal {
                        return ordering;
                    }
                }
                a.len().cmp(&b.len())
            }
            (CypherValue::Map(a), CypherValue::Map(b)) => {
                for ((ak, av), (bk, bv)) in a.iter().zip(b.iter()) {
                    let ordering = ak.cmp(bk).then_with(|| av.total_cmp(bv));
                    if ordering != Ordering::Equal {
                        return ordering;
                    }
                }
                a.len().cmp(&b.len())
            }
            _ => self.type_rank().cmp(&other.type_rank()),
        }
    }

    /// Renders the value for display in a plain-text table cell.
    #[must_use]
    pub fn to_display_string(&self) -> String {
        match self {
            CypherValue::Null => String::new(),
            CypherValue::Bool(b) => b.to_string(),
            CypherValue::Int(i) => i.to_string(),
            CypherValue::Float(f) => format!("{:?}", f.0),
            CypherValue::Str(s) => s.clone(),
            CypherValue::Node { name, .. } => name.clone(),
            CypherValue::Relationship { label, .. } => format!("[:{label}]"),
            CypherValue::List(items) => {
                let rendered: Vec<String> =
                    items.iter().map(CypherValue::to_display_string).collect();
                format!("[{}]", rendered.join(", "))
            }
            CypherValue::Map(entries) => {
                let rendered: Vec<String> = entries
                    .iter()
                    .map(|(key, value)| format!("{key}: {}", value.to_display_string()))
                    .collect();
                format!("{{{}}}", rendered.join(", "))
            }
        }
    }

    /// Renders the value as a JSON fragment, appending to `out`.
    pub fn write_json(&self, out: &mut String) {
        match self {
            CypherValue::Null => out.push_str("null"),
            CypherValue::Bool(b) => {
                let _ = write!(out, "{b}");
            }
            CypherValue::Int(i) => {
                let _ = write!(out, "{i}");
            }
            // Full precision; JSON has no NaN or infinity, so those are null.
            CypherValue::Float(f) if f.0.is_finite() => {
                let _ = write!(out, "{:?}", f.0);
            }
            CypherValue::Float(_) => out.push_str("null"),
            CypherValue::Str(s) => write_json_string(out, s),
            CypherValue::Node { label, name, .. } => {
                out.push_str("{\"label\":");
                write_json_string(out, label);
                out.push_str(",\"name\":");
                write_json_string(out, name);
                out.push('}');
            }
            CypherValue::Relationship { label, .. } => {
                out.push_str("{\"type\":");
                write_json_string(out, label);
                out.push('}');
            }
            CypherValue::List(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    item.write_json(out);
                }
                out.push(']');
            }
            CypherValue::Map(entries) => {
                out.push('{');
                for (index, (key, value)) in entries.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    write_json_string(out, key);
                    out.push(':');
                    value.write_json(out);
                }
                out.push('}');
            }
        }
    }
}

/// Escapes and quotes a string as a JSON string literal.
pub fn write_json_string(out: &mut String, value: &str) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

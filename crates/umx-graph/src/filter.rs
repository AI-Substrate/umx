//! `--where <field><op><value>` (workshop 007 rule 5). Parsing splits the text; the caller types the value by the
//! field's kind. A node that lacks the field never matches, for every operator. On a list field (ws008 E1) `=`
//! matches when any element equals the value and `!=` when none does; the ordering operators never match a list.

use crate::NodeRef;
use alloc::string::{String, ToString};
use core::cmp::Ordering;
use umx_core::Value;
use umx_core::name::is_snake_case;
use umx_core::value::compare_values;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Where {
    pub field: String,
    pub op: Op,
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WhereError(pub &'static str);

/// Splits `likelihood>=3` into the field, the operator and the raw value text.
pub fn parse_where(expr: &str) -> Result<(String, Op, String), WhereError> {
    let at = expr
        .find(['=', '!', '<', '>'])
        .ok_or(WhereError("no operator (= != < <= > >=)"))?;
    let (field, rest) = expr.split_at(at);
    let field = field.trim();
    let (op, len) = match rest.get(..2) {
        Some("!=") => (Op::Ne, 2),
        Some("<=") => (Op::Le, 2),
        Some(">=") => (Op::Ge, 2),
        _ => match &rest[..1] {
            "=" => (Op::Eq, 1),
            "<" => (Op::Lt, 1),
            ">" => (Op::Gt, 1),
            _ => return Err(WhereError("no operator (= != < <= > >=)")),
        },
    };
    if !is_snake_case(field) {
        return Err(WhereError("the field must be a snake_case name"));
    }
    Ok((field.to_string(), op, rest[len..].trim().to_string()))
}

impl Where {
    pub fn matches(&self, node: NodeRef<'_>) -> bool {
        node.field(&self.field)
            .is_some_and(|actual| self.test(&actual))
    }

    /// The value as a number, when it is one. A number held in a column is then compared as f64: for two numbers
    /// `compare_values` compares their f64s, and `==` implies equal f64s, so every operator is the f64 comparison.
    pub(crate) fn number(&self) -> Option<f64> {
        self.value.as_f64()
    }

    /// Whether a node whose field holds `actual` matches.
    pub(crate) fn test(&self, actual: &Value) -> bool {
        // entity_key compares stored forms on both sides (ws008 E1): the keys the index holds, so a hand-edited
        // `email:Ana@X.com` is found by `email:ana@x.com`.
        let stored;
        let actual = if self.field == umx_core::entity::ENTITY_KEY && actual.is_array() {
            stored = Value::Array(
                umx_core::entity::stored_keys(actual)
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            );
            &stored
        } else {
            actual
        };
        if let Value::Array(items) = actual
            && !self.value.is_array()
        {
            let any = items.iter().any(|item| {
                *item == self.value || compare_values(item, &self.value) == Some(Ordering::Equal)
            });
            return match self.op {
                Op::Eq => any,
                Op::Ne => !any,
                _ => false,
            };
        }
        match self.op {
            Op::Eq => {
                *actual == self.value
                    || compare_values(actual, &self.value) == Some(Ordering::Equal)
            }
            Op::Ne => {
                *actual != self.value
                    && compare_values(actual, &self.value) != Some(Ordering::Equal)
            }
            Op::Lt => compare_values(actual, &self.value) == Some(Ordering::Less),
            Op::Le => matches!(
                compare_values(actual, &self.value),
                Some(Ordering::Less | Ordering::Equal)
            ),
            Op::Gt => compare_values(actual, &self.value) == Some(Ordering::Greater),
            Op::Ge => matches!(
                compare_values(actual, &self.value),
                Some(Ordering::Greater | Ordering::Equal)
            ),
        }
    }
}

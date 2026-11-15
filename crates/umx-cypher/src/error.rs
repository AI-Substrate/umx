use alloc::string::String;
use core::fmt;

/// Which limit stopped a query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// The work budget (units of work charged).
    Work,
    /// The cap on rows held at once.
    Rows,
}

/// An error produced while lexing, parsing, or executing a Cypher query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CypherError {
    /// A lexing or parsing error, with a byte position into the source query.
    Syntax { message: String, position: usize },
    /// A semantic or execution error (e.g. unknown property, type mismatch).
    Execution { message: String },
    /// The query went past a limit and was stopped; it returns no rows.
    TooLarge { cause: Limit, used: u64 },
    /// The caller's cancel hook stopped the query (a wall-clock backstop); it returns no rows.
    Cancelled { used: u64 },
    /// A directed step (`->` or `<-`) names a symmetric relation, which has no direction (D49 R1):
    /// refused, never answered with whichever half happens to be stored that way round.
    SymmetricDirection { relation: String },
}

impl CypherError {
    pub fn syntax(message: impl Into<String>, position: usize) -> Self {
        Self::Syntax {
            message: message.into(),
            position,
        }
    }

    pub fn execution(message: impl Into<String>) -> Self {
        Self::Execution {
            message: message.into(),
        }
    }
}

impl fmt::Display for CypherError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CypherError::Syntax { message, position } => {
                write!(f, "Cypher syntax error at position {position}: {message}")
            }
            CypherError::Execution { message } => {
                write!(f, "Cypher execution error: {message}")
            }
            CypherError::TooLarge { cause, used } => write!(
                f,
                "the query was stopped at its {} limit after {used} work units",
                match cause {
                    Limit::Work => "work",
                    Limit::Rows => "rows",
                }
            ),
            CypherError::Cancelled { used } => {
                write!(f, "the query was cancelled after {used} work units")
            }
            CypherError::SymmetricDirection { relation } => write!(
                f,
                "`{relation}` is symmetric, so it has no direction: match it with -[:{relation}]-"
            ),
        }
    }
}

impl core::error::Error for CypherError {}

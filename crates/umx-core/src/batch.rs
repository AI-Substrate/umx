//! `umx batch` (req-88, D48): the caps every side checks, and what a batch-local ref may be. The CLI cannot depend
//! on this crate (r-cli-is-a-client), so umx-protocol mirrors the caps; umx-daemon's test keeps the two equal.

/// The most bytes the CLI reads from a batch file or stdin: half of the 16 MiB frame, leaving room for the
/// request around the ops.
pub const MAX_BATCH_BYTES: u64 = 8 << 20;
/// The most ops one batch may hold. The CLI checks it before sending and the services again on arrival.
pub const MAX_BATCH_OPS: usize = 10_000;
/// The longest line of a batch file, in bytes.
pub const MAX_BATCH_LINE: usize = 1 << 20;
/// The most line errors a failed batch reports; past it one `batch_errors_truncated` gives the total.
pub const MAX_BATCH_ERRORS: usize = 100;
/// The most warnings a batch reports; past it one `batch_warnings_truncated` gives the total.
pub const MAX_BATCH_WARNINGS: usize = 100;
/// The most edge sentences an opted-in batch reports.
pub const MAX_ECHO_SENTENCES: usize = 100;
/// The most Unicode characters in one batch sentence.
pub const MAX_ECHO_SENTENCE_CHARS: usize = 1024;
/// The most candidates one batch problem lists (an ambiguous prefix can match many nodes).
pub const MAX_BATCH_CANDIDATES: usize = 10;
/// The most bytes a batch's reply may take, checked on an upper bound before anything is written, so a batch
/// whose reply the client could not read is refused instead of committed (review R103-1). Set under the old 16 MiB
/// reply cap and kept: clients now read up to umx-protocol's MAX_REPLY (256 MiB, ws004 s34a).
pub const MAX_BATCH_REPLY: u64 = 12 << 20;
/// The longest ref.
pub const MAX_REF_LEN: usize = 32;
pub const MAX_BATCH_EVIDENCE: usize = 32;
/// The most refs one batch may import with `--refs-from`, counted across every receipt it names (ws003 s24).
pub const MAX_IMPORTED_REFS: usize = 10_000;

/// Why a ref is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefError {
    /// An empty ref.
    Syntax,
    TooLong,
    Character {
        character: char,
        position: usize,
    },
}

/// Checks a batch-local ref: `[a-z][a-z0-9_-]{0,31}`. Refs use separate `_ref` fields, so their spelling can
/// never be confused with an id in a plain field.
pub fn check_ref(text: &str) -> Result<(), RefError> {
    if text.len() > MAX_REF_LEN {
        return Err(RefError::TooLong);
    }
    if text.is_empty() {
        return Err(RefError::Syntax);
    }
    for (index, character) in text.chars().enumerate() {
        let valid = if index == 0 {
            character.is_ascii_lowercase()
        } else {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '_' | '-')
        };
        if !valid {
            return Err(RefError::Character {
                character,
                position: index + 1,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    #[test]
    fn a_ref_is_short_lower_case_and_independent_of_id_spelling() {
        assert_eq!(
            check_ref(&format!("u{}", "_".repeat(31))),
            Ok(()),
            "32 is the longest"
        );
        assert_eq!(check_ref("q1"), Ok(()));
        assert_eq!(check_ref("risk_7"), Ok(()));
        assert_eq!(check_ref("opt-2"), Ok(()));
        assert_eq!(
            check_ref("ab12c"),
            Ok(()),
            "5 characters is below the prefix length"
        );
        for bad in [
            "",
            "Q1",
            "1q",
            "q 1",
            "q.1",
            "umx:g/q1",
            "_q",
            &"q".repeat(33),
        ] {
            assert!(check_ref(bad).is_err(), "{bad}");
        }
        for fine in [
            "estate", "contract", "workshop", "staffing", "deadbeef", "abcdef", "answer", "q1q1q1",
            "zzzzzz", "jc8xa4h7", "hello1", "unique", "answer_1", "risk-14a", "q1_q1q1",
        ] {
            assert_eq!(check_ref(fine), Ok(()), "{fine}");
        }
    }
}

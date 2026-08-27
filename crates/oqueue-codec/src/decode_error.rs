//! What a decode refused, and why.
//!
//! ⚠️ **Its own module because a refusal is a contract, not a detail.** Every
//! variant here is something a peer can make happen by sending bytes, so each
//! one is a statement about what this broker will not accept — and the
//! distinctions between them are the ones an operator reads: a truncated read
//! is not an oversized frame, and a ceiling the caller chose is not a floor the
//! *format* fixes. `wire.rs` is the bounds-checked walk that raises these;
//! keeping the two apart is what stopped that file growing past the limit.

/// Why a decode stopped. Carries what a debugger needs: how much was asked
/// for, how much existed, where.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// The input ended before the value did.
    UnexpectedEof {
        /// Bytes the read needed.
        needed: usize,
        /// Bytes that remained.
        remaining: usize,
        /// Byte offset the read started at.
        at: usize,
    },
    /// A length field exceeds the bound the caller stated for it.
    ///
    /// ⚠️ The bound is the caller's claim about what is sane (a frame cap, a
    /// batch cap), not the buffer's size — `UnexpectedEof` covers running
    /// out of bytes. Separating them is what lets an operator tell "peer
    /// sent a 2 GiB frame" from "the read was truncated".
    LengthOutOfBounds {
        /// The length the peer claimed.
        length: u64,
        /// The caller's stated maximum.
        max: u64,
        /// Byte offset of the length field.
        at: usize,
    },
    /// A record declared a body shorter than the format's own minimum.
    ///
    /// ⚠️ **Separate from [`LengthOutOfBounds`](DecodeError::LengthOutOfBounds)
    /// because that one is a *ceiling*** — the caller's claim about what is
    /// sane. This is a floor the record format itself fixes: a record body
    /// carries attributes and five varints before any key or value, so
    /// anything shorter cannot be a record whatever the caller permits.
    RecordTooShort {
        /// The body length the peer declared.
        length: u64,
        /// The shortest body the format allows.
        min: u64,
        /// Byte offset of the length field.
        at: usize,
    },
    /// A length field is negative where the caller said null is not legal.
    NegativeLength {
        /// The value as sent.
        length: i32,
        /// Byte offset of the length field.
        at: usize,
    },
    /// A varint's continuation bits ran past the widest legal encoding.
    VarintTooLong {
        /// The maximum bytes this varint width may span.
        max_bytes: usize,
        /// Byte offset the varint started at.
        at: usize,
    },
    /// A string field's bytes are not valid UTF-8. Kafka strings are UTF-8;
    /// a malformed one is as unusable as a malformed length.
    InvalidUtf8 {
        /// Byte offset the string's bytes started at.
        at: usize,
    },
    /// A field is null where the caller said null is not legal — the
    /// compact encoding's `0` length, distinct from a negative i32 length.
    UnexpectedNull {
        /// Byte offset of the length field.
        at: usize,
    },
}

impl core::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnexpectedEof {
                needed,
                remaining,
                at,
            } => write!(
                f,
                "input ended: needed {needed} byte(s) at offset {at}, {remaining} remained"
            ),
            Self::LengthOutOfBounds { length, max, at } => write!(
                f,
                "length {length} at offset {at} exceeds the stated bound {max}"
            ),
            Self::RecordTooShort { length, min, at } => write!(
                f,
                "record body {length} at offset {at} is below the format's minimum {min}"
            ),
            Self::NegativeLength { length, at } => {
                write!(
                    f,
                    "negative length {length} at offset {at} where null is not legal"
                )
            }
            Self::VarintTooLong { max_bytes, at } => {
                write!(
                    f,
                    "varint at offset {at} continues past its maximum {max_bytes} byte(s)"
                )
            }
            Self::InvalidUtf8 { at } => {
                write!(f, "string at offset {at} is not valid UTF-8")
            }
            Self::UnexpectedNull { at } => {
                write!(f, "null at offset {at} where null is not legal")
            }
        }
    }
}

impl std::error::Error for DecodeError {}

#[cfg(test)]
mod tests {
    use super::DecodeError;

    /// ⚠️ **Every refusal names its own numbers**, and that is a contract
    /// rather than a nicety: these are the messages an operator reads when a
    /// peer is sending bytes this broker will not take, and a message that
    /// omits the length, the bound or the offset leaves them with nothing to
    /// act on (`error-handling.md` rule 13).
    ///
    /// ⚠️ **And the variants must not read alike.** The distinctions here are
    /// the ones worth telling apart — a truncated read from an oversized
    /// frame, a ceiling the caller chose from a floor the format fixes — so a
    /// `Display` that collapsed any two of them would erase exactly the
    /// information these variants exist to carry.
    #[test]
    fn every_refusal_says_which_numbers_made_it_one() {
        let mut seen: Vec<String> = Vec::new();
        for (error, numbers) in cases() {
            let rendered = error.to_string();
            for number in numbers {
                assert!(
                    rendered.contains(number),
                    "{error:?} rendered as {rendered:?} without {number}"
                );
            }
            assert!(
                !seen.contains(&rendered),
                "two refusals render alike: {rendered:?}"
            );
            seen.push(rendered);
        }
    }

    /// Every variant, with the numbers its message owes the reader.
    fn cases() -> Vec<(DecodeError, Vec<&'static str>)> {
        vec![
            (
                DecodeError::UnexpectedEof {
                    needed: 7,
                    remaining: 3,
                    at: 11,
                },
                vec!["7", "3", "11"],
            ),
            (
                DecodeError::LengthOutOfBounds {
                    length: 9,
                    max: 4,
                    at: 12,
                },
                vec!["9", "4", "12"],
            ),
            (
                DecodeError::RecordTooShort {
                    length: 2,
                    min: 6,
                    at: 13,
                },
                vec!["2", "6", "13"],
            ),
            (
                DecodeError::NegativeLength { length: -5, at: 14 },
                vec!["-5", "14"],
            ),
            (
                DecodeError::VarintTooLong {
                    max_bytes: 5,
                    at: 15,
                },
                vec!["5", "15"],
            ),
            (DecodeError::InvalidUtf8 { at: 16 }, vec!["16"]),
            (DecodeError::UnexpectedNull { at: 17 }, vec!["17"]),
        ]
    }
}

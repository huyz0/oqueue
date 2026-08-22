//! CRC-32C (Castagnoli), one-shot and combinable — the checksum Kafka's v2
//! `RecordBatch` mandates.
//!
//! ⚠️ **`crc-fast`, never `crc32fast`** (doc 18 §4.3): `crc32fast` implements
//! CRC-32/IEEE — a different polynomial — and using it produces batches every
//! Kafka client rejects **with no compile error anywhere**. The differential
//! test at the bottom of this file is what makes that mistake loud: a scalar
//! bit-by-bit Castagnoli reference that any wrong-polynomial dependency
//! diverges from on nearly every input.
//!
//! ⚠️ **[`crc32c_combine`] is the function that matters** (doc 18 §4.4):
//! `combine(crc(A), crc(B), len(B)) == crc(A ‖ B)` in O(log n), so merging,
//! splitting, or re-batching segments never recomputes a checksum over bytes
//! already summed. Doc 18 calls this worth more than any instruction tuning.
//!
//! ## Dispatch, and `OQUEUE_SIMD`
//!
//! SIMD dispatch (VPCLMULQDQ/AVX-512, NEON) is `crc-fast`'s own, resolved
//! once per process at first use and cached inside the dependency — nothing
//! here re-detects per call. On top of that sits the one override doc 18
//! §4.2 names as a deliverable: `OQUEUE_SIMD=scalar` forces the bit-by-bit
//! reference path, read **once** into a [`std::sync::OnceLock`], so a user
//! can reproduce a suspected corruption bug with the accelerated kernels out
//! of the equation. ⚠️ Not a threshold (non-negotiable 2 is about values
//! that gate checks): both paths compute the same function, the tests prove
//! it, and the override only trades speed for certainty.
//!
//! ## `unsafe`
//!
//! Budgeted for this crate (`check-unsafe.sh`) and **unused**: the SIMD
//! kernels live in `crc-fast`, whose `unsafe` is its own; every function
//! here is safe Rust over its safe API. `baselines/unsafe.txt` stays empty
//! of this crate until a measured kernel of our own earns an entry.

use crc_fast::CrcAlgorithm::Crc32Iscsi;
use std::sync::OnceLock;

/// `OQUEUE_SIMD=scalar` forces [`scalar_crc32c`]; anything else (or unset)
/// is `crc-fast`'s own dispatch. Read once — a per-call `env::var` would be
/// a syscall on the batch hot path and would let the path change mid-run.
fn force_scalar() -> bool {
    static FORCE_SCALAR: OnceLock<bool> = OnceLock::new();
    *FORCE_SCALAR.get_or_init(|| std::env::var("OQUEUE_SIMD").as_deref() == Ok("scalar"))
}

/// Bit-by-bit reflected CRC-32C — 0x82F63B78 is the reflected Castagnoli
/// polynomial. The `OQUEUE_SIMD=scalar` path, and the reference the
/// differential test drives against `crc-fast`; deliberately the dumbest
/// correct implementation, sharing no table and no code with the fast path.
fn scalar_crc32c(data: &[u8]) -> u32 {
    let mut crc: u32 = !0;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0x82F6_3B78
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// The CRC-32C of `data`, as Kafka's `RecordBatch` v2 stores it.
///
/// One shot. For summing a batch assembled from pieces, sum the pieces and
/// [`crc32c_combine`] them instead of concatenating buffers.
#[must_use]
pub fn crc32c(data: &[u8]) -> u32 {
    if force_scalar() {
        return scalar_crc32c(data);
    }
    // `crc-fast` returns `u64` for algorithm-genericity; a 32-bit algorithm
    // occupies the low half. The mask makes the truncation a statement
    // rather than a cast that clippy (rightly) distrusts.
    u32::try_from(crc_fast::checksum(Crc32Iscsi, data) & 0xFFFF_FFFF).unwrap_or(u32::MAX)
}

/// The CRC-32C of `A ‖ B`, given `crc32c(A)`, `crc32c(B)`, and `B`'s length —
/// without touching a byte of either.
///
/// O(log `len_b`). `len_b` is the byte length of the buffer behind `crc_b`;
/// getting it wrong yields a well-formed wrong answer, which is why the
/// combine property test below drives this with hundreds of random splits.
#[must_use]
pub fn crc32c_combine(crc_a: u32, crc_b: u32, len_b: u64) -> u32 {
    u32::try_from(
        crc_fast::checksum_combine(Crc32Iscsi, u64::from(crc_a), u64::from(crc_b), len_b)
            & 0xFFFF_FFFF,
    )
    .unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::{crc32c, crc32c_combine, scalar_crc32c};

    /// Deterministic pseudo-random bytes — xorshift64*, seeded per test so
    /// failures reproduce exactly. No RNG dependency for the same reason the
    /// scalar reference is hand-rolled: the test must not share machinery
    /// with anything it checks.
    fn pseudo_random_bytes(seed: u64, len: usize) -> Vec<u8> {
        let mut state = seed.max(1);
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                u8::try_from(state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 56).unwrap_or(0)
            })
            .collect()
    }

    #[test]
    fn known_answers_from_the_iscsi_test_vectors() {
        // RFC 3720 / iSCSI's published CRC-32C check values — the vectors
        // every implementation is validated against. A wrong polynomial
        // (crc32fast's IEEE) fails all four.
        assert_eq!(crc32c(b""), 0);
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
        assert_eq!(crc32c(&[0u8; 32]), 0x8A91_36AA);
        assert_eq!(crc32c(&[0xFFu8; 32]), 0x62A8_AB43);
        let ascending: Vec<u8> = (0u8..32).collect();
        assert_eq!(crc32c(&ascending), 0x46DD_794E);
    }

    #[test]
    fn differential_against_the_scalar_reference() {
        // Lengths chosen to cross every dispatch tier: sub-word, one block,
        // the odd tails SIMD kernels get wrong, and multi-kilobyte runs.
        for (i, len) in [0, 1, 7, 8, 15, 63, 64, 65, 255, 1024, 4096, 4099]
            .into_iter()
            .enumerate()
        {
            let data = pseudo_random_bytes(0xDEAD_BEEF + u64::try_from(i).unwrap_or(0), len);
            assert_eq!(
                crc32c(&data),
                scalar_crc32c(&data),
                "diverged from the scalar Castagnoli reference at len {len}"
            );
        }
    }

    #[test]
    fn combine_equals_the_checksum_of_the_concatenation() {
        // Hundreds of random splits — a wrong len_b or a wrong combine
        // matrix produces a well-formed wrong answer only a property like
        // this notices. The split arithmetic reaches split==0 once and
        // split==len never, which is why the dedicated empty-right test
        // below exists and is not redundant.
        for case in 0usize..200 {
            let seed = 0xC0FF_EE00 + u64::try_from(case).unwrap_or(0);
            let whole = pseudo_random_bytes(seed, 1 + (case * 37) % 2048);
            let split = (case * 131) % (whole.len() + 1);
            let (a, b) = whole.split_at(split);
            let combined = crc32c_combine(
                crc32c(a),
                crc32c(b),
                u64::try_from(b.len()).unwrap_or(u64::MAX),
            );
            assert_eq!(
                combined,
                crc32c(&whole),
                "combine diverged at len {} split {split}",
                whole.len()
            );
        }
    }

    #[test]
    fn combine_with_an_empty_right_half_is_the_left_crc() {
        let data = pseudo_random_bytes(42, 100);
        assert_eq!(crc32c_combine(crc32c(&data), crc32c(b""), 0), crc32c(&data));
    }
}

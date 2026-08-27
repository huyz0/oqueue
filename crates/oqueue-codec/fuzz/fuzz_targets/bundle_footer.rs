//! `parse_footer` over arbitrary bytes: no panic, no over-allocation.
//!
//! ⚠️ **A decoder over *stored* bytes, which is the same risk from a different
//! direction.** Every other target here drives bytes a peer sent; these are
//! bytes an object store returned — written by this broker, rewritten by
//! `M5`'s compaction, and torn by anything in between. `security.md` rule 3
//! (nothing reachable from stored bytes may panic) does not distinguish, and
//! the fields this walks are all attacker-influenced in the same way: a
//! region count, a footer length, a name length, and two `u64` byte ranges.
//!
//! ⚠️ **Three calls, because `object_size` selects a branch.** Passing the
//! tail's own length is the whole-object read `M3` does. Passing something
//! larger is the *suffix* read `M5` will do — the path
//! `Error::BundleTailTooShort` exists for, and the one where a wrong answer
//! tells a consumer that live records are permanently gone. Passing something
//! *smaller* is a caller with a wrong `ObjectMeta`, which `parse_footer`'s own
//! doc says is a wrong answer rather than a weaker check, and which is the
//! only call that reaches the `payload_end` underflow.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let size = data.len() as u64;
    // The whole object, as `M3`'s reader passes it.
    let _ = oqueue_core::parse_footer(data, size);
    // A suffix of a larger object, as `M5`'s will.
    let _ = oqueue_core::parse_footer(data, size.saturating_mul(4).saturating_add(1));
    // And an object the caller believes is smaller than the bytes it holds —
    // a wrong `ObjectMeta`, which `parse_footer`'s own doc says is a wrong
    // answer rather than a weaker check, and must still not be a panic.
    let _ = oqueue_core::parse_footer(data, size / 2);
});

// ⚠️ **Two seeds for `BundleTailTooShort`, because it has two sites.**
// `below-the-trailer` is four bytes: too short to hold the trailer at all, so
// `needed` is a *lower bound* — the field that would give the real one is
// among the missing bytes. `footer-past-the-tail` holds the trailer and
// declares a footer longer than the tail, which is the site `M3.27` added and
// the one whose `needed` is exact, being the number a caller re-reads with.
// Without seeds in both ranges the deterministic replay every run performs
// would cover neither, and the branch would be reached only after mutation.

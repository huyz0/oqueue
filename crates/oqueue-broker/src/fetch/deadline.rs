//! How long a fetch may park, and why the ceiling is where it is.
//!
//! ⚠️ **Its own module because a ceiling on a client's number is a decision,
//! not a helper.** `park.rs` is the loop; this is the one place the broker
//! disagrees with what the client asked for, and the argument for the number
//! it substitutes is longer than the arithmetic that applies it.

// ⚠️ `pub(crate)` inside a private module: `unreachable_pub` denies the `pub`
// clippy's `redundant_pub_crate` asks for.
#![allow(clippy::redundant_pub_crate)]

/// How long to park, from what the client asked for.
///
/// ⚠️ **Clamped at both ends.** A negative `max_wait_ms` is a client bug and
/// becomes no park at all rather than an arithmetic surprise; anything above
/// [`MAX_PARK_MS`] becomes that ceiling.
pub(crate) fn park_ms(max_wait_ms: i32) -> u64 {
    u64::from(max_wait_ms.clamp(0, MAX_PARK_MS).unsigned_abs())
}

/// The longest a fetch may be parked, whatever the client asks for.
///
/// ⚠️ **A ceiling on a client's number, which is the one place a broker gets
/// to disagree with it.** `max_wait_ms` is an `i32`, so a client may name
/// twenty-four days; a connection held that long survives the topic it was
/// reading. Kafka's own default is 500 ms and librdkafka's is 100 ms, so a
/// minute is far above anything a real client asks for and far below anything
/// that looks like a leak.
///
/// ⚠️ **It must stay below whatever [`ConnectionLimits::idle_timeout`] a
/// composer chooses**, and that is a real constraint rather than a
/// preference: a client that sends one `Fetch` and waits for the answer sends
/// nothing while it is parked, so the connection looks idle for the whole
/// park. A shorter idle timeout tears the connection down mid-poll — the
/// answer is still written, and then the peer is disconnected after every long
/// poll. `bin/oqueue` picks 120 s against this 60 s, and its own test says so.
///
/// [`ConnectionLimits::idle_timeout`]: crate::ConnectionLimits::idle_timeout
pub const MAX_PARK_MS: i32 = 60_000;

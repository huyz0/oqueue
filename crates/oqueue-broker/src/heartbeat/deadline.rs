//! How long a member's own silence is tolerated before eviction.
//!
//! ⚠️ Its own module for `join_group::deadline`'s own reason: a ceiling
//! this broker imposes on a client's own number is a decision, not a
//! helper.

/// `session_timeout_ms`, clamped — `fetch::deadline::park_ms`'s own shape.
/// A non-positive value is a client bug and becomes the floor rather than
/// an arithmetic surprise (evicting instantly, or never, from one signed
/// mistake); anything above [`MAX_SESSION_TIMEOUT_MS`] becomes that
/// ceiling.
pub(crate) fn clamp_session_timeout_ms(requested: i32) -> u64 {
    u64::from(
        requested
            .clamp(MIN_SESSION_TIMEOUT_MS, MAX_SESSION_TIMEOUT_MS)
            .unsigned_abs(),
    )
}

/// The shortest session timeout this broker honours, whatever a member
/// asks for — real Kafka's own `group.min.session.timeout.ms` default.
/// Below this, a slow GC pause or a scheduling hiccup evicts a healthy
/// member.
pub(crate) const MIN_SESSION_TIMEOUT_MS: i32 = 6_000;

/// The longest session timeout this broker honours, whatever a member
/// asks for — real Kafka's own `group.max.session.timeout.ms` default.
pub(crate) const MAX_SESSION_TIMEOUT_MS: i32 = 1_800_000;

#[cfg(test)]
mod tests {
    use super::{MAX_SESSION_TIMEOUT_MS, MIN_SESSION_TIMEOUT_MS, clamp_session_timeout_ms};

    #[test]
    fn a_negative_value_floors_to_the_minimum() {
        assert_eq!(
            clamp_session_timeout_ms(-1),
            u64::try_from(MIN_SESSION_TIMEOUT_MS).unwrap_or(0)
        );
    }

    #[test]
    fn an_ordinary_value_passes_through() {
        assert_eq!(clamp_session_timeout_ms(30_000), 30_000);
    }

    #[test]
    fn a_value_above_the_ceiling_is_clamped() {
        assert_eq!(
            clamp_session_timeout_ms(i32::MAX),
            u64::try_from(MAX_SESSION_TIMEOUT_MS).unwrap_or(0)
        );
    }

    #[test]
    fn a_value_below_the_floor_is_clamped() {
        assert_eq!(
            clamp_session_timeout_ms(1),
            u64::try_from(MIN_SESSION_TIMEOUT_MS).unwrap_or(0)
        );
    }
}

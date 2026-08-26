//! Translating this project's retry policy into the vendor's.

use object_store::{BackoffConfig, RetryConfig};
use oqueue_core::RetryPolicy;

/// How fast the delay grows per attempt.
///
/// ⚠️ **2.0 because [`RetryPolicy`] doubles**, and this is the one field where
/// the two shapes could silently disagree: `object_store` takes a multiplier
/// and `RetryPolicy` states its growth in prose. Writing the multiplier here,
/// beside the translation, is what keeps "doubling" from becoming a different
/// curve because a default changed.
const BACKOFF_BASE: f64 = 2.0;

/// The vendor configuration a [`RetryPolicy`] means.
///
/// `ADR-0008` chose `object_store` partly for its retry handling, and its
/// status note has carried an unwired premise since `M1.56`: a `RetryPolicy`
/// sat in `oqueue-core` with no caller, above a vendor retry nobody
/// configured, and *no double-retry existed precisely because nothing invoked
/// it*. `M3.13` is the first real attempt-loop caller, so this is where the
/// premise is discharged.
///
/// ⚠️ **Two layers retry, and only one of them is this.** `object_store`'s
/// config governs the HTTP attempts inside one `ObjectStore` call;
/// `RetryPolicy::decide` governs whether a *caller* tries the whole operation
/// again. Setting both from one policy is what stops them multiplying into a
/// retry budget nobody wrote down.
///
/// ⚠️ **`retry_timeout` is left at the vendor default, deliberately.**
/// [`RetryPolicy`] names attempts and delays and no wall-clock ceiling, so
/// there is nothing here to translate — and inventing one would be a number
/// nobody measured wearing a translation's clothes. `M14` is where a real
/// ceiling comes from.
#[must_use]
pub fn retry_config_for(policy: RetryPolicy) -> RetryConfig {
    RetryConfig {
        backoff: BackoffConfig {
            init_backoff: policy.base_delay(),
            max_backoff: policy.max_delay(),
            base: BACKOFF_BASE,
        },
        // ⚠️ **Attempts minus one, and this is the whole of the translation
        // that could be wrong.** `RetryPolicy::decide` gives up when
        // `attempts_so_far >= max_bounded_attempts`, so the field counts
        // *attempts*: `1` means one try and no retry. `object_store` counts
        // *retries* — `retries >= max_retries`, starting at zero — so copying
        // one into the other would turn "never retry" into "retry once" and
        // give every other policy an attempt it did not ask for.
        max_retries: (policy.max_bounded_attempts().get() - 1) as usize,
        ..RetryConfig::default()
    }
}

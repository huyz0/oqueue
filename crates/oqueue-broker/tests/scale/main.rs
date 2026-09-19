//! `oqueue-broker`'s scale tests (`M7`): a node over a synthetic catalog of
//! millions of topics, measured by an allocator that counts.
//!
//! ⚠️ **A test binary of its own because it sets a global allocator.** The
//! counting allocator sees every allocation in the process, so it must not be
//! the allocator of `tests/it`, whose tests would both pay for it and pollute
//! it. A test binary is no library: nothing links it, so `ADR-0007`'s "only
//! the composition root sets an allocator" (whose concern is imposing one on
//! a consumer) is not crossed.
//!
//! ⚠️ **Every test here takes [`serial`] for its whole body.** The counters
//! are process-global and libtest runs tests on parallel threads; a second
//! test allocating while one measures would be counted as the first node's.

#![allow(clippy::expect_used)]
#![allow(unreachable_pub)]

mod catalog;
mod memory;

use stats_alloc::{INSTRUMENTED_SYSTEM, StatsAlloc};
use std::alloc::System;
use tokio::sync::{Mutex, MutexGuard};

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

/// ⚠️ **`tokio`'s, not `std`'s**: it is held across every `.await` of a
/// test body, and each test runs on its own runtime and thread.
static SERIAL: Mutex<()> = Mutex::const_new(());

/// Held for a whole test, so no two tests in this binary allocate at once.
pub async fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().await
}

/// Bytes allocated and not yet freed, process-wide.
///
/// # Panics
///
/// Never in practice: every counter fits an `i128`.
pub fn live_bytes() -> i128 {
    let stats = GLOBAL.stats();
    i128::try_from(stats.bytes_allocated).expect("fits")
        - i128::try_from(stats.bytes_deallocated).expect("fits")
        + i128::try_from(stats.bytes_reallocated).expect("fits")
}

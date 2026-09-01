//! What a seed actually reaches, measured rather than asserted.
//!
//! ⚠️ **Split from `invariants.rs` at `M10.11`, on a real seam.** That file is
//! the run and its three invariants; this is one question about the harness
//! itself — whether a seed can change anything at all — and the answer is a
//! measurement that took two rounds of review to get right.

#![allow(clippy::expect_used)]
// ⚠️ `pub` here is `pub(crate)` in effect — see `roundtrip.rs`.
#![allow(unreachable_pub)]
#![allow(clippy::redundant_pub_crate)]

use crate::invariants::{seed, the_run};

/// Draws twenty-four branch choices from the runtime's seeded RNG.
///
/// ⚠️ **Two ready arms, so the pick is the RNG's and nothing else's.** This is
/// `ADR-0028`'s own probe, kept as a measuring stick rather than as a claim.
async fn arms(n: usize) -> String {
    let mut out = String::new();
    for _ in 0..n {
        let picked = tokio::select! {
            () = std::future::ready(()) => 'a',
            () = std::future::ready(()) => 'b',
        };
        out.push(picked);
    }
    out
}

/// ⚠️ **That the run reaches a seeded choice point at all**, which review
/// measured it did not: with `max_wait_ms = 0` everywhere, `park.rs` returns on
/// its own deadline check before its `select!` is polled, `session.rs` returns
/// before its own, and `connection.rs`'s is not on the `Dispatcher` path — so
/// the run made **zero** draws and every seed executed one identical schedule.
///
/// ⚠️ **This pins the draw, not a divergence, and the difference is the whole
/// honesty of the row.** `park.rs`'s two arms are never ready *together* in
/// this workload — the commit wakes the watch before the paused clock has any
/// reason to advance to the deadline — so the branch order changes and the
/// outcome does not. ⚠️ **`M10.12` gave the seed a *workload* to choose
/// (`generated.rs`'s `Schedule`), and that is a different thing from making
/// `park.rs`'s branch choice observable.** Two seeds now draw different steps
/// and can produce different `orphans`/`checks` — true today, and asserted
/// nowhere: `generated.rs`'s tests constrain what each *step* does, not that
/// two *seeds* diverge, so a regression that made step execution
/// seed-insensitive while `Schedule::draw` kept drawing distinct step lists —
/// exactly the split `M10.11`'s review found once, a seed changing something
/// positional and nothing observable — would pass silently. Recorded rather
/// than fixed, since adding that assertion is scope this row did not carry.
/// ⚠️ **And regardless of whether it is pinned**, the two arms of one
/// `select!` racing one commit are still never both ready, whatever schedule
/// surrounds them — that is what this sentence is actually about, and it is
/// still true after `M10.12`. Closing it needs a workload where the race is
/// genuinely contended — concurrent produces against one parked fetch, say —
/// which `M10.32` receives rather than leaving unowned the way `M10.11`'s own
/// version of this gap was.
#[test]
fn the_run_consumes_the_seeded_rng() {
    let after = oqueue_testkit::run_seeded(seed(), || async {
        the_run().await;
        arms(24).await
    });
    let alone = oqueue_testkit::run_seeded(seed(), || async { arms(24).await });
    assert_ne!(
        after, alone,
        "the run drew nothing from the seeded RNG, so no seed can change what \
         it does and the corpus is a list of arbitrary numbers"
    );
}

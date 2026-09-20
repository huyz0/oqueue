//! The entropy seam: the fake's counter, and the real source's output.

// Sites are on values this test constructed from literals it controls.
#![allow(clippy::expect_used)]

use oqueue_core::DEK_BYTES;
use oqueue_crypto::{Entropy as _, FakeEntropy, OsEntropy, mint_dek};

/// The entropy seam is entered once per mint, and its counter advances.
///
/// ⚠️ Asserted because `FakeEntropy::block(n)` is what every rotation test
/// names its expected DEK with: a counter stuck at one ordinal would hand out
/// the same key on every mint, and `rotation_replaces_the_live_dek` would then
/// be asserting that a rotation changed nothing while appearing to pass.
#[test]
fn each_mint_draws_a_fresh_block_from_the_seam() {
    let entropy = FakeEntropy::new();
    assert_eq!(entropy.calls(), 0);

    let mut blocks = Vec::new();
    for _ in 0..3 {
        let dek = mint_dek(&entropy).expect("the fake never fails");
        blocks.push(*dek.expose());
    }

    assert_eq!(entropy.calls(), 3, "three mints are three draws, not one");
    assert_ne!(blocks[0], blocks[1]);
    assert_ne!(blocks[1], blocks[2]);
    assert_eq!(blocks[0], FakeEntropy::block(0));
    assert_eq!(blocks[2], FakeEntropy::block(2));
}

/// `OsEntropy` writes bytes, and two draws are not the same bytes.
///
/// # ⚠️ What this assertion rests on, exactly
///
/// It is **not** a proof that the output is random; no test can be. It is two
/// claims with very different standing, and they are worth separating:
///
/// - **Against the failure this guards** — an implementation that returns
///   `Ok(())` without writing anything, which is both the mutation
///   `cargo mutants` generates here and the plausible real bug (a discarded
///   result, a buffer filled by a branch that was not taken) — the test is
///   **deterministic**. Such an implementation leaves both buffers exactly as
///   this test initialised them, which is equal, every run, always.
/// - **Against the real implementation** it is probabilistic: two independent
///   uniform 32-byte draws collide with probability 2⁻²⁵⁶. That is not a
///   number to reason about as a flake rate — it is many orders of magnitude
///   below the chance of a bit flipping in the machine running the test, so a
///   failure here means the source is broken, not that the test was unlucky.
///
/// ⚠️ **It says nothing about the quality of the bytes.** A counter
/// incrementing once per call would pass this test exactly as the kernel does,
/// which is why [`FakeEntropy`] exists and is loudly not entropy, and why the
/// argument for `OsEntropy` is *which call it makes* (`ADR-0051` point 3)
/// rather than anything a unit test observes.
#[test]
fn the_os_source_fills_the_buffer_and_does_not_repeat() {
    let os = OsEntropy::new();

    let mut first = [0u8; DEK_BYTES];
    let mut second = [0u8; DEK_BYTES];
    os.fill(&mut first).expect("the OS random source answers");
    os.fill(&mut second).expect("the OS random source answers");

    assert_ne!(
        first, second,
        "two draws from the OS source must differ; equal means nothing was written"
    );
    // ⚠️ Weaker than the line above and kept only because it names the
    // specific shape of "nothing was written": both buffers start zeroed.
    assert_ne!(first, [0u8; DEK_BYTES]);
}

/// And a DEK minted from it is not the zero key either.
#[test]
fn a_dek_minted_from_the_os_source_is_not_the_zero_key() {
    let dek = mint_dek(&OsEntropy::new()).expect("the OS random source answers");
    assert_ne!(*dek.expose(), [0u8; DEK_BYTES]);
}

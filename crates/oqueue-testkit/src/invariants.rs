//! Three properties that must hold at every step of a run, not only at its end.
//!
//! ⚠️ **The difference is the whole point of the row that built this.** A check
//! run once, at the end, sees the state a system settled into and says nothing
//! about the states it passed through — and the failures this project is most
//! afraid of are transient by construction: a consumer that saw a record for
//! one poll before a crash erased it has already broken `NFR-21`, and the
//! system it left behind is consistent.
//!
//! ⚠️ **Sans-I/O, and therefore in this crate** (`ADR-0027`). What is checked
//! here is an [`Observation`] — numbers a caller took from wherever it can see
//! them — so the same three invariants can be checked over a broker's fixtures,
//! a simulated store's, or a future cluster's without this crate knowing that
//! any of those exist. It cannot read the system it is checking, deliberately:
//! a checker that could would be a second implementation of the thing under
//! test.
//!
//! ⚠️ **A violation names the step and the seed**, because a run that fails at
//! step 400 of 500 and reports only "invariant broken" is a run nobody can
//! replay — `ADR-0028`'s criterion, applied to the thing that reports failures.

use core::fmt;

/// One look at the system, taken between steps.
///
/// ⚠️ **Every field is a number a caller can actually get**, which is what
/// keeps this from being a wish. The broker's fixtures can read all four
/// today; a field that needed machinery nobody has would make the harness
/// unusable rather than aspirational.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Observation {
    /// The offsets a consumer can see right now, in the order served.
    ///
    /// ⚠️ **Offsets, not a count.** "Gap-free" is a statement about which
    /// offsets exist, and a count cannot tell 0,1,2 from 0,1,3.
    pub visible_offsets: Vec<i64>,
    /// The highest offset whose bytes are durable in object storage, if any.
    ///
    /// ⚠️ **`None` means nothing is durable yet**, which is not the same as
    /// zero: offset 0 being durable and nothing being durable are different
    /// states and an invariant that conflated them would pass through the
    /// window it exists to catch.
    pub durable_through: Option<i64>,
    /// The highest offset the broker advertises as readable.
    ///
    /// ⚠️ **Offset space, like every other field here**, and Kafka's high
    /// watermark is *exclusive* — so a caller converts. Mixing the two
    /// conventions in one struct is how a boundary case ends up off by one in
    /// the checker rather than in the system.
    pub high_watermark: i64,
    /// The highest offset a read can actually be served.
    pub servable_through: i64,
}

/// An invariant that did not hold, and what it saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    /// A consumer could see offsets with a hole in them.
    OffsetGap {
        /// The offset served before the hole.
        after: i64,
        /// The offset served across it.
        before: i64,
    },
    /// A consumer could see a record a crash would erase — `NFR-21`.
    CacheAheadOfDurability {
        /// The highest offset visible to a consumer.
        visible: i64,
        /// What was durable at the same instant.
        durable: Option<i64>,
    },
    /// The advertised high watermark named an offset no read could serve.
    WatermarkAheadOfServability {
        /// What was advertised.
        watermark: i64,
        /// What could be served.
        servable: i64,
    },
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OffsetGap { after, before } => write!(
                f,
                "offsets are not gap-free: {after} is followed by {before}"
            ),
            Self::CacheAheadOfDurability { visible, durable } => write!(
                f,
                "a consumer can see offset {visible} with only {durable:?} durable — \
                 NFR-21: a crash here erases a record somebody already read"
            ),
            Self::WatermarkAheadOfServability {
                watermark,
                servable,
            } => write!(
                f,
                "the high watermark is {watermark} and only {servable} can be served, \
                 so a consumer that fetches what it was told exists gets nothing"
            ),
        }
    }
}

/// A violation, with enough context to replay the run that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Broken {
    /// Which invariant, and what it saw.
    pub violation: Violation,
    /// How many observations had been checked before this one.
    pub step: u64,
    /// The seed the run was started from.
    pub seed: u64,
}

impl fmt::Display for Broken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} (step {}, SEED {} — replay with this value to reproduce)",
            self.violation, self.step, self.seed
        )
    }
}

/// Checks the three invariants at every step of one seeded run.
#[derive(Debug)]
pub struct Invariants {
    seed: u64,
    step: u64,
}

impl Invariants {
    /// A checker for the run started from `seed`.
    #[must_use]
    pub const fn for_seed(seed: u64) -> Self {
        Self { seed, step: 0 }
    }

    /// How many observations have been checked.
    #[must_use]
    pub const fn steps(&self) -> u64 {
        self.step
    }

    /// Checks `at` against all three invariants.
    ///
    /// ⚠️ **All three, and the first failure wins.** A checker that stopped at
    /// the first invariant would let the other two go unwatched for the whole
    /// run; a checker that reported all three would bury the one that fired
    /// first, which is the one that says where the run went wrong.
    ///
    /// # Errors
    ///
    /// [`Broken`] naming the invariant, the step and the seed.
    pub fn check(&mut self, at: &Observation) -> Result<(), Broken> {
        self.step += 1;
        let broken = |violation| {
            Err(Broken {
                violation,
                step: self.step,
                seed: self.seed,
            })
        };

        for pair in at.visible_offsets.windows(2) {
            let [after, before] = *pair else { continue };
            if before != after + 1 {
                return broken(Violation::OffsetGap { after, before });
            }
        }

        if let Some(&visible) = at.visible_offsets.last() {
            // ⚠️ **`None` fails**, and writing it as `durable < visible` with a
            // default of zero would have passed: nothing durable and offset 0
            // visible is the violation in its purest form.
            if at.durable_through.is_none_or(|durable| durable < visible) {
                return broken(Violation::CacheAheadOfDurability {
                    visible,
                    durable: at.durable_through,
                });
            }
        }

        if at.high_watermark > at.servable_through {
            return broken(Violation::WatermarkAheadOfServability {
                watermark: at.high_watermark,
                servable: at.servable_through,
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // A panic in a test is the test failing, which is what it is for; every
    // site below is on a `Result` this module just produced.
    #![allow(clippy::expect_used)]

    use super::{Invariants, Observation, Violation};

    /// An observation of a system doing nothing wrong.
    fn healthy() -> Observation {
        Observation {
            visible_offsets: vec![0, 1, 2],
            durable_through: Some(2),
            high_watermark: 2,
            servable_through: 2,
        }
    }

    #[test]
    fn a_healthy_observation_breaks_nothing() {
        assert_eq!(Invariants::for_seed(1).check(&healthy()), Ok(()));
    }

    /// ⚠️ **Empty is not a violation.** A run's first observation has nothing
    /// visible and nothing durable, and a checker that read `None` as "behind"
    /// would fail every run at step one.
    #[test]
    fn an_empty_system_breaks_nothing() {
        assert_eq!(
            Invariants::for_seed(1).check(&Observation::default()),
            Ok(())
        );
    }

    #[test]
    fn a_hole_in_the_offsets_is_a_gap() {
        let broken = Invariants::for_seed(1)
            .check(&Observation {
                visible_offsets: vec![0, 1, 3],
                ..healthy()
            })
            .expect_err("3 does not follow 1");
        assert_eq!(
            broken.violation,
            Violation::OffsetGap {
                after: 1,
                before: 3
            }
        );
    }

    /// ⚠️ **Backwards counts too.** A repeated or reordered offset is not a
    /// hole, and a check written as `before > after + 1` would let it through
    /// — which is the shape a stale cache serves.
    #[test]
    fn an_offset_that_goes_backwards_is_a_gap() {
        assert!(
            Invariants::for_seed(1)
                .check(&Observation {
                    visible_offsets: vec![0, 2, 1],
                    ..healthy()
                })
                .is_err()
        );
    }

    /// ⚠️ **`NFR-21` in its purest form**: something is readable and nothing is
    /// durable. Written as a comparison against a defaulted zero this passes,
    /// which is why `durable_through` is an `Option` and why this case exists.
    #[test]
    fn a_visible_record_with_nothing_durable_is_the_cache_ahead() {
        let broken = Invariants::for_seed(7)
            .check(&Observation {
                visible_offsets: vec![0],
                durable_through: None,
                high_watermark: 0,
                servable_through: 0,
            })
            .expect_err("offset 0 is readable and nothing is durable");
        assert_eq!(
            broken.violation,
            Violation::CacheAheadOfDurability {
                visible: 0,
                durable: None
            }
        );
        assert_eq!(broken.seed, 7, "a failure nobody can replay is not a test");
        assert_eq!(broken.step, 1);
    }

    #[test]
    fn a_visible_record_past_what_is_durable_is_the_cache_ahead() {
        assert!(
            Invariants::for_seed(1)
                .check(&Observation {
                    durable_through: Some(1),
                    ..healthy()
                })
                .is_err()
        );
    }

    /// ⚠️ **Equal is not ahead**, and this is the boundary a strict comparison
    /// would fail: the ordinary state of a healthy system is durable exactly
    /// through what is visible.
    #[test]
    fn durable_exactly_through_what_is_visible_is_fine() {
        assert_eq!(
            Invariants::for_seed(1).check(&Observation {
                durable_through: Some(2),
                ..healthy()
            }),
            Ok(())
        );
    }

    /// ⚠️ **One past is the minimal violation, and nothing pinned it.** Review
    /// measured that weakening the check to `> servable + 1` left all eighteen
    /// cases and the broker's own run green — and that is exactly what a slip
    /// in the driver's exclusive-to-inclusive conversion produces, so the
    /// boundary the harness is most likely to get wrong was the one it did not
    /// watch. The durability side had this case from the start; this side did
    /// not.
    #[test]
    fn a_watermark_one_past_what_can_be_served_is_a_violation() {
        let broken = Invariants::for_seed(1)
            .check(&Observation {
                high_watermark: 3,
                ..healthy()
            })
            .expect_err("3 is advertised and 2 can be served");
        assert_eq!(
            broken.violation,
            Violation::WatermarkAheadOfServability {
                watermark: 3,
                servable: 2
            }
        );
    }

    /// ⚠️ **Equal is not ahead**, the same boundary from the other side.
    #[test]
    fn a_watermark_exactly_at_what_can_be_served_is_fine() {
        assert_eq!(
            Invariants::for_seed(1).check(&Observation {
                high_watermark: 2,
                ..healthy()
            }),
            Ok(())
        );
    }

    #[test]
    fn a_watermark_past_what_can_be_served_is_a_violation() {
        let broken = Invariants::for_seed(1)
            .check(&Observation {
                high_watermark: 5,
                ..healthy()
            })
            .expect_err("5 is advertised and 2 can be served");
        assert_eq!(
            broken.violation,
            Violation::WatermarkAheadOfServability {
                watermark: 5,
                servable: 2
            }
        );
    }

    /// ⚠️ **Behind is allowed.** A watermark that lags what a replica could
    /// serve is conservative, not wrong, and an invariant that forbade it
    /// would fail every run in which a commit had landed and the advertisement
    /// had not caught up.
    #[test]
    fn a_watermark_behind_what_can_be_served_is_allowed() {
        assert_eq!(
            Invariants::for_seed(1).check(&Observation {
                high_watermark: 1,
                ..healthy()
            }),
            Ok(())
        );
    }

    /// ⚠️ **The step counts observations, not violations**, so a failure names
    /// where in the run it happened — which is the difference between a
    /// replayable failure and "it broke somewhere".
    #[test]
    fn the_step_names_where_in_the_run_it_broke() {
        let mut invariants = Invariants::for_seed(3);
        for _ in 0..4 {
            assert_eq!(invariants.check(&healthy()), Ok(()));
        }
        assert_eq!(invariants.steps(), 4);
        let broken = invariants
            .check(&Observation {
                visible_offsets: vec![0, 2],
                ..healthy()
            })
            .expect_err("a gap");
        assert_eq!(broken.step, 5);
    }

    /// ⚠️ **The message carries the seed**, because it is what a reader of a
    /// failing run has and everything else is in the test's own output.
    #[test]
    fn the_message_says_how_to_replay() {
        let broken = Invariants::for_seed(11)
            .check(&Observation {
                visible_offsets: vec![0],
                durable_through: None,
                ..healthy()
            })
            .expect_err("nothing durable");
        let said = broken.to_string();
        assert!(said.contains("SEED 11"), "{said}");
        assert!(said.contains("NFR-21"), "{said}");
    }
}

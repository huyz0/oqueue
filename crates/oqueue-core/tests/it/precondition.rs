//! `Precondition`: only two states exist, and nothing else is representable.

use oqueue_core::{Precondition, PreconditionToken};

/// A `match` with no wildcard arm compiles — the two variants named here are
/// the *only* ones the compiler will accept, so a hypothetical third state
/// (an unrepresentable combination, or "matches nothing in particular") is
/// not a runtime case this function forgot to handle, it is a variant that
/// does not exist to forget.
const fn describe(p: &Precondition) -> &'static str {
    match p {
        Precondition::IfAbsent => "if-absent",
        Precondition::IfMatches(_) => "if-matches",
    }
}

#[test]
fn if_absent_is_distinguishable_from_if_matches() {
    let matches = Precondition::IfMatches(PreconditionToken::new("gen-1"));
    assert_eq!(describe(&Precondition::IfAbsent), "if-absent");
    assert_eq!(describe(&matches), "if-matches");
}

/// Two `IfMatches` preconditions carrying the same token are equal; carrying
/// different tokens, they are not. Equality is meaningful, not merely
/// derived and unused.
#[test]
fn if_matches_equality_follows_the_wrapped_token() {
    let a = Precondition::IfMatches(PreconditionToken::new("gen-1"));
    let b = Precondition::IfMatches(PreconditionToken::new("gen-1"));
    let c = Precondition::IfMatches(PreconditionToken::new("gen-2"));

    assert_eq!(a, b);
    assert_ne!(a, c);
    assert_ne!(Precondition::IfAbsent, a);
}

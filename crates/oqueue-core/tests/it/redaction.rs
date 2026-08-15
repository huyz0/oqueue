//! FR-44: no secret reaches a log, and formatting is the only path to a log.
//!
//! ⚠️ **Every rendering below is pinned with `assert_eq!`, not screened with
//! "does not contain".** A containment check only rules out the encodings it
//! was written to think of, and review found that out the hard way: an earlier
//! version of this file screened for the ASCII secret and for `{:?}` of the
//! byte vector, and a `Display` impl rendering the same bytes as `{:x?}` leaked
//! all 32 of them with the whole suite green.
//!
//! ⚠️ **Pinning narrows that gap; it does not close it, and claiming otherwise
//! would be the same mistake one level up.** These tests pin the specifiers
//! listed below, `sign_plus` among them. `Formatter` exposes three more that
//! nothing here sets — `sign_minus`, `sign_aware_zero_pad` and a non-default
//! `fill` — and an impl branching on one of those
//! would leak past every assertion here. **What actually makes a leak
//! impossible is in `redacted.rs`, not in this file**: neither formatting impl
//! takes a `T: Debug` or `T: Display` bound, so there is no expression that
//! renders the value at all. These tests are the regression guard on that
//! property, not the property.

#![allow(clippy::expect_used)]

use oqueue_core::{Error, KeyId, Redacted};

/// Distinctive enough that an accidental match is not plausible, and chosen so
/// its hex, decimal and escaped forms are all different from each other.
const SECRET: &str = "s3cr3t-AKIA7MZQ4EXAMPLE-9f2b1c8d";

fn secret_bytes() -> Vec<u8> {
    SECRET.as_bytes().to_vec()
}

/// Every rendering of the wrapper, pinned exactly.
#[test]
fn redacted_renderings_are_exactly_these_and_nothing_else() {
    let r = Redacted::new(secret_bytes());

    assert_eq!(format!("{r}"), "<redacted>");
    assert_eq!(format!("{r:?}"), "Redacted(<redacted>)");
    // ⚠️ The alternate form too. `{:#?}` is what a pretty-printed panic and
    // most structured log dumps use, so an `if f.alternate()` branch that
    // leaked would sit on the path that matters most and be invisible to a
    // test that only checked `{:?}`.
    assert_eq!(format!("{r:#?}"), "Redacted(<redacted>)");
    // ⚠️ `{:x?}` explicitly: it is the encoding that leaked past an earlier
    // version of this file, so it is the one specifier most worth naming.
    assert_eq!(format!("{r:x?}"), "Redacted(<redacted>)");
    assert_eq!(format!("{r:X?}"), "Redacted(<redacted>)");
    // A sign flag must not become a branch either — on Display *or* Debug.
    // ⚠️ `{:+?}` is valid and was a surviving mutant until it was pinned here.
    assert_eq!(format!("{r:+}"), "<redacted>");
    assert_eq!(format!("{r:+?}"), "Redacted(<redacted>)");
    // Width, fill and precision are honoured, and none of them opens a hole:
    // what is padded or truncated is the constant, never a fragment of `T`.
    assert_eq!(format!("{r:>40}"), format!("{:>40}", "<redacted>"));
    assert_eq!(format!("{r:.3}"), "<re");
}

/// An error raised about secret material, formatted.
///
/// ⚠️ `M0.11` removed the secret from this variant entirely — it now carries a
/// key *identifier*, which names material without being it. The test stays
/// because the property worth pinning is unchanged: nothing about a rejected
/// key renders its bytes.
#[test]
fn error_about_a_secret_renders_exactly_these_and_nothing_else() {
    let err = Error::SecretRejected {
        context: "unwrapping the data encryption key",
        key_id: KeyId::new("arn:aws:kms:eu-west-1:123456789012:key/abcd").expect("non-empty"),
    };

    // ⚠️ Pinned, not screened — this file's header says why, and a previous
    // version of this very test was caught screening.
    assert_eq!(
        format!("{err}"),
        "secret rejected while unwrapping the data encryption key \
         (key arn:aws:kms:eu-west-1:123456789012:key/abcd)"
    );
    assert_eq!(
        format!("{err:?}"),
        "SecretRejected { context: \"unwrapping the data encryption key\", \
         key_id: KeyId(\"arn:aws:kms:eu-west-1:123456789012:key/abcd\") }"
    );
    assert_eq!(
        format!("{err:#?}"),
        "SecretRejected {\n    \
         context: \"unwrapping the data encryption key\",\n    \
         key_id: KeyId(\n        \
         \"arn:aws:kms:eu-west-1:123456789012:key/abcd\",\n    ),\n}"
    );

    // ⚠️ The `source()` chain is a distinct path to a log: an operator's
    // handler walks it and prints each link. `SecretRejected` must not have one
    // — a source carrying the secret would render outside everything pinned
    // above.
    assert!(std::error::Error::source(&err).is_none());
}

/// The secret is still reachable deliberately — a wrapper that hid it from its
/// owner too would be unusable, and `M8` must be able to zeroize it.
#[test]
fn the_owner_can_still_get_the_secret_back() {
    let r = Redacted::new(secret_bytes());
    assert_eq!(r.expose(), &secret_bytes());
    assert_eq!(r.expose_into(), secret_bytes());
}

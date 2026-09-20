//! Cutting one partition's records out of a bundled object.
//!
//! ⚠️ **Its own module because it is a different question from what a read
//! costs.** `read.rs` decides which objects a request may fetch and what it
//! charges for them; this decides what, inside one of those objects, belongs
//! to the partition that asked. The first is a bound on a client's request;
//! the second is a parse over bytes an object store returned, where
//! `security.md` rule 3 is what governs and `parse_footer` does the walking.

// ⚠️ `pub(crate)` inside a private module: `unreachable_pub` denies the `pub`
// clippy's `redundant_pub_crate` asks for. `pub(crate)` is the visibility that
// is actually true here, so the lint that disagrees is the one allowed — the
// same trade `fetch/target.rs` and `test_executor.rs` already make.
#![allow(clippy::redundant_pub_crate)]

use oqueue_core::{ByteRange, Error, KeyDomain, PartitionId, Region, TopicId, parse_footer};

/// This partition's records, cut out of a whole bundled object.
pub(crate) fn slice_region(
    whole: &[u8],
    topic: &TopicId,
    partition: PartitionId,
    domain: &KeyDomain,
) -> Result<Vec<u8>, Error> {
    let size = whole.len() as u64;
    let regions = parse_footer(whole, size)?;
    let ByteRange::Bounded(bounds) = region_for(&regions, topic, partition, domain)? else {
        // `parse_footer` refuses an unbounded region, so this arm is
        // unreachable — named rather than `unwrap`ped, because a panic here
        // would be reachable from stored bytes (`security.md` rule 3).
        return Err(Error::IndexObjectMismatch);
    };
    let from = usize::try_from(bounds.offset()).map_err(|_| Error::IndexObjectMismatch)?;
    let len = usize::try_from(bounds.length()).map_err(|_| Error::IndexObjectMismatch)?;
    let end = from.checked_add(len).ok_or(Error::IndexObjectMismatch)?;
    whole
        .get(from..end)
        .map(<[u8]>::to_vec)
        .ok_or(Error::IndexObjectMismatch)
}

/// The one region in `regions` this partition's records live in.
///
/// ⚠️ **Exactly one, and a second is an error rather than a choice.** Regions
/// carry no offsets — offsets are assigned at commit, after the object is
/// written — so two regions for one `(topic, partition)` in one object leave
/// nothing to say which of them an `ObjectRef` names. The write path is what
/// keeps that from happening: one `Produce` request carries at most one batch
/// per partition, so one flush pushes at most one region for it. This is where
/// that assumption is checked rather than trusted.
fn region_for(
    regions: &[Region],
    topic: &TopicId,
    partition: PartitionId,
    domain: &KeyDomain,
) -> Result<ByteRange, Error> {
    let mut found = regions
        .iter()
        .filter(|region| region.topic() == topic && region.partition() == partition);
    let region = found.next().ok_or(Error::IndexObjectMismatch)?;
    if found.next().is_some() {
        return Err(Error::IndexObjectMismatch);
    }
    domain.validate_region(region)?;
    Ok(region.bytes())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::slice_region;
    use oqueue_core::{BundleBuilder, KeyDomain, PartitionId, PushedRecords, TopicId};

    fn topic() -> TopicId {
        TopicId::new("orders").expect("a valid topic")
    }

    fn object() -> Vec<u8> {
        let mut bundle = BundleBuilder::new();
        bundle
            .push(
                topic(),
                PartitionId::new(0).expect("a valid partition"),
                PushedRecords {
                    count: 1,
                    producer: None,
                },
                b"a record batch",
            )
            .expect("a non-empty region");
        bundle.seal().expect("a non-empty bundle").into_payload()
    }

    #[test]
    fn a_default_domain_accepts_an_unsealed_region() {
        let bytes = object();
        let sliced = slice_region(
            &bytes,
            &topic(),
            PartitionId::new(0).expect("a valid partition"),
            &KeyDomain::default_domain(),
        )
        .expect("the region belongs to the default domain");

        assert_eq!(sliced, b"a record batch");
    }

    #[test]
    fn a_customer_domain_refuses_a_region_relabelled_as_unsealed() {
        let bytes = object();
        let error = slice_region(
            &bytes,
            &topic(),
            PartitionId::new(0).expect("a valid partition"),
            &KeyDomain::customer(oqueue_core::KeyId::new("customer-kek").expect("a key id")),
        )
        .expect_err("topic metadata requires a sealed region");

        assert!(matches!(
            error,
            oqueue_core::Error::RegionKeyDomainMismatch {
                expected_sealed: true
            }
        ));
    }
}

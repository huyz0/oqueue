//! The composition seam for re-sealing a customer-domain region during merge.

use crate::{
    BoxFuture, KeyDomain, PartitionId, Region, RegionAlg, RegionEnvelope, Result, SealedRegion,
    TopicId,
};

/// The sealed bytes and envelope produced for one compacted region.
#[derive(Debug)]
pub struct ResealedRegion {
    bytes: Vec<u8>,
    alg: RegionAlg,
    envelope: RegionEnvelope,
}

impl ResealedRegion {
    /// Creates one result from a re-sealer implementation.
    #[must_use]
    pub const fn new(bytes: Vec<u8>, alg: RegionAlg, envelope: RegionEnvelope) -> Self {
        Self {
            bytes,
            alg,
            envelope,
        }
    }

    /// Borrows this result in the durable format's sealed-region shape.
    #[must_use]
    pub fn as_sealed_region(&self) -> SealedRegion<'_> {
        SealedRegion {
            bytes: &self.bytes,
            alg: self.alg,
            envelope: self.envelope.clone(),
        }
    }

    /// Refuses a result whose sealedness or key identifier disagrees with the
    /// authoritative topic domain.
    ///
    /// # Errors
    ///
    /// [`crate::Error::RegionKeyDomainMismatch`] when the result cannot be
    /// written for `domain`.
    pub fn validate_domain(&self, domain: &KeyDomain) -> Result<()> {
        let expected_sealed = domain.requires_sealing();
        if (self.alg != RegionAlg::None) != expected_sealed
            || domain
                .key_id()
                .is_some_and(|expected| self.envelope.key_id() != expected)
        {
            return Err(crate::Error::RegionKeyDomainMismatch { expected_sealed });
        }
        Ok(())
    }
}

/// Re-seals one input region for its position in a compacted output object.
pub trait RegionReSealer: Send + Sync + core::fmt::Debug {
    /// Re-seals one region, or returns a provider error such as
    /// [`crate::Error::KeyRevoked`].
    fn reseal<'a>(&'a self, request: ResealRequest<'a>) -> BoxFuture<'a, Result<ResealedRegion>>;
}

/// The input and output position supplied to a [`RegionReSealer`].
#[derive(Debug, Clone, Copy)]
pub struct ResealRequest<'a> {
    /// The topic's authoritative key domain.
    pub domain: &'a KeyDomain,
    /// The topic whose region is being rewritten.
    pub topic: &'a TopicId,
    /// The partition whose region is being rewritten.
    pub partition: PartitionId,
    /// The region's index in the new output object.
    pub output_region_index: u32,
    /// The input footer metadata.
    pub region: &'a Region,
    /// The input region bytes.
    pub bytes: &'a [u8],
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::ResealedRegion;
    use crate::{KeyDomain, KeyId, ParsedNonce, Redacted, RegionAlg, RegionEnvelope, WrappedKey};

    fn envelope(key: &str) -> RegionEnvelope {
        RegionEnvelope::new(
            KeyId::new(key).expect("a valid key id"),
            WrappedKey::new(Redacted::new(vec![1; 8])),
            ParsedNonce::decode([0; 12]),
        )
        .expect("a valid envelope")
    }

    fn result(alg: RegionAlg, key: &str) -> ResealedRegion {
        ResealedRegion::new(vec![1], alg, envelope(key))
    }

    #[test]
    fn a_matching_sealed_result_is_accepted() {
        result(RegionAlg::Aes256Gcm, "kek")
            .validate_domain(&KeyDomain::customer(KeyId::new("kek").expect("a key id")))
            .expect("matching metadata is accepted");
    }

    #[test]
    fn an_unsealed_result_is_rejected_for_a_customer_domain() {
        assert!(
            result(RegionAlg::None, "kek")
                .validate_domain(&KeyDomain::customer(KeyId::new("kek").expect("a key id")))
                .is_err()
        );
    }

    #[test]
    fn a_result_from_another_key_domain_is_rejected() {
        assert!(
            result(RegionAlg::Aes256Gcm, "other-kek")
                .validate_domain(&KeyDomain::customer(KeyId::new("kek").expect("a key id")))
                .is_err()
        );
    }
}

//! The topic key domain a reader uses to validate a region's protection.

use crate::{Error, KeyId, Region, RegionAlg, Result};

/// The key domain a topic belongs to.
///
/// A default-domain topic stores its regions as written. A customer-key
/// domain stores sealed regions under the named key. This fact belongs to
/// topic metadata, not to the region footer: the footer's algorithm byte is
/// part of the bytes an attacker can edit.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum KeyDomain {
    /// The provider-managed, unsealed object path.
    #[default]
    Default,
    /// A customer-supplied key domain.
    Customer(KeyId),
}

impl KeyDomain {
    /// The domain with no customer key.
    #[must_use]
    pub const fn default_domain() -> Self {
        Self::Default
    }

    /// The domain identified by a customer-supplied key.
    #[must_use]
    pub const fn customer(key_id: KeyId) -> Self {
        Self::Customer(key_id)
    }

    /// Whether regions in this domain must be sealed.
    #[must_use]
    pub const fn requires_sealing(&self) -> bool {
        matches!(self, Self::Customer(_))
    }

    /// The customer key for this domain, if any.
    #[must_use]
    pub const fn key_id(&self) -> Option<&KeyId> {
        match self {
            Self::Default => None,
            Self::Customer(key_id) => Some(key_id),
        }
    }

    /// Refuses a region whose footer does not match this topic key domain.
    ///
    /// The domain comes from topic metadata; the region's persisted algorithm
    /// byte is not authoritative for deciding whether bytes must be opened.
    ///
    /// # Errors
    ///
    /// [`Error::RegionKeyDomainMismatch`] if the region's sealedness or key
    /// identifier does not match this domain.
    pub fn validate_region(&self, region: &Region) -> Result<()> {
        let expected_sealed = self.requires_sealing();
        if (region.alg() != RegionAlg::None) != expected_sealed {
            return Err(Error::RegionKeyDomainMismatch { expected_sealed });
        }
        if let Some(expected_key) = self.key_id()
            && region
                .envelope()
                .is_none_or(|envelope| envelope.key_id() != expected_key)
        {
            return Err(Error::RegionKeyDomainMismatch { expected_sealed });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::KeyDomain;
    use crate::{
        BundleBuilder, KeyId, ParsedNonce, PartitionId, PushedRecords, Redacted, RegionAlg,
        RegionEnvelope, SealedRegion, TopicId, WrappedKey, parse_footer,
    };

    const WRAPPED: &[u8] = b"wrapped-dek";

    fn topic() -> TopicId {
        TopicId::new("orders").expect("a valid topic")
    }

    fn unsealed_region() -> crate::Region {
        let mut bundle = BundleBuilder::new();
        bundle
            .push(
                topic(),
                PartitionId::new(0).expect("a valid partition"),
                PushedRecords {
                    count: 1,
                    producer: None,
                },
                b"plain",
            )
            .expect("a non-empty region");
        let object = bundle.seal().expect("a non-empty bundle");
        parse_footer(object.payload(), object.payload().len() as u64)
            .expect("a valid footer")
            .into_iter()
            .next()
            .expect("one region")
    }

    fn sealed_region(key: &str) -> crate::Region {
        let envelope = RegionEnvelope::new(
            KeyId::new(key).expect("a valid key id"),
            WrappedKey::new(Redacted::new(WRAPPED.to_vec())),
            ParsedNonce::decode([0; 12]),
        )
        .expect("a representable envelope");
        let mut bundle = BundleBuilder::new();
        bundle
            .push_sealed(
                topic(),
                PartitionId::new(0).expect("a valid partition"),
                PushedRecords {
                    count: 1,
                    producer: None,
                },
                SealedRegion {
                    bytes: b"ciphertext",
                    alg: RegionAlg::Aes256Gcm,
                    envelope,
                },
            )
            .expect("a non-empty region");
        let object = bundle.seal().expect("a non-empty bundle");
        parse_footer(object.payload(), object.payload().len() as u64)
            .expect("a valid footer")
            .into_iter()
            .next()
            .expect("one region")
    }

    #[test]
    fn default_and_customer_domains_have_opposite_sealing_requirements() {
        let default = KeyDomain::default_domain();
        let customer = KeyDomain::customer(KeyId::new("kek").expect("a valid key id"));

        assert!(!default.requires_sealing());
        assert!(default.key_id().is_none());
        assert!(customer.requires_sealing());
        assert_eq!(customer.key_id().map(KeyId::as_str), Some("kek"));
    }

    #[test]
    fn a_customer_domain_rejects_a_region_that_says_none() {
        let region = unsealed_region();
        let error = KeyDomain::customer(KeyId::new("kek").expect("a key id"))
            .validate_region(&region)
            .expect_err("customer data must be sealed");

        assert!(matches!(
            error,
            crate::Error::RegionKeyDomainMismatch {
                expected_sealed: true
            }
        ));
    }

    #[test]
    fn a_default_domain_rejects_a_sealed_region() {
        let region = sealed_region("kek");
        let error = KeyDomain::default_domain()
            .validate_region(&region)
            .expect_err("default data must remain unsealed");

        assert!(matches!(
            error,
            crate::Error::RegionKeyDomainMismatch {
                expected_sealed: false
            }
        ));
    }

    #[test]
    fn a_customer_domain_rejects_a_region_from_another_key_domain() {
        let region = sealed_region("actual-kek");
        let error = KeyDomain::customer(KeyId::new("other-kek").expect("a key id"))
            .validate_region(&region)
            .expect_err("a sealed region must retain its key-domain identity");

        assert!(matches!(
            error,
            crate::Error::RegionKeyDomainMismatch {
                expected_sealed: true
            }
        ));
    }

    #[test]
    fn a_customer_domain_accepts_a_matching_sealed_region() {
        let region = sealed_region("kek");
        KeyDomain::customer(KeyId::new("kek").expect("a key id"))
            .validate_region(&region)
            .expect("the footer names this domain");
    }
}

//! The broker's customer-domain sealing seam.

use oqueue_core::{
    BoxFuture, Error, KeyDomain, PartitionId, RegionAlg, RegionEnvelope, Result, SealedRegion,
    TopicId,
};

/// Owned output from the encryption seam, held until the bundle copies it.
#[derive(Debug)]
pub struct SealedRegionOwned {
    bytes: Vec<u8>,
    alg: RegionAlg,
    envelope: RegionEnvelope,
}

impl SealedRegionOwned {
    /// Creates an owned sealed-region result for the broker's write path.
    #[must_use]
    pub const fn new(bytes: Vec<u8>, alg: RegionAlg, envelope: RegionEnvelope) -> Self {
        Self {
            bytes,
            alg,
            envelope,
        }
    }

    pub(crate) fn as_region(&self) -> SealedRegion<'_> {
        SealedRegion {
            bytes: &self.bytes,
            alg: self.alg,
            envelope: self.envelope.clone(),
        }
    }
}

/// Seals one customer-domain region before it enters a bundle.
pub trait RegionSealer: Send + Sync + std::fmt::Debug {
    /// Seals the records for one topic partition under its key domain.
    fn seal<'a>(
        &'a self,
        domain: &'a KeyDomain,
        topic: &'a TopicId,
        partition: PartitionId,
        records: &'a [u8],
    ) -> BoxFuture<'a, Result<SealedRegionOwned>>;
}

/// The safe default when no KMS-backed region sealer is configured.
#[derive(Debug, Default)]
pub struct RejectingRegionSealer;

impl RejectingRegionSealer {
    /// A sealer that refuses customer-domain writes rather than persisting
    /// plaintext.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl RegionSealer for RejectingRegionSealer {
    fn seal<'a>(
        &'a self,
        _domain: &'a KeyDomain,
        _topic: &'a TopicId,
        _partition: PartitionId,
        _records: &'a [u8],
    ) -> BoxFuture<'a, Result<SealedRegionOwned>> {
        Box::pin(async { Err(Error::EncryptionDisabled) })
    }
}

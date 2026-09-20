//! Customer-domain compaction: re-seal, preserve, and refuse revoked keys.

#![allow(clippy::expect_used)]

use oqueue_compact::{
    CompactionDomain, RegionReSealer, ResealRequest, ResealedRegion, merge, merge_with_resealer,
};
use oqueue_core::{
    BoxFuture, BundleBuilder, ByteRange, Error, FakeKeyProvider, KeyDomain, KeyId, KeyProvider,
    ObjectRef, ObjectStore, ParsedNonce, PushedRecords, Redacted, RegionAlg, RegionEnvelope,
    Result, SealedRegion, WrappedKey, parse_footer,
};

use crate::naming::namer;
use crate::support::{Counting, key, offset, partition, planned, topic, write_input};
use std::sync::atomic::{AtomicUsize, Ordering};

fn key_id() -> KeyId {
    KeyId::new("customer-kek").expect("a valid key id")
}

async fn write_sealed_input(
    store: &Counting,
    name: &str,
    base: i64,
    count: u32,
    nonce_byte: u8,
) -> ObjectRef {
    let provider = FakeKeyProvider::new();
    let id = key_id();
    let wrapped = provider
        .wrap(&id, &Redacted::new(vec![7; 32]))
        .await
        .expect("the fake wraps");
    let envelope = RegionEnvelope::new(id, wrapped, ParsedNonce::decode([nonce_byte; 12]))
        .expect("a valid envelope");
    let mut builder = BundleBuilder::new();
    builder
        .push_sealed(
            topic(),
            partition(),
            PushedRecords {
                count,
                producer: None,
            },
            SealedRegion {
                bytes: b"ciphertext",
                alg: RegionAlg::Aes256Gcm,
                envelope,
            },
        )
        .expect("a valid sealed region");
    let object = builder.seal().expect("a valid bundle");
    store
        .inner
        .put(&key(name), object.into_payload(), None)
        .await
        .expect("the input is stored");
    ObjectRef::new(key(name), offset(base), count)
}

#[derive(Debug, Default)]
struct CountingReSealer {
    calls: AtomicUsize,
}

impl RegionReSealer for CountingReSealer {
    fn reseal<'a>(&'a self, request: ResealRequest<'a>) -> BoxFuture<'a, Result<ResealedRegion>> {
        let expected =
            u32::try_from(self.calls.fetch_add(1, Ordering::Relaxed)).expect("few test calls");
        assert_eq!(request.output_region_index, expected);
        let input_envelope = request.region.envelope().expect("a sealed input");
        let mut bytes = request.bytes.to_vec();
        for byte in &mut bytes {
            *byte ^= 0xff;
        }
        let envelope = RegionEnvelope::new(
            input_envelope.key_id().clone(),
            WrappedKey::new(Redacted::new(b"fresh-wrap".to_vec())),
            input_envelope.nonce(),
        )
        .expect("a valid resealed envelope");
        let result = ResealedRegion::new(bytes, request.region.alg(), envelope);
        Box::pin(async move { Ok(result) })
    }
}

#[tokio::test]
async fn customer_compaction_rewraps_and_preserves_the_key_domain() {
    let store = Counting::new();
    let first = write_sealed_input(&store, "sealed-input-0", 0, 6, 1).await;
    let second = write_sealed_input(&store, "sealed-input-1", 6, 7, 2).await;
    let resealer = CountingReSealer::default();
    let domain = KeyDomain::customer(key_id());

    let outcome = merge_with_resealer(
        &store,
        &planned(13),
        &[first, second],
        &mut namer(),
        CompactionDomain::customer(&domain, &resealer),
    )
    .await
    .expect("customer-domain compaction succeeds");
    let output = outcome.object().expect("one output object");
    let policy_debug = format!("{:?}", CompactionDomain::customer(&domain, &resealer));
    assert!(policy_debug.contains("resealer: true"));
    let bytes = store
        .inner
        .get(output, ByteRange::Full)
        .await
        .expect("the output is readable");
    let regions = parse_footer(&bytes, bytes.len() as u64).expect("a valid footer");

    assert_eq!(regions.len(), 2);
    assert_eq!(regions[0].alg(), RegionAlg::Aes256Gcm);
    assert_eq!(
        regions[0].envelope().expect("the envelope").key_id(),
        domain.key_id().expect("the key")
    );
    let expected: Vec<u8> = b"ciphertext".iter().map(|byte| byte ^ 0xff).collect();
    for region in &regions {
        let ByteRange::Bounded(span) = region.bytes() else {
            panic!("the output region has a bounded range");
        };
        let start = usize::try_from(span.offset()).expect("a test-sized offset");
        let end = start + usize::try_from(span.length()).expect("a test-sized length");
        assert_eq!(&bytes[start..end], expected.as_slice());
        assert_eq!(
            region
                .envelope()
                .expect("the envelope")
                .wrapped_dek()
                .as_redacted()
                .expose(),
            b"fresh-wrap"
        );
    }
    assert_eq!(resealer.calls.load(Ordering::Relaxed), 2);
}

#[derive(Debug)]
struct RevokedReSealer;

impl RegionReSealer for RevokedReSealer {
    fn reseal<'a>(&'a self, request: ResealRequest<'a>) -> BoxFuture<'a, Result<ResealedRegion>> {
        Box::pin(async move {
            Err(Error::KeyRevoked {
                key_id: request.domain.key_id().expect("a customer domain").clone(),
            })
        })
    }
}

#[tokio::test]
async fn a_revoked_domain_is_reported_and_does_not_stall_another_merge() {
    let store = Counting::new();
    let input = write_sealed_input(&store, "sealed-input", 0, 13, 1).await;
    let domain = KeyDomain::customer(key_id());
    let revoked = merge_with_resealer(
        &store,
        &planned(13),
        &[input],
        &mut namer(),
        CompactionDomain::customer(&domain, &RevokedReSealer),
    )
    .await;
    assert_eq!(revoked, Err(Error::KeyRevoked { key_id: key_id() }));

    write_input(&store, "default-input", &topic(), 13, 8).await;
    let normal = merge(
        &store,
        &planned(13),
        &[ObjectRef::new(key("default-input"), offset(0), 13)],
        &mut namer(),
    )
    .await;
    assert!(normal.is_ok(), "another domain continues: {normal:?}");
}

#[derive(Debug)]
struct WrongKeyReSealer;

impl RegionReSealer for WrongKeyReSealer {
    fn reseal<'a>(&'a self, request: ResealRequest<'a>) -> BoxFuture<'a, Result<ResealedRegion>> {
        let input_envelope = request.region.envelope().expect("a sealed input");
        let envelope = RegionEnvelope::new(
            KeyId::new("other-kek").expect("a valid key id"),
            input_envelope.wrapped_dek().clone(),
            input_envelope.nonce(),
        )
        .expect("a valid envelope");
        Box::pin(async move {
            Ok(ResealedRegion::new(
                request.bytes.to_vec(),
                request.region.alg(),
                envelope,
            ))
        })
    }
}

#[tokio::test]
async fn a_resealer_result_from_another_key_domain_is_rejected() {
    let store = Counting::new();
    let input = write_sealed_input(&store, "sealed-input", 0, 13, 1).await;
    let domain = KeyDomain::customer(key_id());

    let result = merge_with_resealer(
        &store,
        &planned(13),
        &[input],
        &mut namer(),
        CompactionDomain::customer(&domain, &WrongKeyReSealer),
    )
    .await;

    assert_eq!(
        result,
        Err(Error::RegionKeyDomainMismatch {
            expected_sealed: true
        })
    );
}

#[derive(Debug)]
struct UnsealedReSealer;

impl RegionReSealer for UnsealedReSealer {
    fn reseal<'a>(&'a self, request: ResealRequest<'a>) -> BoxFuture<'a, Result<ResealedRegion>> {
        Box::pin(async move {
            Ok(ResealedRegion::new(
                request.bytes.to_vec(),
                RegionAlg::None,
                request.region.envelope().expect("a sealed input").clone(),
            ))
        })
    }
}

#[tokio::test]
async fn a_resealer_result_with_unsealed_metadata_is_rejected() {
    let store = Counting::new();
    let input = write_sealed_input(&store, "sealed-input", 0, 13, 1).await;
    let domain = KeyDomain::customer(key_id());

    let result = merge_with_resealer(
        &store,
        &planned(13),
        &[input],
        &mut namer(),
        CompactionDomain::customer(&domain, &UnsealedReSealer),
    )
    .await;

    assert_eq!(
        result,
        Err(Error::RegionKeyDomainMismatch {
            expected_sealed: true
        })
    );
}

//! The streaming half of the `ObjectStore` seam (`ADR-0037`).
//!
//! ⚠️ **Its own file for the reason `store.rs` is split in `src/`**: the seam's
//! whole-payload contract and its streaming one are two subjects, and the one
//! file reached `code-structure.md`'s 500-line limit holding both.
//!
//! ⚠️ **No runtime here either**, for `store.rs`'s own reason — ADR-0001,
//! ADR-0002 and `M0.16` rest NFR-56's floor on no core test compiling one. The
//! futures are driven by `Waker::noop()` and a busy poll, which is correct only
//! because the fake completes every one on its first poll.

// The workspace denies `expect_used`; these sites are on values this test just
// constructed from literals it controls, so a panic means the test is wrong.
#![allow(clippy::expect_used)]

use oqueue_core::{
    BUNDLE_PART_BYTES, BundleStream, ByteRange, Error, FakeObjectStore, FaultConfig, KeyId,
    ObjectKey, ObjectStore, ParsedNonce, PartitionId, PushedRecords, Redacted, RegionAlg,
    RegionEnvelope, SealedRegion, StormKind, TopicId, WrappedKey, Written, parse_footer,
};
use std::future::Future;
use std::task::{Context, Poll, Waker};

/// Drives a future to completion on this thread; `store.rs`'s own, and the
/// same caveat applies.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = Box::pin(future);
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::hint::spin_loop(),
        }
    }
}

fn key(name: &str) -> ObjectKey {
    ObjectKey::new(name).expect("a non-empty key")
}

/// ⚠️ **A streaming write needs no size up front**, which is the whole point of
/// the seam `ADR-0037` adds: the caller pushes parts as they fill and seals
/// without ever declaring how long the object is.
#[test]
fn a_streamed_object_reads_back_as_the_concatenation_of_its_parts() {
    let store = FakeObjectStore::new();
    let key = key("streamed");

    let meta = block_on(async {
        let mut writer = store.open_multipart(&key).await.expect("a writer");
        writer.write_part(b"one".to_vec()).await.expect("part one");
        writer.write_part(b"two".to_vec()).await.expect("part two");
        writer.finish().await.expect("a sealed object")
    });

    assert_eq!(meta.size, 6, "the size is what was written, not declared");
    let read = block_on(store.get(&key, ByteRange::Full)).expect("the object is there");
    assert_eq!(read, b"onetwo".to_vec());
}

/// An object of no bytes costs a request and carries nothing.
#[test]
fn a_streamed_object_with_no_parts_is_refused() {
    let store = FakeObjectStore::new();
    let key = key("empty-stream");
    let sealed = block_on(async {
        let writer = store.open_multipart(&key).await.expect("a writer");
        writer.finish().await
    });
    assert!(
        matches!(sealed, Err(Error::EmptyBundle)),
        "an empty stream seals nothing: {sealed:?}"
    );
    assert!(
        matches!(
            block_on(store.get(&key, ByteRange::Full)),
            Err(Error::ObjectNotFound { .. })
        ),
        "and leaves no object behind"
    );
}

/// ⚠️ **The seal is unconditional and the key is not protected**, which is the
/// consequence `ADR-0037` accepts and answers with unique keys: a second stream
/// to the same key overwrites the first, and nothing here refuses it. A caller
/// that needs the write not to overwrite anything names a key nothing else
/// will name.
#[test]
fn a_second_stream_to_one_key_overwrites_it() {
    let store = FakeObjectStore::new();
    let key = key("reused");

    block_on(async {
        let mut first = store.open_multipart(&key).await.expect("a writer");
        first.write_part(b"first".to_vec()).await.expect("a part");
        first.finish().await.expect("sealed");

        let mut second = store.open_multipart(&key).await.expect("a writer");
        second.write_part(b"second".to_vec()).await.expect("a part");
        second.finish().await.expect("sealed");
    });

    let read = block_on(store.get(&key, ByteRange::Full)).expect("an object");
    assert_eq!(read, b"second".to_vec(), "last writer wins, by design");
}

/// Writes 200 quarter-part regions and reports the peak buffered bytes beside
/// what the write cost.
///
/// ⚠️ **One run feeding two claims**, rather than two tests each doing their
/// own: the run writes fifty parts, and doing it twice would double the
/// wall-clock and the fake's memory to assert two facts about the same write.
async fn fifty_parts(store: &FakeObjectStore, key: &ObjectKey) -> (usize, Written) {
    let records = vec![b'r'; BUNDLE_PART_BYTES / 4];
    let mut stream = BundleStream::open(store, key).await.expect("a stream");
    assert_eq!(stream.buffered(), 0, "nothing is buffered before a push");
    assert_eq!(stream.written(), 0, "and nothing has been written");
    assert_eq!(stream.parts(), 0, "and no part has gone to the writer");
    let mut peak = 0;
    for which in 0..200_u32 {
        stream
            .push(
                TopicId::new("t").expect("a valid topic"),
                PartitionId::new(0).expect("a valid partition"),
                PushedRecords {
                    count: which + 1,
                    producer: None,
                },
                &records,
            )
            .await
            .expect("a region");
        let buffered = stream.buffered();
        if which == 0 {
            assert_eq!(
                buffered,
                records.len(),
                "one quarter-part push buffers exactly that much"
            );
            assert_eq!(
                stream.written(),
                records.len() as u64,
                "and counts it as written, buffered or not"
            );
            assert_eq!(stream.parts(), 0, "nothing has filled a part yet");
        }
        if which == 3 {
            assert_eq!(stream.parts(), 1, "four quarters fill exactly one part");
        }
        peak = peak.max(buffered);
    }
    assert_eq!(stream.parts(), 50, "fifty full parts, before the footer's");
    let (_, cost) = stream.finish().await.expect("a sealed object");
    (peak, cost)
}

/// ⚠️ **The criterion `M5.4` deferred here: peak memory stays under the bound
/// while the object is 50x it.** `BundleStream::buffered` is what makes the
/// claim observable — a test that cannot see it can only believe it.
#[test]
fn a_stream_holds_at_most_one_part_while_writing_fifty() {
    let store = FakeObjectStore::new();
    let key = key("fifty-parts");
    let (peak, cost) = block_on(fifty_parts(&store, &key));

    assert!(
        peak < BUNDLE_PART_BYTES,
        "a full part is flushed as soon as it fills: peak {peak} against a \
         part of {BUNDLE_PART_BYTES}"
    );
    let written = block_on(store.get(&key, ByteRange::Full)).expect("the object");
    assert!(
        written.len() > BUNDLE_PART_BYTES * 40,
        "and the object really is many parts long: {} bytes",
        written.len()
    );

    // ⚠️ **What it cost, counted rather than estimated** (`ADR-0039`). 200
    // quarter-part pushes are 50 parts of records, and the footer's own part
    // is the 51st -- a number no `CostEstimate` can predict, because it is a
    // function of the output's byte length. ⚠️ **Parts, not requests**: a real
    // backend brackets these with a create and a complete that this seam
    // cannot see.
    let pushed = (BUNDLE_PART_BYTES / 4) as u64 * 200;
    assert_eq!(cost.bytes(), pushed, "every record byte, footer excluded");
    assert_eq!(cost.parts(), 51, "fifty full parts, and the footer's");
    assert!(
        u64::try_from(written.len()).expect("a representable length") > cost.bytes(),
        "the object is the records plus a footer, so it is longer than they are"
    );
}

/// The object a stream writes is the object `parse_footer` reads.
#[test]
fn a_streamed_bundle_reads_back_through_the_footer() {
    let store = FakeObjectStore::new();
    let key = key("streamed-bundle");
    let topic = TopicId::new("t").expect("a valid topic");
    let partition = PartitionId::new(0).expect("a valid partition");

    let spans = block_on(async {
        let mut stream = BundleStream::open(&store, &key).await.expect("a stream");
        for (which, marker) in (*b"ab").into_iter().enumerate() {
            stream
                .push(
                    topic.clone(),
                    partition,
                    PushedRecords {
                        count: u32::try_from(which).expect("a small count") + 1,
                        producer: None,
                    },
                    &[marker; 32],
                )
                .await
                .expect("a region");
        }
        stream.finish().await.expect("a sealed object")
    });
    let (spans, cost) = spans;

    assert_eq!(spans.len(), 2);
    // ⚠️ **What it cost, measured rather than estimated** (`ADR-0039`): the
    // two regions' record bytes, and the single part the footer went out in.
    // ⚠️ Parts, not requests -- a real backend brackets these with a create
    // and a complete this seam cannot see.
    assert_eq!(cost.bytes(), 64, "two 32-byte regions, footer excluded");
    assert_eq!(
        cost.parts(),
        1,
        "nothing filled a part, so the seal is the only one"
    );
    let written = block_on(store.get(&key, ByteRange::Full)).expect("the object");
    let regions = parse_footer(&written, written.len() as u64).expect("a valid footer");
    assert_eq!(regions.len(), 2);
    assert_eq!(regions[0].topic(), &topic);
    let ByteRange::Bounded(first) = regions[0].bytes() else {
        panic!("a bundled region is always bounded")
    };
    assert_eq!(first.offset(), 0);
    assert_eq!(first.length(), 32);
    assert_eq!(&written[..32], b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
}

/// The sealed streaming path must account for bytes, flush a full part, and
/// preserve the envelope metadata in the footer just like the ordinary path.
fn sealed_envelope() -> RegionEnvelope {
    RegionEnvelope::new(
        KeyId::new("kek").expect("a valid key id"),
        WrappedKey::new(Redacted::new(vec![1; 8])),
        ParsedNonce::decode([0; 12]),
    )
    .expect("a valid envelope")
}

#[test]
fn a_sealed_stream_flushes_and_reads_back_its_region() {
    let store = FakeObjectStore::new();
    let key = key("sealed-stream");
    let topic = TopicId::new("t").expect("a valid topic");
    let partition = PartitionId::new(0).expect("a valid partition");
    let payload = vec![b'c'; BUNDLE_PART_BYTES];

    let (spans, cost) = block_on(async {
        let mut stream = BundleStream::open(&store, &key).await.expect("a stream");
        stream
            .push_sealed(
                topic,
                partition,
                PushedRecords {
                    count: 1,
                    producer: None,
                },
                SealedRegion {
                    bytes: &payload,
                    alg: RegionAlg::Aes256Gcm,
                    envelope: sealed_envelope(),
                },
            )
            .await
            .expect("a sealed region");
        assert_eq!(stream.written(), BUNDLE_PART_BYTES as u64);
        assert_eq!(stream.parts(), 1, "a full sealed part flushes immediately");
        stream.finish().await.expect("a sealed object")
    });

    assert_eq!(spans.len(), 1);
    assert_eq!(cost.bytes(), BUNDLE_PART_BYTES as u64);
    assert_eq!(cost.parts(), 2, "the full payload and footer are two parts");
    let object = block_on(store.get(&key, ByteRange::Full)).expect("the object");
    let regions = parse_footer(&object, object.len() as u64).expect("a valid footer");
    assert_eq!(regions.len(), 1);
    assert_eq!(regions[0].alg(), RegionAlg::Aes256Gcm);
    assert_eq!(
        regions[0].bytes(),
        ByteRange::bounded(0, BUNDLE_PART_BYTES as u64).expect("a range")
    );
    assert_eq!(
        regions[0]
            .envelope()
            .expect("an envelope")
            .key_id()
            .as_str(),
        "kek"
    );
}

/// ⚠️ **An empty part is not a part**, and the fake must say what a backend
/// says: S3 and GCS seal on whether bytes were uploaded, so a stream of one
/// empty part is an empty object there. A fake counting calls instead would
/// pass a test no backend would.
#[test]
fn a_stream_of_only_empty_parts_is_refused() {
    let store = FakeObjectStore::new();
    let key = key("empty-parts");
    let sealed = block_on(async {
        let mut writer = store.open_multipart(&key).await.expect("a writer");
        writer.write_part(Vec::new()).await.expect("an empty part");
        writer.finish().await
    });
    assert!(
        matches!(sealed, Err(Error::EmptyBundle)),
        "no bytes is no object: {sealed:?}"
    );
}

/// ⚠️ **A failed part poisons the writer.** Against a backend the upload is
/// already aborted, so a retried part would be sent against an upload id that
/// no longer exists; a writer that accepted it would seal an object missing
/// the failed part.
#[test]
fn a_writer_that_failed_a_part_refuses_everything_after() {
    let store = FakeObjectStore::new();
    let key = key("poisoned");

    let (failed, retried, sealed) = block_on(async {
        let mut writer = store.open_multipart(&key).await.expect("a writer");
        // ⚠️ **Installed after the open**, because a storm counts store calls
        // and opening is one: this test is about a part failing, not an open.
        store.set_faults(FaultConfig {
            storm: Some((StormKind::Transient, 1)),
            ..FaultConfig::default()
        });
        let failed = writer.write_part(b"one".to_vec()).await;
        let retried = writer.write_part(b"one".to_vec()).await;
        let sealed = writer.finish().await;
        (failed, retried, sealed)
    });

    assert!(failed.is_err(), "the storm fails the first part");
    assert!(
        matches!(retried, Err(Error::Permanent)),
        "and the writer refuses the retry: {retried:?}"
    );
    assert!(
        matches!(sealed, Err(Error::Permanent)),
        "and refuses to seal: {sealed:?}"
    );
}

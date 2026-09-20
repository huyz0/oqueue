//! Sealing and opening one region: what round-trips, and what is refused.
//!
//! ⚠️ **Every negative case asserts a *refusal*, never a wrong plaintext.**
//! An AEAD that returned altered bytes rather than an error would be the whole
//! failure mode worth testing for, so each test here changes exactly one thing
//! and asserts [`Error::RegionOpenFailed`].

// Sites are on values this test constructed from literals it controls.
#![allow(clippy::expect_used)]

use oqueue_core::{
    Dek, Error, KeyId, Nonce, NonceMinter, ParsedNonce, PartitionId, RegionAlg, TopicId,
};
use oqueue_crypto::{RegionAad, TAG_BYTES, open, seal};

const PLAINTEXT: &[u8] = b"a region's worth of record batches, or near enough";

const fn dek() -> Dek {
    Dek::new([7u8; 32])
}

const fn other_dek() -> Dek {
    let mut bytes = [7u8; 32];
    bytes[31] = 8;
    Dek::new(bytes)
}

fn topic() -> TopicId {
    TopicId::new("orders").expect("non-empty")
}

fn partition() -> PartitionId {
    PartitionId::new(3).expect("non-negative")
}

/// ⚠️ A fresh nonce per call, from the one API that can mint them. There is no
/// `Nonce::from_bytes`, by design — `oqueue-core::nonce`.
fn nonce() -> Nonce {
    NonceMinter::new(1)
        .expect("in range")
        .next_object()
        .expect("first object")
        .for_region(0)
        .expect("first region")
}

/// The twelve bytes of a freshly minted nonce, as a header would carry them.
const fn parsed(n: &Nonce) -> ParsedNonce {
    ParsedNonce::decode(*n.as_bytes())
}

/// The key id every vector below is sealed under.
///
/// ⚠️ **A `static`, because `RegionAad` borrows it** — and borrowing rather
/// than owning is what keeps the associated data cheap on the seal path, where
/// it is built per region.
fn key_id() -> &'static KeyId {
    static KEY_ID: std::sync::OnceLock<KeyId> = std::sync::OnceLock::new();
    KEY_ID.get_or_init(|| KeyId::new("kek-1").expect("non-empty"))
}

fn aad(topic: &TopicId) -> RegionAad<'_> {
    RegionAad {
        topic,
        partition: partition(),
        region_index: 0,
        alg: RegionAlg::Aes256Gcm,
        key_id: key_id(),
    }
}

/// Seals `PLAINTEXT` and hands back the bytes plus the nonce a reader sees.
fn sealed(topic: &TopicId) -> (Vec<u8>, ParsedNonce) {
    let minted = nonce();
    let seen = parsed(&minted);
    let bytes = seal(&dek(), minted, aad(topic), PLAINTEXT).expect("seal");
    (bytes, seen)
}

#[test]
fn a_sealed_region_opens_to_exactly_its_plaintext() {
    let topic = topic();
    let (bytes, seen) = sealed(&topic);

    assert_eq!(
        open(&dek(), seen, aad(&topic), &bytes).expect("open"),
        PLAINTEXT
    );
}

/// ⚠️ The tag is appended and never truncated, so the overhead is exactly
/// sixteen bytes — the number a flush planner sizes an object with.
#[test]
fn sealing_costs_exactly_the_tag() {
    let topic = topic();
    let (bytes, _) = sealed(&topic);

    assert_eq!(TAG_BYTES, 16);
    assert_eq!(bytes.len(), PLAINTEXT.len() + TAG_BYTES);
}

#[test]
fn an_empty_region_seals_to_the_tag_alone() {
    let topic = topic();
    let minted = nonce();
    let seen = parsed(&minted);
    let bytes = seal(&dek(), minted, aad(&topic), b"").expect("seal");

    assert_eq!(bytes.len(), TAG_BYTES);
    assert_eq!(open(&dek(), seen, aad(&topic), &bytes).expect("open"), b"");
}

#[test]
fn a_flipped_ciphertext_byte_is_refused() {
    let topic = topic();
    let (mut bytes, seen) = sealed(&topic);
    bytes[0] ^= 1;

    assert_eq!(
        open(&dek(), seen, aad(&topic), &bytes),
        Err(Error::RegionOpenFailed)
    );
}

#[test]
fn a_flipped_tag_byte_is_refused() {
    let topic = topic();
    let (mut bytes, seen) = sealed(&topic);
    let last = bytes.len() - 1;
    bytes[last] ^= 1;

    assert_eq!(
        open(&dek(), seen, aad(&topic), &bytes),
        Err(Error::RegionOpenFailed)
    );
}

#[test]
fn a_truncated_region_is_refused_rather_than_read_short() {
    let topic = topic();
    let (bytes, seen) = sealed(&topic);

    assert_eq!(
        open(&dek(), seen, aad(&topic), &bytes[..TAG_BYTES - 1]),
        Err(Error::RegionOpenFailed)
    );
}

#[test]
fn a_wrong_nonce_is_refused() {
    let topic = topic();
    let (bytes, seen) = sealed(&topic);
    let mut twelve = *seen.as_bytes();
    twelve[11] ^= 1;

    assert_eq!(
        open(&dek(), ParsedNonce::decode(twelve), aad(&topic), &bytes),
        Err(Error::RegionOpenFailed)
    );
}

#[test]
fn a_wrong_dek_is_refused() {
    let topic = topic();
    let (bytes, seen) = sealed(&topic);

    assert_eq!(
        open(&other_dek(), seen, aad(&topic), &bytes),
        Err(Error::RegionOpenFailed)
    );
}

/// ⚠️ Under BYOK another topic is another tenant, so this is the binding that
/// matters most.
#[test]
fn a_relabelled_topic_is_refused() {
    let topic = topic();
    let (bytes, seen) = sealed(&topic);
    let other = TopicId::new("payments").expect("non-empty");

    assert_eq!(
        open(&dek(), seen, aad(&other), &bytes),
        Err(Error::RegionOpenFailed)
    );
}

#[test]
fn a_changed_partition_is_refused() {
    let topic = topic();
    let (bytes, seen) = sealed(&topic);
    let mut moved = aad(&topic);
    moved.partition = PartitionId::new(4).expect("non-negative");

    assert_eq!(
        open(&dek(), seen, moved, &bytes),
        Err(Error::RegionOpenFailed)
    );
}

/// ⚠️ **`M8.4` brought the footer's key id under the tag.** A footer edited to
/// name a different KEK — which under BYOK is a different tenant's key domain
/// — does not open, and that is a guarantee of the binding rather than a
/// consequence of the two keys happening to differ.
#[test]
fn a_relabelled_key_id_is_refused() {
    let topic = topic();
    let (bytes, seen) = sealed(&topic);
    let other = KeyId::new("kek-2").expect("non-empty");
    let relabelled = RegionAad {
        key_id: &other,
        ..aad(&topic)
    };

    assert_eq!(
        open(&dek(), seen, relabelled, &bytes),
        Err(Error::RegionOpenFailed)
    );
}

#[test]
fn a_changed_region_index_is_refused() {
    let topic = topic();
    let (bytes, seen) = sealed(&topic);
    let mut moved = aad(&topic);
    moved.region_index = 1;

    assert_eq!(
        open(&dek(), seen, moved, &bytes),
        Err(Error::RegionOpenFailed)
    );
}

/// ⚠️ **An `alg` that does not match what the region was sealed with is
/// refused**, and this is the honest statement of how far that is testable
/// today. With only two variants the only mismatch available is `Aes256Gcm`
/// against `None`, and the dispatch catches that *before* the tag — so what
/// this asserts is the refusal, not the tag's own coverage of the code. The
/// code is in the associated data regardless (`RegionAad::encode`), which is
/// what makes a third variant's mismatch a tag failure rather than a decode
/// under the wrong algorithm; the test that can distinguish the two arrives
/// with that third variant.
#[test]
fn the_alg_code_is_part_of_what_the_tag_covers() {
    let topic = topic();
    let (bytes, seen) = sealed(&topic);
    // Both directions: neither sealing nor opening falls through to a
    // default when the header's algorithm is not the one in hand.
    assert_eq!(
        seal(&dek(), nonce(), none_aad(&topic), PLAINTEXT),
        Err(Error::RegionNotEncrypted)
    );
    assert_eq!(
        open(&dek(), seen, none_aad(&topic), &bytes),
        Err(Error::RegionNotEncrypted)
    );
}

fn none_aad(topic: &TopicId) -> RegionAad<'_> {
    RegionAad {
        alg: RegionAlg::None,
        ..aad(topic)
    }
}

/// `RegionAlg::None` is "stored as written", so there is nothing to open —
/// and handing the bytes back unchanged would tell a caller that asked for
/// decryption that it got some.
#[test]
fn a_region_stored_as_written_cannot_be_opened_as_encrypted() {
    let topic = topic();

    assert_eq!(
        open(&dek(), parsed(&nonce()), none_aad(&topic), PLAINTEXT),
        Err(Error::RegionNotEncrypted)
    );
}

/// The behaviour `M3` shipped, asserted here because `M8.3` added the first
/// second variant and a `from_code` that started defaulting would be invisible
/// from `oqueue-core`'s own tests alone.
#[test]
fn an_unknown_alg_code_is_still_an_error() {
    assert_eq!(RegionAlg::from_code(0), Ok(RegionAlg::None));
    assert_eq!(RegionAlg::from_code(1), Ok(RegionAlg::Aes256Gcm));
    assert_eq!(
        RegionAlg::from_code(2),
        Err(Error::UnknownRegionAlg { code: 2 })
    );
    assert_eq!(RegionAlg::Aes256Gcm.code(), 1);
}

/// Two regions of one object are sealed under two nonces, and neither opens
/// as the other — the index binding and the nonce's own uniqueness together.
#[test]
fn two_regions_of_one_object_do_not_open_as_each_other() {
    let topic = topic();
    let mut source = NonceMinter::new(9)
        .expect("in range")
        .next_object()
        .expect("first object");

    let first_nonce = source.for_region(0).expect("region 0");
    let first_seen = parsed(&first_nonce);
    let first = seal(&dek(), first_nonce, aad(&topic), PLAINTEXT).expect("seal");

    let second_nonce = source.for_region(1).expect("region 1");
    let second_seen = parsed(&second_nonce);
    let mut second_aad = aad(&topic);
    second_aad.region_index = 1;
    let second = seal(&dek(), second_nonce, second_aad, PLAINTEXT).expect("seal");

    assert_ne!(first, second);
    assert_eq!(
        open(&dek(), second_seen, second_aad, &first),
        Err(Error::RegionOpenFailed)
    );
    assert_eq!(
        open(&dek(), first_seen, aad(&topic), &second),
        Err(Error::RegionOpenFailed)
    );
}

// ── Known-answer vectors ────────────────────────────────────────────────────
//
// ⚠️ **Everything above this line round-trips `seal` through `open`, so
// everything above this line passes if the format moves and both halves move
// with it.** A little-endian partition in `RegionAad::encode`, a dropped
// `AAD_DOMAIN`, `partition` and `region_index` swapped, the nonce bytes
// reversed — each of those is a change that keeps all fifteen round-trip tests
// green while making every region sealed by an earlier build **unopenable
// forever**, with no error until a customer reads old data.
//
// ⚠️ **And nothing else pins it.** `M13`'s FIPS build swaps this crate's
// pure-Rust AES-GCM for `aws-lc-rs`'s validated one and must produce these
// exact bytes (`ADR-0050` point 7, `ADR-0012`) — a differential test between
// the two builds is `M13`'s, and until it exists these literals are the only
// statement of what "the same format" means.
//
// The literals below were produced by running this code once and pasting the
// result. ⚠️ **That means they are only as correct as the first run**, which is
// the honest limitation of every known-answer vector generated rather than
// taken from a standard: they pin the format against *drift*, not against
// having been wrong on day one. What checks day one is `a_sealed_region_opens_
// to_exactly_its_plaintext` above, plus AES-GCM being a published construction
// `aes-gcm`'s own test vectors already check.

/// The twelve bytes a nonce minted at writer epoch 1, object 0, region 0 has —
/// `nonce()`'s own value, asserted before anything is sealed under it.
///
/// ⚠️ Asserted rather than assumed, because the vector below means nothing if
/// the nonce it was generated under is not the nonce the test mints today.
const NONCE_VECTOR: [u8; 12] = [0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0];

#[test]
fn the_minted_nonce_is_the_one_the_vectors_were_generated_under() {
    assert_eq!(*nonce().as_bytes(), NONCE_VECTOR);
}

/// ⚠️ **The part a refactor is most likely to move**, and the part no
/// round-trip test can see moving: the associated data's own byte encoding.
///
/// `b"oqueue:region:v1"` ‖ `u64_be(topic_len)` ‖ topic ‖ `i32_be(partition)` ‖
/// `u32_be(region_index)` ‖ `u8(alg_code)` ‖ `u64_be(key_id_len)` ‖ key id,
/// for topic `orders`, partition 3, region 0, `Aes256Gcm`, key id `kek-1`.
///
/// ⚠️ **The key id was appended by `M8.4`, and both vectors here were
/// regenerated with it.** That is a change to a *pinned* format, which is
/// normally the thing these literals exist to prevent — it is admissible only
/// because nothing has sealed a region yet: `M8.6` is the first writer to
/// choose `alg = 1`, so the set of objects the old encoding could make
/// unopenable is empty. After that row, this literal moves only with a
/// migration.
#[test]
fn the_associated_data_encoding_is_pinned() {
    let topic = topic();

    assert_eq!(
        aad(&topic).encode(),
        b"oqueue:region:v1\x00\x00\x00\x00\x00\x00\x00\x06orders\x00\x00\x00\x03\x00\x00\x00\x00\x01\x00\x00\x00\x00\x00\x00\x00\x05kek-1"
    );
}

/// ⚠️ **The vector that stops `seal` and `open` drifting together.** Fixed
/// key, fixed nonce, fixed associated data, fixed plaintext — one ciphertext,
/// and it is part of the on-disk format rather than an artefact of this
/// implementation.
#[test]
fn a_sealed_region_is_byte_for_byte_what_it_has_always_been() {
    let topic = topic();
    let (bytes, _) = sealed(&topic);

    assert_eq!(hex(&bytes), SEALED_VECTOR);
}

/// And the same vector opens, under the same fixed inputs, from a literal
/// rather than from something this test just sealed.
#[test]
fn the_pinned_vector_opens_to_the_pinned_plaintext() {
    let topic = topic();
    let bytes = unhex(SEALED_VECTOR);

    assert_eq!(
        open(
            &dek(),
            ParsedNonce::decode(NONCE_VECTOR),
            aad(&topic),
            &bytes
        )
        .expect("open"),
        PLAINTEXT
    );
}

/// `seal(dek = [7u8; 32], nonce = NONCE_VECTOR, aad = the vector above,
/// plaintext = PLAINTEXT)`, as ciphertext ‖ tag.
const SEALED_VECTOR: &str = "5eba01af63c7878fe19778a9ff14910e8644d075d4b9c3c2630793ea9fa5cc0b114b0b02b91347d7a29d99474264bb059d3358f1e61f51e3a0971547ca1d5d2a7171";

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

fn unhex(text: &str) -> Vec<u8> {
    assert!(
        text.len().is_multiple_of(2),
        "a hex literal has even length"
    );
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).expect("hex"))
        .collect()
}

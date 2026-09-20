//! Writing the footer a bundled object ends with.
//!
//! ⚠️ **Its own module because `M8.4` made the footer conditional**, and
//! `bundle.rs` was at the 500-line limit. The shape this writes is the shape
//! [`parse_footer`](crate::parse_footer) reads, and the two are the halves of
//! one durable format — a change to either that is not a change to both is a
//! change nothing can read back.
//!
//! # The per-region shape
//!
//! ```text
//! u16 name_len ‖ name ‖ u32 partition ‖ u32 record_count
//!              ‖ u64 offset ‖ u64 length ‖ u8 alg
//!              ‖ [envelope, iff alg != 0]
//! envelope := u16 key_id_len ‖ key_id ‖ u32 wrapped_len ‖ wrapped ‖ 12-byte nonce
//! ```
//!
//! # ⚠️ Why the format version did not change
//!
//! The trailer carries [`BUNDLE_FORMAT_VERSION`], and
//! [`parse_footer`](crate::parse_footer) **refuses** any value it does not
//! know: this format has no forward compatibility and never had any, so a
//! version bump is a hard cut for every reader, not a graceful one. That is
//! exactly why the envelope is *not* a new version:
//!
//! - a region with `alg = 0` encodes byte for byte what it encoded before this
//!   row, so an object with no sealed region is identical to what the previous
//!   code wrote — including its version byte. Bumping the version would have
//!   changed every unsealed object's bytes to describe a change none of them
//!   carry, and made today's objects unreadable by today's readers;
//! - the discriminator for the new bytes is the `alg` byte, which is the field
//!   `M3` put in the header for exactly this (doc 10 #40, `RegionAlg`'s own
//!   documentation): a reader that knows an algorithm code knows the envelope
//!   shape that goes with it, and one that does not already refuses the code
//!   through [`RegionAlg::from_code`](crate::RegionAlg::from_code).
//!
//! ⚠️ **What an older reader does with a sealed footer, stated rather than
//! implied.** A build from before this row parses `alg = 1` (`M8.3` taught
//! `from_code` that value one commit earlier) and then reads the envelope's
//! bytes as the next region's fields, so it fails with
//! [`Error::MalformedBundleFooter`] in almost every case — but "almost" is not
//! "always", and the honest guarantee is therefore not a parser-level one: it
//! is that **no object with `alg = 1` exists**. Nothing writes a sealed region
//! until `M8.6` routes a flush to a key domain, and `M8.12` is what makes a
//! reader derive sealed-ness from the topic's key domain rather than from this
//! byte. A version bump would not have bought the guarantee either — it would
//! have made those same older readers refuse *unsealed* objects too.

use super::{BUNDLE_FORMAT_VERSION, Region, RegionAlg};
use crate::{ByteRange, Error, Result};

pub(super) fn encode_footer(regions: &[Region], out: &mut Vec<u8>) -> Result<()> {
    let start = out.len();
    for region in regions {
        let topic = region.topic.as_str().as_bytes();
        let name_len = u16::try_from(topic.len()).map_err(|_| Error::BundleTooLarge)?;
        out.extend_from_slice(&name_len.to_be_bytes());
        out.extend_from_slice(topic);
        // `PartitionId` is never negative (its own invariant), so this is a
        // widening rather than a reinterpretation.
        out.extend_from_slice(&region.partition.get().unsigned_abs().to_be_bytes());
        out.extend_from_slice(&region.record_count.to_be_bytes());
        let (offset, length) = match region.bytes {
            ByteRange::Bounded(bounded) => (bounded.offset(), bounded.length()),
            // ⚠️ **An error, not `(0, 0)`.** Unreachable from `push`, but
            // `Region` is public and this is a durable-format writer: encoding
            // a zero-length range would produce an object `parse_footer`
            // refuses, after `seal` returned `Ok`, the PUT landed and the
            // metadata record committed. Failing here costs nothing and fails
            // before anything durable exists.
            ByteRange::Full => return Err(Error::UnboundedRegion),
        };
        out.extend_from_slice(&offset.to_be_bytes());
        out.extend_from_slice(&length.to_be_bytes());
        out.push(region.alg.code());
        encode_envelope(region, out)?;
    }
    let footer_len = u32::try_from(out.len() - start).map_err(|_| Error::BundleTooLarge)?;
    let count = u32::try_from(regions.len()).map_err(|_| Error::BundleTooLarge)?;
    out.extend_from_slice(&count.to_be_bytes());
    out.push(BUNDLE_FORMAT_VERSION);
    out.extend_from_slice(&footer_len.to_be_bytes());
    Ok(())
}

/// The envelope bytes of a sealed region, and nothing at all for an unsealed
/// one.
///
/// ⚠️ **Both disagreements are errors, and neither is reachable from
/// [`BundleBuilder`](crate::BundleBuilder).** They are checked for the reason
/// the `ByteRange::Full` arm above is: `Region` is a durable-format type, and
/// the two ways its two fields can contradict each other both produce an
/// object a reader must reject —
///
/// - `alg != None` with no envelope writes a sealed region nobody holds a key
///   for: the data is durable, billed, and unreadable forever;
/// - `alg == None` with an envelope writes bytes a reader will parse as the
///   *next* region's fields, which is the shape that has one topic's consumer
///   served another topic's records.
fn encode_envelope(region: &Region, out: &mut Vec<u8>) -> Result<()> {
    match (region.alg, region.envelope.as_ref()) {
        (RegionAlg::None, None) => Ok(()),
        (RegionAlg::None, Some(_)) => Err(Error::RegionEnvelopeMismatch { sealed: false }),
        (RegionAlg::Aes256Gcm, None) => Err(Error::RegionEnvelopeMismatch { sealed: true }),
        (RegionAlg::Aes256Gcm, Some(envelope)) => {
            let key_id = envelope.key_id().as_str().as_bytes();
            let key_len = u16::try_from(key_id.len()).map_err(|_| Error::BundleTooLarge)?;
            out.extend_from_slice(&key_len.to_be_bytes());
            out.extend_from_slice(key_id);
            let wrapped = envelope.wrapped_dek().as_redacted().expose();
            // Bounded by `RegionEnvelope::new`, which refuses anything above
            // `MAX_WRAPPED_DEK_LEN`; this conversion cannot fail, and is a
            // `try_from` rather than an `as` so that it stays that way if the
            // bound ever moves.
            let wrapped_len = u32::try_from(wrapped.len()).map_err(|_| Error::BundleTooLarge)?;
            out.extend_from_slice(&wrapped_len.to_be_bytes());
            out.extend_from_slice(wrapped);
            out.extend_from_slice(envelope.nonce().as_bytes());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests;

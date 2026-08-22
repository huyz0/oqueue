//! The compression seam: one bounded decode per wired codec.
//!
//! Split from `records.rs` by concept at the 500-line rule: the seam knows
//! codecs and bounds and nothing about record shapes. See `records.rs`'s
//! module doc for which codecs are wired, why every one is pure Rust, and
//! why zstd *compression* is deliberately absent.

use crate::attributes::Compression;
use crate::records::RecordsError;
use std::borrow::Cow;
use std::io::Read;

/// Decompresses a records blob per `codec`, bounded by `max_out` bytes.
///
/// `Compression::None` borrows; everything else allocates at most
/// `max_out`.
///
/// # Errors
/// [`RecordsError::UnsupportedCodec`] for a codec this build does not
/// carry (including [`Compression::Unknown`] bits),
/// [`RecordsError::DecompressedTooLarge`] at the bound, and
/// [`RecordsError::Decompress`] when the codec refuses the bytes.
pub fn decompress_records(
    codec: Compression,
    blob: &[u8],
    max_out: u64,
) -> Result<Cow<'_, [u8]>, RecordsError> {
    match codec {
        Compression::None => Ok(Cow::Borrowed(blob)),
        Compression::Lz4 => {
            bounded_read(codec, lz4_flex::frame::FrameDecoder::new(blob), max_out).map(Cow::Owned)
        }
        Compression::Zstd => {
            let decoder = ruzstd::decoding::StreamingDecoder::new(blob).map_err(|e| {
                RecordsError::Decompress {
                    codec,
                    detail: e.to_string(),
                }
            })?;
            bounded_read(codec, decoder, max_out).map(Cow::Owned)
        }
        #[cfg(feature = "gzip")]
        Compression::Gzip => {
            bounded_read(codec, flate2::read::GzDecoder::new(blob), max_out).map(Cow::Owned)
        }
        #[cfg(feature = "snappy")]
        Compression::Snappy => snappy::decompress(blob, max_out).map(Cow::Owned),
        other => Err(RecordsError::UnsupportedCodec(other)),
    }
}

/// Reads `src` to its end or the bound, whichever comes first — the bound
/// coming first is the error.
fn bounded_read<R: Read>(
    codec: Compression,
    src: R,
    max_out: u64,
) -> Result<Vec<u8>, RecordsError> {
    let mut out = Vec::new();
    let mut limited = src.take(max_out.saturating_add(1));
    limited
        .read_to_end(&mut out)
        .map_err(|e| RecordsError::Decompress {
            codec,
            detail: e.to_string(),
        })?;
    if u64::try_from(out.len()).unwrap_or(u64::MAX) > max_out {
        return Err(RecordsError::DecompressedTooLarge { max: max_out });
    }
    Ok(out)
}

/// Kafka's snappy is two wire shapes, and the framing logic here is ours:
/// the Java client wraps blocks in xerial snappy-java stream framing (an
/// 8-byte magic, two i32 version fields, then `[i32 length, raw block]`
/// chunks), while librdkafka sends one bare raw block. Detection is by the
/// xerial magic; both paths bound every allocation by the caller's cap
/// *before* allocating (`snap`'s `decompress_len` reads only a header).
#[cfg(feature = "snappy")]
mod snappy {
    use super::RecordsError;
    use crate::attributes::Compression;
    use crate::wire::Cursor;

    const XERIAL_MAGIC: &[u8; 8] = &[0x82, b'S', b'N', b'A', b'P', b'P', b'Y', 0x00];

    fn raw_block(blob: &[u8], budget: u64) -> Result<Vec<u8>, RecordsError> {
        let len = snap::raw::decompress_len(blob).map_err(|e| RecordsError::Decompress {
            codec: Compression::Snappy,
            detail: e.to_string(),
        })?;
        if u64::try_from(len).unwrap_or(u64::MAX) > budget {
            return Err(RecordsError::DecompressedTooLarge { max: budget });
        }
        let mut out = vec![0u8; len];
        snap::raw::Decoder::new()
            .decompress(blob, &mut out)
            .map_err(|e| RecordsError::Decompress {
                codec: Compression::Snappy,
                detail: e.to_string(),
            })?;
        Ok(out)
    }

    pub(super) fn decompress(blob: &[u8], max_out: u64) -> Result<Vec<u8>, RecordsError> {
        if !blob.starts_with(XERIAL_MAGIC) {
            return raw_block(blob, max_out);
        }
        let mut cur = Cursor::new(blob);
        // Magic + two i32s the format never made meaningful.
        cur.take(8).map_err(RecordsError::Decode)?;
        cur.read_i32().map_err(RecordsError::Decode)?;
        cur.read_i32().map_err(RecordsError::Decode)?;
        let mut out = Vec::new();
        while cur.remaining() > 0 {
            // Each chunk is our own bounded length-prefixed read; the block
            // then decompresses under whatever budget remains.
            let chunk = cur
                .read_length_prefixed(u64::try_from(cur.remaining()).unwrap_or(u64::MAX))
                .map_err(RecordsError::Decode)?;
            let remaining_budget =
                max_out.saturating_sub(u64::try_from(out.len()).unwrap_or(u64::MAX));
            let block = raw_block(chunk, remaining_budget)?;
            out.extend_from_slice(&block);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::decompress_records;
    use crate::attributes::Compression;
    use crate::records::tests::golden_records_blob;
    use crate::records::{RecordsError, iter_records};

    /// An LZ4 *frame* (the format Kafka uses), via the encoder's Write
    /// surface — `lz4_flex` exposes no one-shot frame compressor.
    fn lz4_frame(data: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut enc = lz4_flex::frame::FrameEncoder::new(Vec::new());
        enc.write_all(data).expect("compressing into a Vec");
        enc.finish().expect("a finished frame")
    }

    #[test]
    fn an_lz4_frame_round_trips_through_the_seam() {
        let blob = golden_records_blob();
        let compressed = lz4_frame(&blob);
        let out =
            decompress_records(Compression::Lz4, &compressed, 1 << 20).expect("lz4 frame decodes");
        assert_eq!(&*out, &blob[..]);
        let records: Vec<_> = iter_records(&out)
            .collect::<Result<_, _>>()
            .expect("records survive the round trip");
        assert_eq!(records.len(), 2);
    }

    /// The zstd fixture was produced by an independent implementation —
    /// `CPython` 3.14's stdlib (`compression.zstd`, backed by `libzstd`):
    ///
    /// ```text
    /// plain = b"the oqueue zstd seam fixture: " * 8
    /// compression.zstd.compress(plain, level=3).hex()
    /// ```
    ///
    /// so `ruzstd` agreeing with it is a cross-implementation check, not a
    /// round trip through one codebase.
    #[test]
    fn the_zstd_fixture_from_an_independent_encoder_decodes() {
        let frame: Vec<u8> = {
            let hex = "28b52ffd20f02d0100f0746865206f7175657565207a737464207365616d2066\
                       6978747572653a2001003e03a59c";
            (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("fixture hex"))
                .collect()
        };
        let plain: Vec<u8> = b"the oqueue zstd seam fixture: ".repeat(8);
        let out =
            decompress_records(Compression::Zstd, &frame, 1024).expect("the real frame decodes");
        assert_eq!(&*out, &plain[..]);
        // And the bound stops it: one byte under the plaintext size.
        assert!(matches!(
            decompress_records(Compression::Zstd, &frame, 239),
            Err(RecordsError::DecompressedTooLarge { max: 239 })
        ));
    }

    #[test]
    fn a_bomb_dies_on_the_bound_not_in_the_allocator() {
        let big = vec![0u8; 1 << 16];
        let compressed = lz4_frame(&big);
        assert!(compressed.len() < 1024, "highly compressible on purpose");
        assert!(matches!(
            decompress_records(Compression::Lz4, &compressed, 4096),
            Err(RecordsError::DecompressedTooLarge { max: 4096 })
        ));
    }

    #[test]
    fn a_codec_this_build_does_not_carry_is_refused_loudly() {
        #[cfg(not(feature = "gzip"))]
        assert!(matches!(
            decompress_records(Compression::Gzip, &[0x1F, 0x8B], 1024),
            Err(RecordsError::UnsupportedCodec(Compression::Gzip))
        ));
        assert!(matches!(
            decompress_records(Compression::Unknown(5), &[], 1024),
            Err(RecordsError::UnsupportedCodec(Compression::Unknown(5)))
        ));
    }

    /// Behind the feature and run by `--all-features` (by hand today, and
    /// by `M2.25`'s gate): a gzip fixture from `CPython`'s stdlib — an
    /// independent implementation, same discipline as the zstd fixture.
    ///
    /// ```text
    /// gzip.compress(b"the oqueue gzip seam fixture: " * 4, mtime=0).hex()
    /// ```
    #[cfg(feature = "gzip")]
    #[test]
    fn the_gzip_fixture_from_an_independent_encoder_decodes() {
        let frame = hex_bytes(
            "1f8b08000000000002ff2bc94855c82f2c4d2d4d5548afca2c50284e4dcc5548\
             cbac28292d4ab55228a1992c00f41711ea78000000",
        );
        let plain: Vec<u8> = b"the oqueue gzip seam fixture: ".repeat(4);
        let out =
            decompress_records(Compression::Gzip, &frame, 1024).expect("the real frame decodes");
        assert_eq!(&*out, &plain[..]);
        assert!(matches!(
            decompress_records(Compression::Gzip, &frame, 119),
            Err(RecordsError::DecompressedTooLarge { max: 119 })
        ));
    }

    /// Behind the feature, same runners: the Java client's xerial framing
    /// (magic, two version i32s, `[len, raw block]` chunks) assembled by
    /// hand around `snap` raw blocks, plus librdkafka's bare-block shape.
    #[cfg(feature = "snappy")]
    #[test]
    fn both_snappy_wire_shapes_decode() {
        let plain: Vec<u8> = b"the oqueue snappy seam fixture: ".repeat(4);
        let block = snap::raw::Encoder::new()
            .compress_vec(&plain)
            .expect("compressible");

        // librdkafka's shape: one bare raw block.
        let out = decompress_records(Compression::Snappy, &block, 1024).expect("raw decodes");
        assert_eq!(&*out, &plain[..]);

        // The Java client's shape: xerial framing, two chunks.
        let mut xerial = vec![0x82, b'S', b'N', b'A', b'P', b'P', b'Y', 0x00];
        crate::wire::put_i32(&mut xerial, 1);
        crate::wire::put_i32(&mut xerial, 1);
        for half in [&plain[..64], &plain[64..]] {
            let b = snap::raw::Encoder::new()
                .compress_vec(half)
                .expect("compressible");
            crate::wire::put_i32(&mut xerial, i32::try_from(b.len()).expect("small"));
            xerial.extend_from_slice(&b);
        }
        let out = decompress_records(Compression::Snappy, &xerial, 1024).expect("xerial decodes");
        assert_eq!(&*out, &plain[..]);
        // And the cap holds across chunks.
        assert!(matches!(
            decompress_records(Compression::Snappy, &xerial, 100),
            Err(RecordsError::DecompressedTooLarge { .. })
        ));
    }

    fn hex_bytes(hex: &str) -> Vec<u8> {
        let clean: String = hex.chars().filter(|c| !c.is_whitespace()).collect();
        (0..clean.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&clean[i..i + 2], 16).expect("fixture hex"))
            .collect()
    }
}

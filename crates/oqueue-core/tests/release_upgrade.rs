//! M13.13: an old reader must refuse object formats it cannot interpret.

use oqueue_core::{Error, parse_composite, parse_footer, parse_partition_manifest};

fn trailer(version: u8, magic: [u8; 4]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(13);
    bytes.extend_from_slice(&1_u32.to_be_bytes());
    bytes.push(version);
    bytes.extend_from_slice(&0_u32.to_be_bytes());
    bytes.extend_from_slice(&magic);
    bytes
}

fn bundle_trailer(version: u8) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(9);
    bytes.extend_from_slice(&1_u32.to_be_bytes());
    bytes.push(version);
    bytes.extend_from_slice(&0_u32.to_be_bytes());
    bytes
}

#[test]
fn unknown_bundle_version_is_refused_before_decoding_regions() {
    let bytes = bundle_trailer(0xff);
    assert!(matches!(
        parse_footer(&bytes, 9),
        Err(Error::UnknownBundleFormat { version: 0xff })
    ));
}

#[test]
fn unknown_composite_version_is_refused_before_decoding_components() {
    let bytes = trailer(0xff, *b"OQCM");
    assert!(matches!(
        parse_composite(&bytes),
        Err(Error::UnknownCompositeVersion { version: 0xff })
    ));
}

#[test]
fn unknown_partition_manifest_version_is_refused_before_decoding_entries() {
    let bytes = trailer(0xff, *b"OQPM");
    assert!(matches!(
        parse_partition_manifest(&bytes),
        Err(Error::UnknownPartitionManifestVersion { version: 0xff })
    ));
}

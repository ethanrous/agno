//! Shared helpers for building synthetic DICOM Part-10 byte streams in tests.
//! Used by the parse/decode unit tests, the loader-bridge tests, and the FFI
//! tests so the framing logic lives in exactly one place.

/// Encode an Explicit-VR short-form element (2-byte length).
pub(crate) fn short(group: u16, elem: u16, vr: &[u8; 2], val: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&group.to_le_bytes());
    v.extend_from_slice(&elem.to_le_bytes());
    v.extend_from_slice(vr);
    v.extend_from_slice(&(val.len() as u16).to_le_bytes());
    v.extend_from_slice(val);
    v
}

/// Even-length-pad a value (DICOM values must be even length).
pub(crate) fn pad(mut s: Vec<u8>, p: u8) -> Vec<u8> {
    if !s.len().is_multiple_of(2) {
        s.push(p);
    }
    s
}

/// Encode an unsigned-short (US) value.
pub(crate) fn us(v: u16) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

/// Wrap a dataset + pixel block into a minimal Explicit-VR-LE Part-10 file using
/// the given pixel-data tag (so error-path tests can use a non-pixel tag).
pub(crate) fn make_part10_with_tag(dataset: &[u8], pixel_tag: (u16, u16), pixel: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; 128];
    out.extend_from_slice(b"DICM");
    // file meta: (0002,0010) UI transfer syntax = Explicit VR LE
    let ts = pad(b"1.2.840.10008.1.2.1".to_vec(), 0);
    let meta = short(0x0002, 0x0010, b"UI", &ts);
    out.extend_from_slice(&short(
        0x0002,
        0x0000,
        b"UL",
        &(meta.len() as u32).to_le_bytes(),
    ));
    out.extend_from_slice(&meta);
    out.extend_from_slice(dataset);
    // pixel data as OW (long form)
    out.extend_from_slice(&pixel_tag.0.to_le_bytes());
    out.extend_from_slice(&pixel_tag.1.to_le_bytes());
    out.extend_from_slice(b"OW");
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&(pixel.len() as u32).to_le_bytes());
    out.extend_from_slice(pixel);
    out
}

/// Wrap a dataset + pixel block into a minimal Explicit-VR-LE Part-10 file with
/// the standard Pixel Data tag (7FE0,0010).
pub(crate) fn make_part10(dataset: &[u8], pixel: &[u8]) -> Vec<u8> {
    make_part10_with_tag(dataset, (0x7FE0, 0x0010), pixel)
}

//! Guards for allocations whose sizes come from untrusted file headers.
//!
//! Decoders must not size frame buffers directly from header-declared
//! dimensions: a corrupt or malicious file can claim arbitrary sizes, and a
//! failed allocation aborts the whole process (fatal for the Go server that
//! embeds agno via CGO). Instead, validate dimensions with [`check_dims`] and
//! allocate with [`try_vec`] / [`try_with_capacity`], which return an error
//! on overflow or allocation failure.

use std::error::Error;
use std::fmt;

/// Maximum accepted image dimension (width or height), in pixels (2^20).
pub const MAX_DIMENSION: u64 = 1_048_576;

/// Maximum accepted total pixel count per image (~1.07G pixels, ~3 GB RGB8).
pub const MAX_PIXELS: u64 = 1 << 30;

/// Error returned when a dimension check or guarded allocation fails.
#[derive(Debug)]
pub struct GuardError(String);

impl fmt::Display for GuardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for GuardError {}

/// Validate header-declared image dimensions before any buffer is sized from
/// them. Rejects zero and anything beyond [`MAX_DIMENSION`] / [`MAX_PIXELS`].
pub fn check_dims(width: u64, height: u64) -> Result<(), GuardError> {
    if width == 0 || height == 0 {
        return Err(GuardError(format!(
            "image dimensions {width}x{height} have a zero side"
        )));
    }
    if width > MAX_DIMENSION || height > MAX_DIMENSION || width * height > MAX_PIXELS {
        return Err(GuardError(format!(
            "image dimensions {width}x{height} exceed accepted limits"
        )));
    }
    Ok(())
}

/// Fallible replacement for `vec![value; len]`: returns an error instead of
/// aborting the process when the allocation cannot be satisfied.
pub fn try_vec<T: Clone>(value: T, len: usize) -> Result<Vec<T>, GuardError> {
    // Probe the allocation fallibly first, then let `vec![]` perform the
    // real allocation: for zero values it lowers to alloc_zeroed, which
    // hands back untouched zero pages instead of memsetting the buffer.
    drop(try_with_capacity::<T>(len)?);
    Ok(vec![value; len])
}

/// Fallible replacement for `Vec::with_capacity`.
pub fn try_with_capacity<T>(capacity: usize) -> Result<Vec<T>, GuardError> {
    let mut v = Vec::new();
    v.try_reserve_exact(capacity).map_err(|e| {
        GuardError(format!(
            "failed to allocate buffer of {capacity} elements: {e}"
        ))
    })?;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normal_dimensions() {
        check_dims(4032, 3024).expect("typical photo dimensions should pass");
    }

    #[test]
    fn rejects_zero_dimensions() {
        assert!(check_dims(0, 1080).is_err());
        assert!(check_dims(1920, 0).is_err());
    }

    #[test]
    fn rejects_absurd_dimensions() {
        assert!(check_dims(1 << 20, 1 << 20).is_err());
    }

    #[test]
    fn rejects_oversized_pixel_count_with_sane_dimensions() {
        assert!(check_dims(MAX_DIMENSION, MAX_DIMENSION).is_err());
    }

    #[test]
    fn accepts_long_skinny_dimensions() {
        check_dims(100_000, 50).expect("long skinny panorama should pass");
    }

    #[test]
    fn accepts_large_document_scan_dimensions() {
        check_dims(30_000, 20_000).expect("large document scan should pass");
    }

    #[test]
    fn try_vec_allocates_small_buffers() {
        let v = try_vec(7u8, 16).expect("small allocation should succeed");
        assert_eq!(v, vec![7u8; 16]);
    }

    #[test]
    fn try_vec_errors_instead_of_aborting_on_huge_len() {
        assert!(try_vec(0u8, usize::MAX).is_err());
    }

    #[test]
    fn try_with_capacity_errors_instead_of_aborting_on_huge_capacity() {
        assert!(try_with_capacity::<u64>(usize::MAX).is_err());
    }
}

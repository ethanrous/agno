//! Modality LUT (rescale) and linear VOI LUT (window/level) math for DICOM
//! rendering. Pure functions, no I/O. See DICOM PS3.3 C.11.

/// Modality LUT: stored sample value -> modality value (PS3.3 C.11.1).
pub fn modality(stored: f64, slope: f64, intercept: f64) -> f64 {
    stored * slope + intercept
}

/// Apply the DICOM linear VOI LUT (window center/width) to a modality value,
/// returning an 8-bit display value in [0, 255]. PS3.3 C.11.2.1.2 (LINEAR).
///
/// Rounding is round-half-up (`floor(y * 255 + 0.5)`); this must match the
/// reference-render generator so committed fixtures line up.
pub fn window_to_u8(value: f64, center: f64, width: f64) -> u8 {
    let w = if width < 1.0 { 1.0 } else { width };
    let lo = center - 0.5 - (w - 1.0) / 2.0;
    let hi = center - 0.5 + (w - 1.0) / 2.0;
    // When width == 1.0, lo == hi, so the else (divide-by-(w - 1)) branch is
    // unreachable; values at/below lo are black, above are white.
    let y = if value <= lo {
        0.0
    } else if value > hi {
        1.0
    } else {
        (value - (center - 0.5)) / (w - 1.0) + 0.5
    };
    // y is bounded to [0, 1] by the branches above, so the result is in [0, 255].
    // Clamp before the cast so float roundoff at the window endpoints can never
    // land outside the byte range (the `as u8` cast also saturates in modern Rust).
    (y * 255.0 + 0.5).floor().clamp(0.0, 255.0) as u8
}

/// Derive a (center, width) window from the min/max of modality values, used
/// when the dataset carries no WindowCenter/WindowWidth. Width is clamped to >= 1.
pub fn auto_window(min_v: f64, max_v: f64) -> (f64, f64) {
    let width = (max_v - min_v).max(1.0);
    let center = min_v + width / 2.0;
    (center, width)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modality_applies_slope_and_intercept() {
        assert!((modality(100.0, 5.0, 7.0) - 507.0).abs() < 1e-9);
        assert!((modality(0.0, 1.0, -1024.0) + 1024.0).abs() < 1e-9);
    }

    #[test]
    fn window_center_maps_to_mid_gray() {
        // The window center should land at ~128 (mid gray) for a wide window.
        assert_eq!(window_to_u8(1215.0, 1215.0, 2113.0), 128);
    }

    #[test]
    fn window_clamps_below_and_above() {
        // Far below the window -> black; far above -> white.
        assert_eq!(window_to_u8(-10000.0, 1215.0, 2113.0), 0);
        assert_eq!(window_to_u8(1_000_000.0, 1215.0, 2113.0), 255);
    }

    #[test]
    fn window_full_range_endpoints() {
        // center=128, width=256 -> input 0 maps to 0, input 255 maps to 255.
        assert_eq!(window_to_u8(0.0, 128.0, 256.0), 0);
        assert_eq!(window_to_u8(255.0, 128.0, 256.0), 255);
        assert_eq!(window_to_u8(128.0, 128.0, 256.0), 128);
    }

    #[test]
    fn window_width_one_is_binary() {
        // width == 1 => lo == hi; the divide-by-(w-1) branch is unreachable.
        // At/below lo -> black, above -> white.
        assert_eq!(window_to_u8(127.5, 128.0, 1.0), 0);
        assert_eq!(window_to_u8(128.5, 128.0, 1.0), 255);
    }

    #[test]
    fn auto_window_spans_min_to_max() {
        let (c, w) = auto_window(0.0, 2577.0);
        assert!((w - 2577.0).abs() < 1e-9);
        assert!((c - 1288.5).abs() < 1e-9);
        // The min maps to black, the max maps to white under this window.
        assert_eq!(window_to_u8(0.0, c, w), 0);
        assert_eq!(window_to_u8(2577.0, c, w), 255);
    }

    #[test]
    fn auto_window_handles_flat_image() {
        // min == max must not divide by zero; width is clamped to >= 1.
        let (_c, w) = auto_window(50.0, 50.0);
        assert!(w >= 1.0);
    }
}

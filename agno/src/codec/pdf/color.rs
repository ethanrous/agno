/// Color space representations for PDF rendering.
#[derive(Debug, Clone)]
pub enum ColorSpace {
    DeviceRGB,
    DeviceGray,
    DeviceCMYK,
    CalRGB,
    CalGray,
    ICCBased { num_components: u8 },
}

impl ColorSpace {
    pub fn num_components(&self) -> usize {
        match self {
            ColorSpace::DeviceGray
            | ColorSpace::CalGray
            | ColorSpace::ICCBased { num_components: 1 } => 1,
            ColorSpace::DeviceRGB
            | ColorSpace::CalRGB
            | ColorSpace::ICCBased { num_components: 3 } => 3,
            ColorSpace::DeviceCMYK | ColorSpace::ICCBased { num_components: 4 } => 4,
            ColorSpace::ICCBased { num_components: n } => *n as usize,
        }
    }
}

/// Convert color components to RGB (0.0–1.0 range).
pub fn to_rgb(space: &ColorSpace, components: &[f64]) -> (f64, f64, f64) {
    let get = |i: usize| components.get(i).copied().unwrap_or(0.0);
    match space {
        ColorSpace::DeviceRGB | ColorSpace::CalRGB | ColorSpace::ICCBased { num_components: 3 } => {
            (get(0), get(1), get(2))
        }
        ColorSpace::DeviceGray
        | ColorSpace::CalGray
        | ColorSpace::ICCBased { num_components: 1 } => {
            let g = get(0);
            (g, g, g)
        }
        ColorSpace::DeviceCMYK | ColorSpace::ICCBased { num_components: 4 } => {
            let (c, m, y, k) = (get(0), get(1), get(2), get(3));
            (
                (1.0 - c) * (1.0 - k),
                (1.0 - m) * (1.0 - k),
                (1.0 - y) * (1.0 - k),
            )
        }
        _ => (0.0, 0.0, 0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_rgb_passthrough() {
        let (r, g, b) = to_rgb(&ColorSpace::DeviceRGB, &[0.5, 0.25, 0.75]);
        assert!((r - 0.5).abs() < 1e-9);
        assert!((g - 0.25).abs() < 1e-9);
        assert!((b - 0.75).abs() < 1e-9);
    }

    #[test]
    fn device_gray_to_rgb() {
        let (r, g, b) = to_rgb(&ColorSpace::DeviceGray, &[0.5]);
        assert!((r - 0.5).abs() < 1e-9);
        assert!((g - 0.5).abs() < 1e-9);
        assert!((b - 0.5).abs() < 1e-9);
    }

    #[test]
    fn device_cmyk_cyan() {
        let (r, g, b) = to_rgb(&ColorSpace::DeviceCMYK, &[1.0, 0.0, 0.0, 0.0]);
        assert!(r < 0.01);
        assert!((g - 1.0).abs() < 0.01);
        assert!((b - 1.0).abs() < 0.01);
    }

    #[test]
    fn cmyk_black() {
        let (r, g, b) = to_rgb(&ColorSpace::DeviceCMYK, &[0.0, 0.0, 0.0, 1.0]);
        assert!(r < 0.01);
        assert!(g < 0.01);
        assert!(b < 0.01);
    }

    #[test]
    fn num_components() {
        assert_eq!(ColorSpace::DeviceGray.num_components(), 1);
        assert_eq!(ColorSpace::DeviceRGB.num_components(), 3);
        assert_eq!(ColorSpace::DeviceCMYK.num_components(), 4);
        assert_eq!(
            (ColorSpace::ICCBased { num_components: 3 }).num_components(),
            3
        );
    }

    #[test]
    fn icc_3_component_as_rgb() {
        let (r, g, b) = to_rgb(
            &ColorSpace::ICCBased { num_components: 3 },
            &[0.1, 0.2, 0.3],
        );
        assert!((r - 0.1).abs() < 1e-9);
        assert!((g - 0.2).abs() < 1e-9);
        assert!((b - 0.3).abs() < 1e-9);
    }
}

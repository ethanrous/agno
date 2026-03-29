//! System font discovery and caching for Standard 14 font rendering.
//!
//! Loads system-installed TrueType/OpenType fonts as substitutes for the
//! PDF Standard 14 fonts (Helvetica, Times, Courier and variants).

use std::collections::HashMap;
use std::sync::OnceLock;

/// Cached system font data for Standard 14 font rendering.
/// Keyed by font category: "sans", "serif", "mono".
static SYSTEM_FONT_CACHE: OnceLock<HashMap<&'static str, Vec<u8>>> = OnceLock::new();

/// Try to load a system font matching a Standard 14 font name.
/// Returns cached font data on success, None if no suitable system font found.
pub(super) fn system_font_for_standard14(name: &str) -> Option<&'static Vec<u8>> {
    let cache = SYSTEM_FONT_CACHE.get_or_init(|| {
        let mut map = HashMap::new();
        // Try sans-serif fonts (Helvetica substitutes)
        for path in SANS_FONT_PATHS {
            if let Ok(data) = std::fs::read(path) {
                map.insert("sans", data);
                break;
            }
        }
        // Try serif fonts (Times substitutes)
        for path in SERIF_FONT_PATHS {
            if let Ok(data) = std::fs::read(path) {
                map.insert("serif", data);
                break;
            }
        }
        // Try monospace fonts (Courier substitutes)
        for path in MONO_FONT_PATHS {
            if let Ok(data) = std::fs::read(path) {
                map.insert("mono", data);
                break;
            }
        }
        map
    });

    let category = match name {
        n if n.starts_with("Courier") => "mono",
        n if n.starts_with("Times") => "serif",
        _ => "sans", // Helvetica and everything else
    };
    cache.get(category)
}

// System font search paths, in priority order.
#[cfg(target_os = "macos")]
const SANS_FONT_PATHS: &[&str] = &[
    "/System/Library/Fonts/Helvetica.ttc",
    "/System/Library/Fonts/SFNSText.ttf",
    "/Library/Fonts/Arial.ttf",
];
#[cfg(target_os = "macos")]
const SERIF_FONT_PATHS: &[&str] = &[
    "/System/Library/Fonts/Times.ttc",
    "/Library/Fonts/Times New Roman.ttf",
];
#[cfg(target_os = "macos")]
const MONO_FONT_PATHS: &[&str] = &[
    "/System/Library/Fonts/Courier.ttc",
    "/System/Library/Fonts/Menlo.ttc",
    "/Library/Fonts/Courier New.ttf",
];

#[cfg(target_os = "linux")]
const SANS_FONT_PATHS: &[&str] = &[
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/liberation-sans/LiberationSans-Regular.ttf",
    "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
];
#[cfg(target_os = "linux")]
const SERIF_FONT_PATHS: &[&str] = &[
    "/usr/share/fonts/truetype/liberation/LiberationSerif-Regular.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf",
    "/usr/share/fonts/liberation-serif/LiberationSerif-Regular.ttf",
];
#[cfg(target_os = "linux")]
const MONO_FONT_PATHS: &[&str] = &[
    "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/liberation-mono/LiberationMono-Regular.ttf",
];

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
const SANS_FONT_PATHS: &[&str] = &[];
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
const SERIF_FONT_PATHS: &[&str] = &[];
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
const MONO_FONT_PATHS: &[&str] = &[];

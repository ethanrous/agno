//! Standard PDF encoding tables and Adobe Glyph List (AGL) for glyph name resolution.
//!
//! Maps character codes to glyph names via standard PDF encodings (WinAnsiEncoding,
//! MacRomanEncoding, StandardEncoding), and resolves glyph names to Unicode code
//! points via the Adobe Glyph List. These mappings enable correct glyph selection
//! when rendering embedded PDF fonts with custom encodings.

/// Map a character code to its glyph name under WinAnsiEncoding (PDF Reference Table D.1).
pub fn winansi_name(code: u8) -> Option<&'static str> {
    WINANSI[code as usize]
}

#[rustfmt::skip]
const WINANSI: [Option<&str>; 256] = [
    // 0x00-0x07
    None, None, None, None, None, None, None, None,
    // 0x08-0x0F
    None, None, None, None, None, None, None, None,
    // 0x10-0x17
    None, None, None, None, None, None, None, None,
    // 0x18-0x1F
    None, None, None, None, None, None, None, None,
    // 0x20-0x27
    Some("space"), Some("exclam"), Some("quotedbl"), Some("numbersign"),
    Some("dollar"), Some("percent"), Some("ampersand"), Some("quotesingle"),
    // 0x28-0x2F
    Some("parenleft"), Some("parenright"), Some("asterisk"), Some("plus"),
    Some("comma"), Some("hyphen"), Some("period"), Some("slash"),
    // 0x30-0x37
    Some("zero"), Some("one"), Some("two"), Some("three"),
    Some("four"), Some("five"), Some("six"), Some("seven"),
    // 0x38-0x3F
    Some("eight"), Some("nine"), Some("colon"), Some("semicolon"),
    Some("less"), Some("equal"), Some("greater"), Some("question"),
    // 0x40-0x47
    Some("at"), Some("A"), Some("B"), Some("C"),
    Some("D"), Some("E"), Some("F"), Some("G"),
    // 0x48-0x4F
    Some("H"), Some("I"), Some("J"), Some("K"),
    Some("L"), Some("M"), Some("N"), Some("O"),
    // 0x50-0x57
    Some("P"), Some("Q"), Some("R"), Some("S"),
    Some("T"), Some("U"), Some("V"), Some("W"),
    // 0x58-0x5F
    Some("X"), Some("Y"), Some("Z"), Some("bracketleft"),
    Some("backslash"), Some("bracketright"), Some("asciicircum"), Some("underscore"),
    // 0x60-0x67
    Some("grave"), Some("a"), Some("b"), Some("c"),
    Some("d"), Some("e"), Some("f"), Some("g"),
    // 0x68-0x6F
    Some("h"), Some("i"), Some("j"), Some("k"),
    Some("l"), Some("m"), Some("n"), Some("o"),
    // 0x70-0x77
    Some("p"), Some("q"), Some("r"), Some("s"),
    Some("t"), Some("u"), Some("v"), Some("w"),
    // 0x78-0x7F
    Some("x"), Some("y"), Some("z"), Some("braceleft"),
    Some("bar"), Some("braceright"), Some("asciitilde"), None,
    // 0x80-0x87 (Windows-1252 extensions)
    Some("Euro"), None, Some("quotesinglbase"), Some("florin"),
    Some("quotedblbase"), Some("ellipsis"), Some("dagger"), Some("daggerdbl"),
    // 0x88-0x8F
    Some("circumflex"), Some("perthousand"), Some("Scaron"), Some("guilsinglleft"),
    Some("OE"), None, Some("Zcaron"), None,
    // 0x90-0x97
    None, Some("quoteleft"), Some("quoteright"), Some("quotedblleft"),
    Some("quotedblright"), Some("bullet"), Some("endash"), Some("emdash"),
    // 0x98-0x9F
    Some("tilde"), Some("trademark"), Some("scaron"), Some("guilsinglright"),
    Some("oe"), None, Some("zcaron"), Some("Ydieresis"),
    // 0xA0-0xA7
    Some("space"), Some("exclamdown"), Some("cent"), Some("sterling"),
    Some("currency"), Some("yen"), Some("brokenbar"), Some("section"),
    // 0xA8-0xAF
    Some("dieresis"), Some("copyright"), Some("ordfeminine"), Some("guillemotleft"),
    Some("logicalnot"), Some("hyphen"), Some("registered"), Some("macron"),
    // 0xB0-0xB7
    Some("degree"), Some("plusminus"), Some("twosuperior"), Some("threesuperior"),
    Some("acute"), Some("mu"), Some("paragraph"), Some("periodcentered"),
    // 0xB8-0xBF
    Some("cedilla"), Some("onesuperior"), Some("ordmasculine"), Some("guillemotright"),
    Some("onequarter"), Some("onehalf"), Some("threequarters"), Some("questiondown"),
    // 0xC0-0xC7
    Some("Agrave"), Some("Aacute"), Some("Acircumflex"), Some("Atilde"),
    Some("Adieresis"), Some("Aring"), Some("AE"), Some("Ccedilla"),
    // 0xC8-0xCF
    Some("Egrave"), Some("Eacute"), Some("Ecircumflex"), Some("Edieresis"),
    Some("Igrave"), Some("Iacute"), Some("Icircumflex"), Some("Idieresis"),
    // 0xD0-0xD7
    Some("Eth"), Some("Ntilde"), Some("Ograve"), Some("Oacute"),
    Some("Ocircumflex"), Some("Otilde"), Some("Odieresis"), Some("multiply"),
    // 0xD8-0xDF
    Some("Oslash"), Some("Ugrave"), Some("Uacute"), Some("Ucircumflex"),
    Some("Udieresis"), Some("Yacute"), Some("Thorn"), Some("germandbls"),
    // 0xE0-0xE7
    Some("agrave"), Some("aacute"), Some("acircumflex"), Some("atilde"),
    Some("adieresis"), Some("aring"), Some("ae"), Some("ccedilla"),
    // 0xE8-0xEF
    Some("egrave"), Some("eacute"), Some("ecircumflex"), Some("edieresis"),
    Some("igrave"), Some("iacute"), Some("icircumflex"), Some("idieresis"),
    // 0xF0-0xF7
    Some("eth"), Some("ntilde"), Some("ograve"), Some("oacute"),
    Some("ocircumflex"), Some("otilde"), Some("odieresis"), Some("divide"),
    // 0xF8-0xFF
    Some("oslash"), Some("ugrave"), Some("uacute"), Some("ucircumflex"),
    Some("udieresis"), Some("yacute"), Some("thorn"), Some("ydieresis"),
];

/// Map a character code to its glyph name under MacRomanEncoding (PDF Reference Table D.2).
pub fn macroman_name(code: u8) -> Option<&'static str> {
    MACROMAN[code as usize]
}

#[rustfmt::skip]
const MACROMAN: [Option<&str>; 256] = [
    // 0x00-0x1F: undefined
    None, None, None, None, None, None, None, None,
    None, None, None, None, None, None, None, None,
    None, None, None, None, None, None, None, None,
    None, None, None, None, None, None, None, None,
    // 0x20-0x7E: same as WinAnsi (standard ASCII)
    Some("space"), Some("exclam"), Some("quotedbl"), Some("numbersign"),
    Some("dollar"), Some("percent"), Some("ampersand"), Some("quotesingle"),
    Some("parenleft"), Some("parenright"), Some("asterisk"), Some("plus"),
    Some("comma"), Some("hyphen"), Some("period"), Some("slash"),
    Some("zero"), Some("one"), Some("two"), Some("three"),
    Some("four"), Some("five"), Some("six"), Some("seven"),
    Some("eight"), Some("nine"), Some("colon"), Some("semicolon"),
    Some("less"), Some("equal"), Some("greater"), Some("question"),
    Some("at"), Some("A"), Some("B"), Some("C"),
    Some("D"), Some("E"), Some("F"), Some("G"),
    Some("H"), Some("I"), Some("J"), Some("K"),
    Some("L"), Some("M"), Some("N"), Some("O"),
    Some("P"), Some("Q"), Some("R"), Some("S"),
    Some("T"), Some("U"), Some("V"), Some("W"),
    Some("X"), Some("Y"), Some("Z"), Some("bracketleft"),
    Some("backslash"), Some("bracketright"), Some("asciicircum"), Some("underscore"),
    Some("grave"), Some("a"), Some("b"), Some("c"),
    Some("d"), Some("e"), Some("f"), Some("g"),
    Some("h"), Some("i"), Some("j"), Some("k"),
    Some("l"), Some("m"), Some("n"), Some("o"),
    Some("p"), Some("q"), Some("r"), Some("s"),
    Some("t"), Some("u"), Some("v"), Some("w"),
    Some("x"), Some("y"), Some("z"), Some("braceleft"),
    Some("bar"), Some("braceright"), Some("asciitilde"), None,
    // 0x80-0x87 (Mac-specific extended characters)
    Some("Adieresis"), Some("Aring"), Some("Ccedilla"), Some("Eacute"),
    Some("Ntilde"), Some("Odieresis"), Some("Udieresis"), Some("aacute"),
    // 0x88-0x8F
    Some("agrave"), Some("acircumflex"), Some("adieresis"), Some("atilde"),
    Some("aring"), Some("ccedilla"), Some("eacute"), Some("egrave"),
    // 0x90-0x97
    Some("ecircumflex"), Some("edieresis"), Some("iacute"), Some("igrave"),
    Some("icircumflex"), Some("idieresis"), Some("ntilde"), Some("oacute"),
    // 0x98-0x9F
    Some("ograve"), Some("ocircumflex"), Some("odieresis"), Some("otilde"),
    Some("uacute"), Some("ugrave"), Some("ucircumflex"), Some("udieresis"),
    // 0xA0-0xA7
    Some("dagger"), Some("degree"), Some("cent"), Some("sterling"),
    Some("section"), Some("bullet"), Some("paragraph"), Some("germandbls"),
    // 0xA8-0xAF
    Some("registered"), Some("copyright"), Some("trademark"), Some("acute"),
    Some("dieresis"), None, Some("AE"), Some("Oslash"),
    // 0xB0-0xB7
    None, Some("plusminus"), None, None,
    Some("yen"), Some("mu"), None, None,
    // 0xB8-0xBF
    None, None, None, Some("ordfeminine"),
    Some("ordmasculine"), None, Some("ae"), Some("oslash"),
    // 0xC0-0xC7
    Some("questiondown"), Some("exclamdown"), Some("logicalnot"), None,
    Some("florin"), None, None, Some("guillemotleft"),
    // 0xC8-0xCF
    Some("guillemotright"), Some("ellipsis"), Some("space"), Some("Agrave"),
    Some("Atilde"), Some("Otilde"), Some("OE"), Some("oe"),
    // 0xD0-0xD7
    Some("endash"), Some("emdash"), Some("quotedblleft"), Some("quotedblright"),
    Some("quoteleft"), Some("quoteright"), Some("divide"), None,
    // 0xD8-0xDF
    Some("ydieresis"), Some("Ydieresis"), Some("fraction"), Some("Euro"),
    Some("guilsinglleft"), Some("guilsinglright"), Some("fi"), Some("fl"),
    // 0xE0-0xE7
    Some("daggerdbl"), Some("periodcentered"), Some("quotesinglbase"), Some("quotedblbase"),
    Some("perthousand"), Some("Acircumflex"), Some("Ecircumflex"), Some("Aacute"),
    // 0xE8-0xEF
    Some("Edieresis"), Some("Egrave"), Some("Iacute"), Some("Icircumflex"),
    Some("Idieresis"), Some("Igrave"), Some("Oacute"), Some("Ocircumflex"),
    // 0xF0-0xF7
    None, Some("Ograve"), Some("Uacute"), Some("Ucircumflex"),
    Some("Ugrave"), Some("dotlessi"), Some("circumflex"), Some("tilde"),
    // 0xF8-0xFF
    Some("macron"), Some("breve"), Some("dotaccent"), Some("ring"),
    Some("cedilla"), Some("hungarumlaut"), Some("ogonek"), Some("caron"),
];

/// Map a character code to its glyph name under StandardEncoding (PDF Reference Table D.1).
pub fn standard_name(code: u8) -> Option<&'static str> {
    STANDARD[code as usize]
}

#[rustfmt::skip]
const STANDARD: [Option<&str>; 256] = [
    // 0x00-0x1F: undefined
    None, None, None, None, None, None, None, None,
    None, None, None, None, None, None, None, None,
    None, None, None, None, None, None, None, None,
    None, None, None, None, None, None, None, None,
    // 0x20-0x27
    Some("space"), Some("exclam"), Some("quotedbl"), Some("numbersign"),
    Some("dollar"), Some("percent"), Some("ampersand"), Some("quoteright"),
    // 0x28-0x2F
    Some("parenleft"), Some("parenright"), Some("asterisk"), Some("plus"),
    Some("comma"), Some("hyphen"), Some("period"), Some("slash"),
    // 0x30-0x37
    Some("zero"), Some("one"), Some("two"), Some("three"),
    Some("four"), Some("five"), Some("six"), Some("seven"),
    // 0x38-0x3F
    Some("eight"), Some("nine"), Some("colon"), Some("semicolon"),
    Some("less"), Some("equal"), Some("greater"), Some("question"),
    // 0x40-0x47
    Some("at"), Some("A"), Some("B"), Some("C"),
    Some("D"), Some("E"), Some("F"), Some("G"),
    // 0x48-0x4F
    Some("H"), Some("I"), Some("J"), Some("K"),
    Some("L"), Some("M"), Some("N"), Some("O"),
    // 0x50-0x57
    Some("P"), Some("Q"), Some("R"), Some("S"),
    Some("T"), Some("U"), Some("V"), Some("W"),
    // 0x58-0x5F
    Some("X"), Some("Y"), Some("Z"), Some("bracketleft"),
    Some("backslash"), Some("bracketright"), Some("asciicircum"), Some("underscore"),
    // 0x60-0x67
    Some("quoteleft"), Some("a"), Some("b"), Some("c"),
    Some("d"), Some("e"), Some("f"), Some("g"),
    // 0x68-0x6F
    Some("h"), Some("i"), Some("j"), Some("k"),
    Some("l"), Some("m"), Some("n"), Some("o"),
    // 0x70-0x77
    Some("p"), Some("q"), Some("r"), Some("s"),
    Some("t"), Some("u"), Some("v"), Some("w"),
    // 0x78-0x7F
    Some("x"), Some("y"), Some("z"), Some("braceleft"),
    Some("bar"), Some("braceright"), Some("asciitilde"), None,
    // 0x80-0xBF: mostly undefined in StandardEncoding
    None, None, None, None, None, None, None, None,
    None, None, None, None, None, None, None, None,
    None, None, None, None, None, None, None, None,
    None, None, None, None, None, None, None, None,
    None, Some("exclamdown"), Some("cent"), Some("sterling"),
    Some("fraction"), Some("yen"), Some("florin"), Some("section"),
    Some("currency"), Some("quotesingle"), Some("quotedblleft"), Some("guillemotleft"),
    Some("guilsinglleft"), Some("guilsinglright"), Some("fi"), Some("fl"),
    None, Some("endash"), Some("dagger"), Some("daggerdbl"),
    Some("periodcentered"), None, Some("paragraph"), Some("bullet"),
    Some("quotesinglbase"), Some("quotedblbase"), Some("quotedblright"), Some("guillemotright"),
    Some("ellipsis"), Some("perthousand"), None, Some("questiondown"),
    // 0xC0-0xCF
    None, Some("grave"), Some("acute"), Some("circumflex"),
    Some("tilde"), Some("macron"), Some("breve"), Some("dotaccent"),
    Some("dieresis"), None, Some("ring"), Some("cedilla"),
    None, Some("hungarumlaut"), Some("ogonek"), Some("caron"),
    // 0xD0-0xDF
    Some("emdash"), None, None, None,
    None, None, None, None,
    None, None, None, None,
    None, None, None, None,
    // 0xE0-0xEF
    None, Some("AE"), None, Some("ordfeminine"),
    None, None, None, None,
    Some("Lslash"), Some("Oslash"), Some("OE"), Some("ordmasculine"),
    None, None, None, None,
    // 0xF0-0xFF
    None, Some("ae"), None, None,
    None, Some("dotlessi"), None, None,
    Some("lslash"), Some("oslash"), Some("oe"), Some("germandbls"),
    None, None, None, None,
];

/// Look up a glyph name in the Adobe Glyph List, returning its Unicode code point.
/// Also handles the algorithmic `uniXXXX` pattern (e.g., "uni00C4" → U+00C4).
pub fn agl_name_to_unicode(name: &str) -> Option<char> {
    // Handle uniXXXX algorithmic pattern (ISO 32000-1 Annex D)
    if name.starts_with("uni") && name.len() == 7 {
        if let Ok(cp) = u32::from_str_radix(&name[3..], 16) {
            return char::from_u32(cp);
        }
    }
    // Handle uXXXX-uYYYY (ligature) or uXXXXX (supplementary) — return first code point
    if name.starts_with('u') && name.len() >= 5 && name.as_bytes()[1].is_ascii_hexdigit() {
        let hex_part = name.split('-').next().unwrap_or(name);
        let hex = &hex_part[1..];
        if hex.len() >= 4 && hex.len() <= 6 {
            if let Ok(cp) = u32::from_str_radix(hex, 16) {
                return char::from_u32(cp);
            }
        }
    }
    // Binary search in sorted AGL table
    AGL_TABLE
        .binary_search_by_key(&name, |&(n, _)| n)
        .ok()
        .and_then(|i| char::from_u32(AGL_TABLE[i].1))
}

/// Adobe Glyph List subset, sorted by name for binary search.
/// Covers all glyph names used in WinAnsi/MacRoman/Standard encodings
/// plus common ligatures and symbols.
#[rustfmt::skip]
static AGL_TABLE: &[(&str, u32)] = &[
    ("A", 0x0041), ("AE", 0x00C6), ("Aacute", 0x00C1), ("Acircumflex", 0x00C2),
    ("Adieresis", 0x00C4), ("Agrave", 0x00C0), ("Aring", 0x00C5), ("Atilde", 0x00C3),
    ("B", 0x0042), ("C", 0x0043), ("Ccedilla", 0x00C7), ("D", 0x0044),
    ("E", 0x0045), ("Eacute", 0x00C9), ("Ecircumflex", 0x00CA), ("Edieresis", 0x00CB),
    ("Egrave", 0x00C8), ("Eth", 0x00D0), ("Euro", 0x20AC), ("F", 0x0046),
    ("G", 0x0047), ("H", 0x0048), ("I", 0x0049), ("Iacute", 0x00CD),
    ("Icircumflex", 0x00CE), ("Idieresis", 0x00CF), ("Igrave", 0x00CC),
    ("J", 0x004A), ("K", 0x004B), ("L", 0x004C), ("Lslash", 0x0141),
    ("M", 0x004D), ("N", 0x004E), ("Ntilde", 0x00D1), ("O", 0x004F),
    ("OE", 0x0152), ("Oacute", 0x00D3), ("Ocircumflex", 0x00D4),
    ("Odieresis", 0x00D6), ("Ograve", 0x00D2), ("Oslash", 0x00D8),
    ("Otilde", 0x00D5), ("P", 0x0050), ("Q", 0x0051), ("R", 0x0052),
    ("S", 0x0053), ("Scaron", 0x0160), ("T", 0x0054), ("Thorn", 0x00DE),
    ("U", 0x0055), ("Uacute", 0x00DA), ("Ucircumflex", 0x00DB),
    ("Udieresis", 0x00DC), ("Ugrave", 0x00D9), ("V", 0x0056), ("W", 0x0057),
    ("X", 0x0058), ("Y", 0x0059), ("Yacute", 0x00DD), ("Ydieresis", 0x0178),
    ("Z", 0x005A), ("Zcaron", 0x017D),
    ("a", 0x0061), ("aacute", 0x00E1), ("acircumflex", 0x00E2), ("acute", 0x00B4),
    ("adieresis", 0x00E4), ("ae", 0x00E6), ("agrave", 0x00E0),
    ("ampersand", 0x0026), ("aring", 0x00E5), ("asciicircum", 0x005E),
    ("asciitilde", 0x007E), ("asterisk", 0x002A), ("at", 0x0040),
    ("atilde", 0x00E3),
    ("b", 0x0062), ("backslash", 0x005C), ("bar", 0x007C),
    ("braceleft", 0x007B), ("braceright", 0x007D), ("bracketleft", 0x005B),
    ("bracketright", 0x005D), ("breve", 0x02D8), ("brokenbar", 0x00A6),
    ("bullet", 0x2022),
    ("c", 0x0063), ("caron", 0x02C7), ("ccedilla", 0x00E7), ("cedilla", 0x00B8),
    ("cent", 0x00A2), ("circumflex", 0x02C6), ("colon", 0x003A),
    ("comma", 0x002C), ("copyright", 0x00A9), ("currency", 0x00A4),
    ("d", 0x0064), ("dagger", 0x2020), ("daggerdbl", 0x2021), ("degree", 0x00B0),
    ("dieresis", 0x00A8), ("divide", 0x00F7), ("dollar", 0x0024),
    ("dotaccent", 0x02D9), ("dotlessi", 0x0131),
    ("e", 0x0065), ("eacute", 0x00E9), ("ecircumflex", 0x00EA),
    ("edieresis", 0x00EB), ("egrave", 0x00E8), ("eight", 0x0038),
    ("ellipsis", 0x2026), ("emdash", 0x2014), ("endash", 0x2013),
    ("equal", 0x003D), ("eth", 0x00F0), ("exclam", 0x0021),
    ("exclamdown", 0x00A1),
    ("f", 0x0066), ("fi", 0xFB01), ("five", 0x0035), ("fl", 0xFB02),
    ("florin", 0x0192), ("four", 0x0034), ("fraction", 0x2044),
    ("g", 0x0067), ("germandbls", 0x00DF), ("grave", 0x0060),
    ("greater", 0x003E), ("guillemotleft", 0x00AB), ("guillemotright", 0x00BB),
    ("guilsinglleft", 0x2039), ("guilsinglright", 0x203A),
    ("h", 0x0068), ("hungarumlaut", 0x02DD), ("hyphen", 0x002D),
    ("i", 0x0069), ("iacute", 0x00ED), ("icircumflex", 0x00EE),
    ("idieresis", 0x00EF), ("igrave", 0x00EC),
    ("j", 0x006A),
    ("k", 0x006B),
    ("l", 0x006C), ("less", 0x003C), ("logicalnot", 0x00AC), ("lslash", 0x0142),
    ("m", 0x006D), ("macron", 0x00AF), ("minus", 0x2212), ("mu", 0x00B5),
    ("multiply", 0x00D7),
    ("n", 0x006E), ("nbspace", 0x00A0), ("nine", 0x0039), ("ntilde", 0x00F1),
    ("numbersign", 0x0023),
    ("o", 0x006F), ("oacute", 0x00F3), ("ocircumflex", 0x00F4),
    ("odieresis", 0x00F6), ("oe", 0x0153), ("ograve", 0x00F2),
    ("ogonek", 0x02DB), ("one", 0x0031), ("onehalf", 0x00BD),
    ("onequarter", 0x00BC), ("onesuperior", 0x00B9), ("ordmasculine", 0x00BA),
    ("ordfeminine", 0x00AA), ("oslash", 0x00F8), ("otilde", 0x00F5),
    ("p", 0x0070), ("paragraph", 0x00B6), ("parenleft", 0x0028),
    ("parenright", 0x0029), ("percent", 0x0025), ("period", 0x002E),
    ("periodcentered", 0x00B7), ("perthousand", 0x2030), ("plus", 0x002B),
    ("plusminus", 0x00B1),
    ("q", 0x0071), ("question", 0x003F), ("questiondown", 0x00BF),
    ("quotedbl", 0x0022), ("quotedblbase", 0x201E), ("quotedblleft", 0x201C),
    ("quotedblright", 0x201D), ("quoteleft", 0x2018), ("quoteright", 0x2019),
    ("quotesinglbase", 0x201A), ("quotesingle", 0x0027),
    ("r", 0x0072), ("registered", 0x00AE), ("ring", 0x02DA),
    ("s", 0x0073), ("scaron", 0x0161), ("section", 0x00A7),
    ("semicolon", 0x003B), ("seven", 0x0037), ("six", 0x0036),
    ("slash", 0x002F), ("space", 0x0020), ("sterling", 0x00A3),
    ("t", 0x0074), ("thorn", 0x00FE), ("three", 0x0033),
    ("threequarters", 0x00BE), ("threesuperior", 0x00B3), ("tilde", 0x02DC),
    ("trademark", 0x2122), ("two", 0x0032), ("twosuperior", 0x00B2),
    ("u", 0x0075), ("uacute", 0x00FA), ("ucircumflex", 0x00FB),
    ("udieresis", 0x00FC), ("ugrave", 0x00F9), ("underscore", 0x005F),
    ("v", 0x0076),
    ("w", 0x0077),
    ("x", 0x0078),
    ("y", 0x0079), ("yacute", 0x00FD), ("ydieresis", 0x00FF), ("yen", 0x00A5),
    ("z", 0x007A), ("zcaron", 0x017E), ("zero", 0x0030),
];

/// Resolve a glyph name to a GlyphId in the given font face.
///
/// Strategy:
/// 1. Search the font's `post` table for a glyph with the matching name.
/// 2. If not found, look up the name in the AGL to get Unicode, then use the font's cmap.
/// 3. Return None if no match found.
pub fn resolve_glyph_by_name(
    face: &ttf_parser::Face,
    name: &str,
) -> Option<ttf_parser::GlyphId> {
    // Strategy 1: Search the post table for a matching glyph name.
    // Most PDF-embedded fonts are subsets with few glyphs, so this is fast.
    let num_glyphs = face.number_of_glyphs();
    for i in 0..num_glyphs {
        let gid = ttf_parser::GlyphId(i);
        if let Some(glyph_name) = face.glyph_name(gid) {
            if glyph_name == name {
                return Some(gid);
            }
        }
    }

    // Strategy 2: AGL name → Unicode → cmap lookup.
    if let Some(unicode_char) = agl_name_to_unicode(name) {
        if let Some(gid) = face.glyph_index(unicode_char) {
            if gid.0 != 0 {
                return Some(gid);
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- WinAnsiEncoding tests ---

    #[test]
    fn winansi_ascii_letters() {
        assert_eq!(winansi_name(65), Some("A"));
        assert_eq!(winansi_name(90), Some("Z"));
        assert_eq!(winansi_name(97), Some("a"));
        assert_eq!(winansi_name(122), Some("z"));
    }

    #[test]
    fn winansi_digits() {
        assert_eq!(winansi_name(48), Some("zero"));
        assert_eq!(winansi_name(57), Some("nine"));
    }

    #[test]
    fn winansi_punctuation() {
        assert_eq!(winansi_name(32), Some("space"));
        assert_eq!(winansi_name(33), Some("exclam"));
        assert_eq!(winansi_name(46), Some("period"));
    }

    #[test]
    fn winansi_extended() {
        assert_eq!(winansi_name(128), Some("Euro"));
        assert_eq!(winansi_name(196), Some("Adieresis"));
        assert_eq!(winansi_name(252), Some("udieresis"));
        assert_eq!(winansi_name(255), Some("ydieresis"));
    }

    #[test]
    fn winansi_undefined() {
        assert_eq!(winansi_name(0), None);
        assert_eq!(winansi_name(127), None);
        assert_eq!(winansi_name(129), None);
    }

    // --- AGL tests ---

    #[test]
    fn agl_simple_ascii() {
        assert_eq!(agl_name_to_unicode("A"), Some('A'));
        assert_eq!(agl_name_to_unicode("z"), Some('z'));
        assert_eq!(agl_name_to_unicode("space"), Some(' '));
    }

    #[test]
    fn agl_accented() {
        assert_eq!(agl_name_to_unicode("Adieresis"), Some('\u{00C4}'));
        assert_eq!(agl_name_to_unicode("eacute"), Some('\u{00E9}'));
        assert_eq!(agl_name_to_unicode("ntilde"), Some('\u{00F1}'));
    }

    #[test]
    fn agl_symbols() {
        assert_eq!(agl_name_to_unicode("Euro"), Some('\u{20AC}'));
        assert_eq!(agl_name_to_unicode("bullet"), Some('\u{2022}'));
        assert_eq!(agl_name_to_unicode("endash"), Some('\u{2013}'));
        assert_eq!(agl_name_to_unicode("fi"), Some('\u{FB01}'));
    }

    #[test]
    fn agl_uni_pattern() {
        assert_eq!(agl_name_to_unicode("uni00C4"), Some('\u{00C4}'));
        assert_eq!(agl_name_to_unicode("uni20AC"), Some('\u{20AC}'));
        assert_eq!(agl_name_to_unicode("uni0041"), Some('A'));
    }

    #[test]
    fn agl_unknown() {
        assert_eq!(agl_name_to_unicode("nonexistentglyph"), None);
    }

    // --- MacRomanEncoding tests ---

    #[test]
    fn macroman_ascii_range() {
        assert_eq!(macroman_name(65), Some("A"));
        assert_eq!(macroman_name(32), Some("space"));
    }

    #[test]
    fn macroman_extended() {
        assert_eq!(macroman_name(128), Some("Adieresis"));
        assert_eq!(macroman_name(130), Some("Ccedilla"));
        assert_eq!(macroman_name(250), Some("dotaccent"));
    }

    // --- StandardEncoding tests ---

    #[test]
    fn standard_ascii_range() {
        assert_eq!(standard_name(65), Some("A"));
        assert_eq!(standard_name(32), Some("space"));
    }

    #[test]
    fn standard_extended() {
        assert_eq!(standard_name(193), Some("grave"));
        assert_eq!(standard_name(164), Some("fraction"));
        assert_eq!(standard_name(241), Some("ae"));
    }
}

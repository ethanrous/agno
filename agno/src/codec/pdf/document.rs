//! PDF document structure: opens a PDF, resolves indirect objects, navigates the
//! page tree, and extracts content streams and resources (ISO 32000-1 §7.7).

use std::cell::RefCell;
use std::collections::HashMap;
use std::error::Error;

use super::lexer::Lexer;
use super::objects::{ObjRef, PdfObject};
use super::xref::{Xref, XrefEntry, parse_xref};

/// Maximum recursion depth for resolving indirect references (prevents loops).
const MAX_RESOLVE_DEPTH: usize = 32;

/// A parsed PDF document providing access to pages and their contents.
pub struct PdfDocument<'a> {
    data: &'a [u8],
    xref: Xref,
    cache: RefCell<HashMap<u32, PdfObject>>,
}

impl<'a> PdfDocument<'a> {
    /// Open a PDF from raw bytes, parsing the cross-reference table and trailer.
    pub fn open(data: &'a [u8]) -> Result<Self, Box<dyn Error>> {
        let xref = parse_xref(data)?;
        Ok(PdfDocument {
            data,
            xref,
            cache: RefCell::new(HashMap::new()),
        })
    }

    /// Resolve an indirect object reference to its parsed value.
    pub fn resolve(&self, obj_ref: ObjRef) -> Result<PdfObject, Box<dyn Error>> {
        self.resolve_with_depth(obj_ref, 0)
    }

    /// If `obj` is a Reference, resolve it. Otherwise return a clone.
    pub fn resolve_value(&self, obj: &PdfObject) -> Result<PdfObject, Box<dyn Error>> {
        match obj.as_reference() {
            Some(r) => self.resolve(r),
            None => Ok(obj.clone()),
        }
    }

    /// Return the number of pages in the document.
    pub fn page_count(&self) -> usize {
        (|| -> Result<usize, Box<dyn Error>> {
            let root = self.trailer_root()?;
            let pages = self.resolve_dict_ref(&root, b"Pages")?;
            Ok(pages.get_i64(b"Count").unwrap_or(0) as usize)
        })()
        .unwrap_or(0)
    }

    /// Return the page dictionary for the given 0-based page index.
    pub fn page_object(&self, page_index: usize) -> Result<PdfObject, Box<dyn Error>> {
        let root = self.trailer_root()?;
        let pages = self.resolve_dict_ref(&root, b"Pages")?;
        self.find_page_in_tree(&pages, page_index)
    }

    /// Return the MediaBox (or CropBox) for a page as (x0, y0, x1, y1).
    pub fn page_media_box(
        &self,
        page_index: usize,
    ) -> Result<(f64, f64, f64, f64), Box<dyn Error>> {
        let page = self.page_object(page_index)?;
        self.find_box_in_hierarchy(&page)
    }

    /// Return the concatenated, decompressed content stream bytes for a page.
    pub fn page_content_stream(&self, page_index: usize) -> Result<Vec<u8>, Box<dyn Error>> {
        let page = self.page_object(page_index)?;
        let contents = page
            .get(b"Contents")
            .ok_or("Page has no /Contents")?
            .clone();
        self.read_content_streams(&contents)
    }

    /// Return the /Resources dictionary for a page, walking up /Parent if needed.
    pub fn page_resources(&self, page_index: usize) -> Result<PdfObject, Box<dyn Error>> {
        let page = self.page_object(page_index)?;
        self.find_resources_in_hierarchy(&page)
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    /// Resolve an indirect reference with bounded recursion depth.
    fn resolve_with_depth(
        &self,
        obj_ref: ObjRef,
        depth: usize,
    ) -> Result<PdfObject, Box<dyn Error>> {
        if depth >= MAX_RESOLVE_DEPTH {
            return Err(format!(
                "Indirect reference recursion limit reached resolving obj {}",
                obj_ref.num
            )
            .into());
        }

        if let Some(cached) = self.cache.borrow().get(&obj_ref.num) {
            return Ok(cached.clone());
        }

        let entry = self
            .xref
            .entries
            .get(&obj_ref.num)
            .ok_or_else(|| format!("Object {} not found in xref", obj_ref.num))?;

        let result = match *entry {
            XrefEntry::InUse { offset, .. } => self.parse_indirect_object(offset as usize, depth),
            XrefEntry::Free => Err(format!("Object {} is a free entry", obj_ref.num).into()),
            XrefEntry::Compressed { stream_obj, index } => {
                self.resolve_compressed_object(stream_obj, index, depth)
            }
        }?;

        self.cache.borrow_mut().insert(obj_ref.num, result.clone());
        Ok(result)
    }

    /// Parse an indirect object at the given byte offset.
    ///
    /// Format: `N G obj <value> endobj`
    /// If the value is a dictionary followed by `stream`, extracts and decodes
    /// the stream data.
    fn parse_indirect_object(
        &self,
        offset: usize,
        depth: usize,
    ) -> Result<PdfObject, Box<dyn Error>> {
        let mut lexer = Lexer::new(self.data);
        lexer.set_position(offset);

        // Read: N G obj
        let _obj_num = lexer.next_object()?.ok_or("Expected object number")?;
        let _gen = lexer.next_object()?.ok_or("Expected generation number")?;
        lexer.expect_keyword(b"obj")?;

        // Read the object value.
        let value = lexer
            .next_object()?
            .ok_or("Expected object value after 'obj'")?;

        // If the value is a Reference, resolve it recursively.
        if let Some(r) = value.as_reference() {
            return self.resolve_with_depth(r, depth + 1);
        }

        // Check if this is a stream object (dictionary followed by "stream" keyword).
        if value.as_dict().is_some() {
            let saved_pos = lexer.position();
            lexer.skip_whitespace();

            // Peek to see if next keyword is "stream".
            let kw_pos = lexer.position();
            if !lexer.at_end() {
                let maybe_kw = lexer.read_keyword();
                if let Ok(kw) = maybe_kw {
                    if kw == b"stream" {
                        return self.extract_stream(value, kw_pos, depth);
                    }
                }
            }
            // Not a stream — restore position (doesn't matter, but clean).
            lexer.set_position(saved_pos);
        }

        Ok(value)
    }

    /// Extract stream data from a dictionary object whose "stream" keyword starts
    /// at `kw_pos` in the file. Handles /Length, /Filter, /DecodeParms.
    fn extract_stream(
        &self,
        dict_obj: PdfObject,
        kw_pos: usize,
        depth: usize,
    ) -> Result<PdfObject, Box<dyn Error>> {
        let dict = dict_obj
            .as_dict()
            .ok_or("Stream object is not a dictionary")?;

        // Position after "stream" keyword.
        let after_kw = kw_pos + b"stream".len();

        // Skip the mandatory newline after "stream" (LF or CRLF).
        let data_start = skip_stream_eol(self.data, after_kw);

        // Get /Length — may be a reference that needs resolving.
        let length_obj = dict
            .get(b"Length".as_slice())
            .ok_or("Stream dictionary missing /Length")?;
        let length = match length_obj.as_reference() {
            Some(r) => {
                let resolved = self.resolve_with_depth(r, depth + 1)?;
                safe_usize(
                    resolved
                        .as_i64()
                        .ok_or("Stream /Length reference did not resolve to integer")?,
                )?
            }
            None => safe_usize(
                length_obj
                    .as_i64()
                    .ok_or("Stream /Length is not an integer")?,
            )?,
        };

        if data_start + length > self.data.len() {
            return Err(format!(
                "Stream data at offset {} with /Length {} exceeds file size {}",
                data_start,
                length,
                self.data.len()
            )
            .into());
        }

        let raw_data = &self.data[data_start..data_start + length];

        // Collect filters.
        let (filter_names, params_list) = self.collect_filters(dict, depth)?;
        let filter_refs: Vec<&[u8]> = filter_names.iter().map(|f| f.as_slice()).collect();
        let params_refs: Vec<Option<&PdfObject>> = params_list.iter().map(|p| p.as_ref()).collect();

        let decoded = if filter_refs.is_empty() {
            raw_data.to_vec()
        } else {
            super::stream::decode_stream(raw_data, &filter_refs, &params_refs)?
        };

        Ok(PdfObject::Stream {
            dict: dict.clone(),
            data: decoded,
        })
    }

    /// Collect /Filter and /DecodeParms from a stream dictionary.
    /// Returns (filter_name_bytes_list, params_list).
    fn collect_filters(
        &self,
        dict: &std::collections::HashMap<Vec<u8>, PdfObject>,
        depth: usize,
    ) -> Result<(Vec<Vec<u8>>, Vec<Option<PdfObject>>), Box<dyn Error>> {
        let filter_obj = match dict.get(b"Filter".as_slice()) {
            Some(obj) => obj.clone(),
            None => return Ok((vec![], vec![])),
        };

        let filter_obj = match filter_obj.as_reference() {
            Some(r) => self.resolve_with_depth(r, depth + 1)?,
            None => filter_obj,
        };

        let filter_names: Vec<Vec<u8>> = match &filter_obj {
            PdfObject::Name(n) => vec![n.clone()],
            PdfObject::Array(arr) => {
                let mut names = Vec::with_capacity(arr.len());
                for item in arr {
                    let resolved = self.resolve_value(item)?;
                    match resolved.as_name() {
                        Some(n) => names.push(n.to_vec()),
                        None => {
                            return Err(
                                format!("Filter array element is not a Name: {resolved}").into()
                            );
                        }
                    }
                }
                names
            }
            _ => return Err(format!("Invalid /Filter value: {filter_obj}").into()),
        };

        // Collect /DecodeParms (parallel to filters).
        let params_list: Vec<Option<PdfObject>> =
            if let Some(dp) = dict.get(b"DecodeParms".as_slice()) {
                let dp = self.resolve_value(dp)?;
                match &dp {
                    PdfObject::Dictionary(_) => vec![Some(dp)],
                    PdfObject::Array(arr) => arr
                        .iter()
                        .map(|item| {
                            let r = self.resolve_value(item)?;
                            if matches!(r, PdfObject::Null) {
                                Ok(None)
                            } else {
                                Ok(Some(r))
                            }
                        })
                        .collect::<Result<Vec<_>, Box<dyn Error>>>()?,
                    PdfObject::Null => vec![None; filter_names.len()],
                    _ => vec![None; filter_names.len()],
                }
            } else {
                vec![None; filter_names.len()]
            };

        Ok((filter_names, params_list))
    }

    /// Resolve a compressed object (stored inside an object stream).
    fn resolve_compressed_object(
        &self,
        stream_obj_num: u32,
        index: u16,
        depth: usize,
    ) -> Result<PdfObject, Box<dyn Error>> {
        // Resolve the containing object stream.
        let stream = self.resolve_with_depth(
            ObjRef {
                num: stream_obj_num,
                gen_num: 0,
            },
            depth + 1,
        )?;

        let (dict, data) = stream.as_stream().ok_or("Object stream is not a Stream")?;

        let n = safe_usize(
            dict.get(b"N".as_slice())
                .and_then(|o| o.as_i64())
                .ok_or("Object stream missing /N")?,
        )?;
        let first = safe_usize(
            dict.get(b"First".as_slice())
                .and_then(|o| o.as_i64())
                .ok_or("Object stream missing /First")?,
        )?;

        if (index as usize) >= n {
            return Err(format!(
                "Compressed object index {} >= /N {} in stream obj {}",
                index, n, stream_obj_num
            )
            .into());
        }

        // Parse the index table: N pairs of (obj_num, byte_offset) at the start of data.
        let mut lexer = Lexer::new(data);
        let mut offsets: Vec<usize> = Vec::with_capacity(n);
        for _ in 0..n {
            let _obj_num = lexer
                .next_object()?
                .and_then(|o| o.as_i64())
                .ok_or("Bad object stream index entry")?;
            let off = safe_usize(
                lexer
                    .next_object()?
                    .and_then(|o| o.as_i64())
                    .ok_or("Bad object stream offset entry")?,
            )?;
            offsets.push(off);
        }

        // Navigate to the index-th object.
        let obj_offset = first + offsets[index as usize];
        lexer.set_position(obj_offset);
        let obj = lexer
            .next_object()?
            .ok_or("Expected object in object stream")?;

        Ok(obj)
    }

    /// Get the /Root catalog from the trailer.
    fn trailer_root(&self) -> Result<PdfObject, Box<dyn Error>> {
        let root_ref = self
            .xref
            .trailer
            .get(b"Root")
            .ok_or("Trailer missing /Root")?;
        self.resolve_value(root_ref)
    }

    /// Resolve a reference stored under `key` in a dictionary.
    fn resolve_dict_ref(&self, dict: &PdfObject, key: &[u8]) -> Result<PdfObject, Box<dyn Error>> {
        let val = dict
            .get(key)
            .ok_or_else(|| format!("Dictionary missing /{}", String::from_utf8_lossy(key)))?;
        self.resolve_value(val)
    }

    /// Recursively walk the page tree to find the page at `target_index`.
    fn find_page_in_tree(
        &self,
        node: &PdfObject,
        target_index: usize,
    ) -> Result<PdfObject, Box<dyn Error>> {
        let kids = node
            .get(b"Kids")
            .and_then(|k| k.as_array())
            .ok_or("Pages node missing /Kids array")?;

        let mut accumulated = 0usize;
        for kid_ref in kids {
            let kid = self.resolve_value(kid_ref)?;
            let kid_type = kid.get_name_str(b"Type").unwrap_or("");

            if kid_type == "Page" {
                if accumulated == target_index {
                    return Ok(kid);
                }
                accumulated += 1;
            } else {
                // Intermediate /Pages node — use /Count to skip if possible.
                let count = kid.get_i64(b"Count").unwrap_or(0) as usize;
                if target_index < accumulated + count {
                    return self.find_page_in_tree(&kid, target_index - accumulated);
                }
                accumulated += count;
            }
        }

        Err(format!(
            "Page index {} out of range (document has {} pages)",
            target_index, accumulated
        )
        .into())
    }

    /// Find /CropBox or /MediaBox, walking up /Parent if not on the page itself.
    fn find_box_in_hierarchy(
        &self,
        page: &PdfObject,
    ) -> Result<(f64, f64, f64, f64), Box<dyn Error>> {
        let mut node = page.clone();
        for _ in 0..MAX_RESOLVE_DEPTH {
            // Prefer /CropBox, fall back to /MediaBox.
            if let Some(b) = node.get(b"CropBox").or_else(|| node.get(b"MediaBox")) {
                let b = self.resolve_value(b)?;
                return parse_rect_array(&b);
            }
            // Walk up /Parent.
            match node.get(b"Parent") {
                Some(parent_ref) => {
                    node = self.resolve_value(parent_ref)?;
                }
                None => break,
            }
        }
        Err("No /MediaBox or /CropBox found in page hierarchy".into())
    }

    /// Resolve /Contents (single reference or array of references) and concatenate.
    fn read_content_streams(&self, contents: &PdfObject) -> Result<Vec<u8>, Box<dyn Error>> {
        match contents {
            PdfObject::Reference(r) => {
                let obj = self.resolve(*r)?;
                match obj {
                    PdfObject::Stream { data, .. } => Ok(data),
                    _ => Err("Content reference did not resolve to a stream".into()),
                }
            }
            PdfObject::Array(arr) => {
                let mut combined = Vec::new();
                for item in arr {
                    let obj = self.resolve_value(item)?;
                    match obj {
                        PdfObject::Stream { data, .. } => {
                            if !combined.is_empty() {
                                combined.push(b'\n');
                            }
                            combined.extend_from_slice(&data);
                        }
                        _ => return Err("Content array element did not resolve to a stream".into()),
                    }
                }
                Ok(combined)
            }
            _ => Err(format!("Unexpected /Contents type: {contents}").into()),
        }
    }

    /// Find /Resources walking up /Parent if not directly on the page.
    fn find_resources_in_hierarchy(&self, page: &PdfObject) -> Result<PdfObject, Box<dyn Error>> {
        let mut node = page.clone();
        for _ in 0..MAX_RESOLVE_DEPTH {
            if let Some(res) = node.get(b"Resources") {
                return self.resolve_value(res);
            }
            match node.get(b"Parent") {
                Some(parent_ref) => {
                    node = self.resolve_value(parent_ref)?;
                }
                None => break,
            }
        }
        // No resources found — return empty dict.
        Ok(PdfObject::Dictionary(std::collections::HashMap::new()))
    }
}

/// Convert an i64 to usize, returning an error for negative or oversized values.
fn safe_usize(val: i64) -> Result<usize, Box<dyn Error>> {
    usize::try_from(val).map_err(|_| format!("Negative or oversized value: {val}").into())
}

/// Skip the mandatory newline after the "stream" keyword.
/// Returns the byte position where stream data begins.
fn skip_stream_eol(data: &[u8], pos: usize) -> usize {
    let mut cur = pos;
    if cur < data.len() && data[cur] == b'\r' {
        cur += 1;
    }
    if cur < data.len() && data[cur] == b'\n' {
        cur += 1;
    }
    cur
}

/// Parse a 4-element array [x0, y0, x1, y1] as f64 values.
fn parse_rect_array(obj: &PdfObject) -> Result<(f64, f64, f64, f64), Box<dyn Error>> {
    let arr = obj.as_array().ok_or("MediaBox/CropBox is not an array")?;
    if arr.len() < 4 {
        return Err(format!("MediaBox/CropBox has {} elements, need 4", arr.len()).into());
    }
    let x0 = arr[0].as_f64().ok_or("MediaBox[0] not a number")?;
    let y0 = arr[1].as_f64().ok_or("MediaBox[1] not a number")?;
    let x1 = arr[2].as_f64().ok_or("MediaBox[2] not a number")?;
    let y1 = arr[3].as_f64().ok_or("MediaBox[3] not a number")?;
    Ok((x0, y0, x1, y1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_simple_pdf() -> Vec<u8> {
        let content = b"1 0 0 rg 0 0 200 100 re f";
        let mut pdf = Vec::new();
        write!(pdf, "%PDF-1.4\n").unwrap();
        let obj1_off = pdf.len();
        write!(pdf, "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n").unwrap();
        let obj2_off = pdf.len();
        write!(
            pdf,
            "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n"
        )
        .unwrap();
        let obj3_off = pdf.len();
        write!(
            pdf,
            "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R >>\nendobj\n"
        )
        .unwrap();
        let obj4_off = pdf.len();
        write!(pdf, "4 0 obj\n<< /Length {} >>\nstream\n", content.len()).unwrap();
        pdf.extend_from_slice(content);
        write!(pdf, "\nendstream\nendobj\n").unwrap();
        let xref_off = pdf.len();
        write!(pdf, "xref\n0 5\n").unwrap();
        write!(pdf, "0000000000 65535 f \n").unwrap();
        write!(pdf, "{:010} 00000 n \n", obj1_off).unwrap();
        write!(pdf, "{:010} 00000 n \n", obj2_off).unwrap();
        write!(pdf, "{:010} 00000 n \n", obj3_off).unwrap();
        write!(pdf, "{:010} 00000 n \n", obj4_off).unwrap();
        write!(pdf, "trailer\n<< /Size 5 /Root 1 0 R >>\n").unwrap();
        write!(pdf, "startxref\n{xref_off}\n%%EOF\n").unwrap();
        pdf
    }

    #[test]
    fn open_document() {
        let pdf = make_simple_pdf();
        let doc = PdfDocument::open(&pdf).unwrap();
        assert_eq!(doc.page_count(), 1);
    }

    #[test]
    fn resolve_object() {
        let pdf = make_simple_pdf();
        let doc = PdfDocument::open(&pdf).unwrap();
        let catalog = doc.resolve(ObjRef { num: 1, gen_num: 0 }).unwrap();
        assert_eq!(catalog.get_name_str(b"Type"), Some("Catalog"));
    }

    #[test]
    fn get_page_mediabox() {
        let pdf = make_simple_pdf();
        let doc = PdfDocument::open(&pdf).unwrap();
        let (x, y, w, h) = doc.page_media_box(0).unwrap();
        assert!((x - 0.0).abs() < 0.01);
        assert!((y - 0.0).abs() < 0.01);
        assert!((w - 200.0).abs() < 0.01);
        assert!((h - 100.0).abs() < 0.01);
    }

    #[test]
    fn get_page_content_stream() {
        let pdf = make_simple_pdf();
        let doc = PdfDocument::open(&pdf).unwrap();
        let content = doc.page_content_stream(0).unwrap();
        let text = std::str::from_utf8(&content).unwrap();
        assert!(text.contains("rg"));
        assert!(text.contains("re f"));
    }

    #[test]
    fn page_out_of_range() {
        let pdf = make_simple_pdf();
        let doc = PdfDocument::open(&pdf).unwrap();
        assert!(doc.page_media_box(99).is_err());
    }

    #[test]
    fn resolve_through_references() {
        let pdf = make_simple_pdf();
        let doc = PdfDocument::open(&pdf).unwrap();
        let page = doc.page_object(0).unwrap();
        assert_eq!(page.get_name_str(b"Type"), Some("Page"));
    }
}

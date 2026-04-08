//! PDF document structure: opens a PDF, resolves indirect objects, navigates the
//! page tree, and extracts content streams and resources (ISO 32000-1 §7.7).

use std::cell::RefCell;
use std::collections::HashMap;
use std::error::Error;

use super::crypt::CryptContext;
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
    crypt: Option<CryptContext>,
}

impl<'a> PdfDocument<'a> {
    /// Open a PDF from raw bytes, parsing the cross-reference table and trailer.
    pub fn open(data: &'a [u8]) -> Result<Self, Box<dyn Error>> {
        let xref = parse_xref(data)?;
        let mut doc = PdfDocument {
            data,
            xref,
            cache: RefCell::new(HashMap::new()),
            crypt: None,
        };
        doc.crypt = doc.setup_encryption()?;
        Ok(doc)
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

    /// Return the resolved annotation objects for a page.
    /// Returns an empty Vec if the page has no /Annots.
    pub fn page_annotations(&self, page_index: usize) -> Result<Vec<PdfObject>, Box<dyn Error>> {
        let page = self.page_object(page_index)?;
        let annots_ref = match page.get(b"Annots") {
            Some(a) => a.clone(),
            None => return Ok(Vec::new()),
        };
        let annots = self.resolve_value(&annots_ref)?;
        let arr = match annots.as_array() {
            Some(a) => a,
            None => return Ok(Vec::new()),
        };
        let mut result = Vec::new();
        for item in arr {
            if let Ok(resolved) = self.resolve_value(item) {
                result.push(resolved);
            }
        }
        Ok(result)
    }

    /// Return the /Resources dictionary for a page, walking up /Parent if needed.
    pub fn page_resources(&self, page_index: usize) -> Result<PdfObject, Box<dyn Error>> {
        let page = self.page_object(page_index)?;
        self.find_resources_in_hierarchy(&page)
    }

    /// Check the trailer for /Encrypt and set up decryption if present.
    fn setup_encryption(&self) -> Result<Option<CryptContext>, Box<dyn Error>> {
        let encrypt_obj = match self.xref.trailer.get(b"Encrypt") {
            Some(obj) => obj,
            None => return Ok(None),
        };

        // Resolve the /Encrypt dictionary. crypt is None at this point,
        // so no decryption is applied (correct: /Encrypt itself is not encrypted).
        let encrypt_dict = self.resolve_value(encrypt_obj)?;
        let dict = encrypt_dict
            .as_dict()
            .ok_or("/Encrypt did not resolve to a dictionary")?;

        // Extract the first element of the trailer /ID array.
        let id_array = self
            .xref
            .trailer
            .get(b"ID")
            .and_then(|o| o.as_array())
            .ok_or("Encrypted PDF trailer missing /ID array")?;
        let file_id = id_array
            .first()
            .and_then(|o| o.as_string_bytes())
            .ok_or("Trailer /ID[0] is not a string")?;

        let encrypt_obj_num = encrypt_obj.as_reference().map(|r| r.num);

        let ctx = CryptContext::new(dict, file_id, encrypt_obj_num)?;
        Ok(Some(ctx))
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
        let obj_num = lexer
            .next_object()?
            .ok_or("Expected object number")?
            .as_i64()
            .ok_or("Object number is not an integer")? as u32;
        let gen_num = lexer
            .next_object()?
            .ok_or("Expected generation number")?
            .as_i64()
            .ok_or("Generation number is not an integer")? as u16;
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
        let value = if value.as_dict().is_some() {
            let saved_pos = lexer.position();
            lexer.skip_whitespace();

            let kw_pos = lexer.position();
            if !lexer.at_end() {
                let maybe_kw = lexer.read_keyword();
                if let Ok(kw) = maybe_kw
                    && kw == b"stream"
                {
                    return self.extract_stream(value, kw_pos, depth, obj_num, gen_num);
                }
            }
            lexer.set_position(saved_pos);
            value
        } else {
            value
        };

        // Decrypt string values if encryption is active and this is not the /Encrypt dict.
        match &self.crypt {
            Some(crypt) if !crypt.is_encrypt_dict(obj_num) => {
                Ok(decrypt_strings(crypt, value, obj_num, gen_num))
            }
            _ => Ok(value),
        }
    }

    /// Extract stream data from a dictionary object whose "stream" keyword starts
    /// at `kw_pos` in the file. Handles /Length, /Filter, /DecodeParms.
    ///
    /// Many real-world PDFs have incorrect `/Length` values (off-by-one from
    /// CRLF/LF confusion, wrong indirect references, etc.). When `/Length`-based
    /// extraction fails to decompress, we fall back to scanning for the
    /// `endstream` keyword to find the true data boundary.
    fn extract_stream(
        &self,
        dict_obj: PdfObject,
        kw_pos: usize,
        depth: usize,
        obj_num: u32,
        gen_num: u16,
    ) -> Result<PdfObject, Box<dyn Error>> {
        let dict = dict_obj
            .as_dict()
            .ok_or("Stream object is not a dictionary")?;

        let after_kw = kw_pos + b"stream".len();
        let data_start = skip_stream_eol(self.data, after_kw);

        let length_result = self.resolve_stream_length(dict, depth);
        let raw_data = match length_result {
            Ok(length) if data_start + length <= self.data.len() => {
                &self.data[data_start..data_start + length]
            }
            _ => scan_for_endstream(self.data, data_start)?,
        };

        // Decrypt the raw stream bytes before decompression.
        let decrypted;
        let effective_data = match &self.crypt {
            Some(crypt) if !crypt.is_encrypt_dict(obj_num) => {
                decrypted = crypt.decrypt(raw_data, obj_num, gen_num);
                &decrypted[..]
            }
            _ => raw_data,
        };

        let (filter_names, params_list) = self.collect_filters(dict, depth)?;
        let filter_refs: Vec<&[u8]> = filter_names.iter().map(|f| f.as_slice()).collect();
        let params_refs: Vec<Option<&PdfObject>> = params_list.iter().map(|p| p.as_ref()).collect();

        let decoded = if filter_refs.is_empty() {
            effective_data.to_vec()
        } else {
            match super::stream::decode_stream(effective_data, &filter_refs, &params_refs) {
                Ok(data) => data,
                Err(first_err) => {
                    // /Length may be subtly wrong. Retry with endstream-scanned boundaries,
                    // but only if the scanned data differs from what we already tried.
                    let scanned = scan_for_endstream(self.data, data_start)?;
                    if scanned.len() == raw_data.len() {
                        return Err(first_err);
                    }
                    let rescanned_decrypted;
                    let rescanned_effective = match &self.crypt {
                        Some(crypt) if !crypt.is_encrypt_dict(obj_num) => {
                            rescanned_decrypted = crypt.decrypt(scanned, obj_num, gen_num);
                            &rescanned_decrypted[..]
                        }
                        _ => scanned,
                    };
                    super::stream::decode_stream(rescanned_effective, &filter_refs, &params_refs)?
                }
            }
        };

        // Decrypt string values inside the stream dictionary.
        let final_dict = match &self.crypt {
            Some(crypt) if !crypt.is_encrypt_dict(obj_num) => dict
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        decrypt_strings(crypt, v.clone(), obj_num, gen_num),
                    )
                })
                .collect(),
            _ => dict.clone(),
        };

        Ok(PdfObject::Stream {
            dict: final_dict,
            data: decoded,
        })
    }

    /// Resolve the /Length value from a stream dictionary, handling indirect references.
    fn resolve_stream_length(
        &self,
        dict: &std::collections::HashMap<Vec<u8>, PdfObject>,
        depth: usize,
    ) -> Result<usize, Box<dyn Error>> {
        let length_obj = dict
            .get(b"Length".as_slice())
            .ok_or("Stream dictionary missing /Length")?;
        match length_obj.as_reference() {
            Some(r) => {
                let resolved = self.resolve_with_depth(r, depth + 1)?;
                safe_usize(
                    resolved
                        .as_i64()
                        .ok_or("Stream /Length reference did not resolve to integer")?,
                )
            }
            None => safe_usize(
                length_obj
                    .as_i64()
                    .ok_or("Stream /Length is not an integer")?,
            ),
        }
    }

    /// Collect /Filter and /DecodeParms from a stream dictionary.
    /// Returns (filter_name_bytes_list, params_list).
    #[allow(clippy::type_complexity)]
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

/// Recursively decrypt all String/HexString values in a PDF object tree.
/// Stream data is NOT decrypted here — that is handled in `extract_stream`
/// before decompression. This only decrypts metadata strings in stream dicts.
fn decrypt_strings(crypt: &CryptContext, obj: PdfObject, obj_num: u32, gen_num: u16) -> PdfObject {
    decrypt_strings_bounded(crypt, obj, obj_num, gen_num, 0)
}

fn decrypt_strings_bounded(
    crypt: &CryptContext,
    obj: PdfObject,
    obj_num: u32,
    gen_num: u16,
    depth: usize,
) -> PdfObject {
    const MAX_DEPTH: usize = 32;
    if depth >= MAX_DEPTH {
        return obj;
    }
    match obj {
        PdfObject::String(data) => PdfObject::String(crypt.decrypt(&data, obj_num, gen_num)),
        PdfObject::HexString(data) => PdfObject::HexString(crypt.decrypt(&data, obj_num, gen_num)),
        PdfObject::Array(items) => PdfObject::Array(
            items
                .into_iter()
                .map(|item| decrypt_strings_bounded(crypt, item, obj_num, gen_num, depth + 1))
                .collect(),
        ),
        PdfObject::Dictionary(d) => PdfObject::Dictionary(
            d.into_iter()
                .map(|(k, v)| {
                    (
                        k,
                        decrypt_strings_bounded(crypt, v, obj_num, gen_num, depth + 1),
                    )
                })
                .collect(),
        ),
        PdfObject::Stream { dict, data } => PdfObject::Stream {
            dict: dict
                .into_iter()
                .map(|(k, v)| {
                    (
                        k,
                        decrypt_strings_bounded(crypt, v, obj_num, gen_num, depth + 1),
                    )
                })
                .collect(),
            data,
        },
        other => other,
    }
}

/// Convert an i64 to usize, returning an error for negative or oversized values.
fn safe_usize(val: i64) -> Result<usize, Box<dyn Error>> {
    usize::try_from(val).map_err(|_| format!("Negative or oversized value: {val}").into())
}

/// Scan forward from `start` for the `endstream` keyword and return the
/// stream data slice, trimming any trailing whitespace before the marker.
///
/// Stream data is arbitrary binary, so we require the keyword to sit on PDF
/// token boundaries (preceded by whitespace and followed by whitespace or
/// end-of-data) to avoid false positives where the byte pattern occurs inside
/// the stream payload.
fn scan_for_endstream(data: &[u8], start: usize) -> Result<&[u8], Box<dyn Error>> {
    let marker = b"endstream";
    let search_end = data.len().saturating_sub(marker.len());
    let mut pos = start;
    while pos <= search_end {
        if &data[pos..pos + marker.len()] == marker {
            let preceded_by_ws = pos == 0 || data[pos - 1].is_ascii_whitespace();
            let after = pos + marker.len();
            let followed_by_ws = after >= data.len() || data[after].is_ascii_whitespace();
            if preceded_by_ws && followed_by_ws {
                let mut end = pos;
                while end > start && data[end - 1].is_ascii_whitespace() {
                    end -= 1;
                }
                return Ok(&data[start..end]);
            }
        }
        pos += 1;
    }
    Err("Could not find 'endstream' marker".into())
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
    fn open_non_encrypted_pdf_has_no_crypt() {
        let pdf = make_simple_pdf();
        let doc = PdfDocument::open(&pdf).unwrap();
        assert!(
            doc.crypt.is_none(),
            "Non-encrypted PDF should have no CryptContext"
        );
    }

    #[test]
    fn resolve_through_references() {
        let pdf = make_simple_pdf();
        let doc = PdfDocument::open(&pdf).unwrap();
        let page = doc.page_object(0).unwrap();
        assert_eq!(page.get_name_str(b"Type"), Some("Page"));
    }

    #[test]
    fn page_annotations_returns_empty_for_no_annots() {
        let pdf = make_simple_pdf();
        let doc = PdfDocument::open(&pdf).unwrap();
        let annots = doc.page_annotations(0).unwrap();
        assert!(annots.is_empty());
    }
}

//! PDF Standard Security Handler decryption (ISO 32000-1 §7.6).
//!
//! Supports V=1 (RC4-40) and V=2 (RC4-128) with revisions R=2 and R=3.
//! Decrypts PDFs protected with an empty user password.

use std::collections::HashMap;
use std::error::Error;

use super::objects::PdfObject;

/// Standard 32-byte padding string (ISO 32000-1 Table 3.18 / PDF Ref Table 3.19).
const PADDING: [u8; 32] = [
    0x28, 0xBF, 0x4E, 0x5E, 0x4E, 0x75, 0x8A, 0x41, 0x64, 0x00, 0x4E, 0x56, 0xFF, 0xFA, 0x01, 0x08,
    0x2E, 0x2E, 0x00, 0xB6, 0xD0, 0x68, 0x3E, 0x80, 0x2F, 0x0C, 0xA9, 0xFE, 0x64, 0x53, 0x69, 0x7A,
];

// ---------------------------------------------------------------------------
// RC4 stream cipher
// ---------------------------------------------------------------------------

/// RC4 key-scheduling and keystream generation.
struct Rc4 {
    s: [u8; 256],
    i: u8,
    j: u8,
}

impl Rc4 {
    fn new(key: &[u8]) -> Self {
        let mut s = [0u8; 256];
        for (i, slot) in s.iter_mut().enumerate() {
            *slot = i as u8;
        }
        let mut j: u8 = 0;
        for i in 0..256 {
            j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
            s.swap(i, j as usize);
        }
        Rc4 { s, i: 0, j: 0 }
    }

    fn apply(&mut self, data: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; data.len()];
        for (idx, &byte) in data.iter().enumerate() {
            self.i = self.i.wrapping_add(1);
            self.j = self.j.wrapping_add(self.s[self.i as usize]);
            self.s.swap(self.i as usize, self.j as usize);
            let k = self.s[self.s[self.i as usize].wrapping_add(self.s[self.j as usize]) as usize];
            out[idx] = byte ^ k;
        }
        out
    }
}

/// Encrypt/decrypt `data` using RC4 with the given `key`.
fn rc4_crypt(data: &[u8], key: &[u8]) -> Vec<u8> {
    Rc4::new(key).apply(data)
}

// ---------------------------------------------------------------------------
// PDF encryption key derivation (ISO 32000-1 Algorithm 2)
// ---------------------------------------------------------------------------

/// Derive the file encryption key from a password and /Encrypt parameters.
///
/// `password`: user password bytes (empty for empty-password PDFs).
/// `o`: 32-byte /O value from /Encrypt dictionary.
/// `p`: /P permissions value (signed 32-bit).
/// `file_id`: first element of the trailer /ID array.
/// `r`: security handler revision (2 or 3).
/// `key_len`: encryption key length in bytes (5 for 40-bit, 16 for 128-bit).
fn compute_encryption_key(
    password: &[u8],
    o: &[u8; 32],
    p: i32,
    file_id: &[u8],
    r: u32,
    key_len: usize,
) -> Vec<u8> {
    // Step 1: Pad/truncate the password to 32 bytes using the standard padding.
    let mut padded = [0u8; 32];
    let copy_len = password.len().min(32);
    padded[..copy_len].copy_from_slice(&password[..copy_len]);
    padded[copy_len..].copy_from_slice(&PADDING[..32 - copy_len]);

    // Steps 2-5: MD5 hash of padded password + /O + /P (LE) + file ID.
    let mut ctx = md5::Context::new();
    ctx.consume(padded);
    ctx.consume(o);
    ctx.consume((p as u32).to_le_bytes());
    ctx.consume(file_id);

    let mut hash: [u8; 16] = ctx.compute().into();

    // Step 8 (R >= 3): 50 additional MD5 rounds on the first key_len bytes.
    if r >= 3 {
        for _ in 0..50 {
            hash = md5::compute(&hash[..key_len]).into();
        }
    }

    // Step 9: Truncate to key_len bytes.
    hash[..key_len].to_vec()
}

// ---------------------------------------------------------------------------
// User password validation (ISO 32000-1 Algorithms 4-6)
// ---------------------------------------------------------------------------

/// Validate that the derived encryption key matches the stored /U value.
///
/// For R=2: RC4-encrypt the padding, compare all 32 bytes with /U.
/// For R=3: MD5(padding + file_id), 20 rounds of RC4 with XOR'd keys,
///          compare first 16 bytes with /U.
fn authenticate_user_password(key: &[u8], u: &[u8; 32], file_id: &[u8], r: u32) -> bool {
    if r == 2 {
        let computed = rc4_crypt(&PADDING, key);
        return computed[..] == u[..];
    }

    // R >= 3 (Algorithm 5 then compare per Algorithm 6)
    let mut ctx = md5::Context::new();
    ctx.consume(PADDING);
    ctx.consume(file_id);
    let hash: [u8; 16] = ctx.compute().into();

    let mut result = rc4_crypt(&hash, key);

    for i in 1u8..=19 {
        let modified_key: Vec<u8> = key.iter().map(|&b| b ^ i).collect();
        result = rc4_crypt(&result, &modified_key);
    }

    result[..16] == u[..16]
}

// ---------------------------------------------------------------------------
// Per-object key derivation (ISO 32000-1 Algorithm 1)
// ---------------------------------------------------------------------------

/// Derive the per-object decryption key from the file key and object/generation numbers.
///
/// Appends the 3 low-order bytes of obj_num (LE) and 2 low-order bytes of gen_num (LE)
/// to the file key, then MD5-hashes and truncates to min(key_len + 5, 16) bytes.
fn object_key(file_key: &[u8], obj_num: u32, gen_num: u16) -> Vec<u8> {
    let n = file_key.len();
    let mut input = Vec::with_capacity(n + 5);
    input.extend_from_slice(file_key);
    input.push((obj_num & 0xFF) as u8);
    input.push(((obj_num >> 8) & 0xFF) as u8);
    input.push(((obj_num >> 16) & 0xFF) as u8);
    input.push((gen_num & 0xFF) as u8);
    input.push(((gen_num >> 8) & 0xFF) as u8);

    let hash: [u8; 16] = md5::compute(&input).into();
    let key_len = (n + 5).min(16);
    hash[..key_len].to_vec()
}

// ---------------------------------------------------------------------------
// CryptContext — encryption state for a PDF document
// ---------------------------------------------------------------------------

/// Decryption state for an encrypted PDF document.
#[derive(Debug)]
pub(super) struct CryptContext {
    file_key: Vec<u8>,
    encrypt_obj_num: Option<u32>,
}

impl CryptContext {
    /// Create a new CryptContext from the /Encrypt dictionary and trailer /ID.
    ///
    /// Attempts decryption with the empty password. Returns an error if:
    /// - The encryption version/revision is unsupported (V must be 1 or 2, R must be 2 or 3)
    /// - The empty password fails validation against /U
    pub(super) fn new(
        encrypt_dict: &HashMap<Vec<u8>, PdfObject>,
        file_id: &[u8],
        encrypt_obj_num: Option<u32>,
    ) -> Result<Self, Box<dyn Error>> {
        let v = encrypt_dict
            .get(b"V".as_slice())
            .and_then(|o| o.as_i64())
            .unwrap_or(0);
        let r = encrypt_dict
            .get(b"R".as_slice())
            .and_then(|o| o.as_i64())
            .unwrap_or(0) as u32;
        let length = encrypt_dict
            .get(b"Length".as_slice())
            .and_then(|o| o.as_i64())
            .unwrap_or(40) as usize;

        if v != 1 && v != 2 {
            return Err(format!(
                "Unsupported PDF encryption version V={v} (only V=1 and V=2 are supported)"
            )
            .into());
        }

        // V=1 is fixed RC4-40 (5-byte key) per PDF Reference 1.7 §3.5;
        // /Length is only meaningful for V>=2.
        let key_len = match v {
            1 => 5,
            _ => (length / 8).clamp(5, 16),
        };
        if r != 2 && r != 3 {
            return Err(format!(
                "Unsupported PDF security handler revision R={r} (only R=2 and R=3 are supported)"
            )
            .into());
        }

        let o = extract_32_bytes(encrypt_dict, b"O")?;
        let u = extract_32_bytes(encrypt_dict, b"U")?;
        let p = encrypt_dict
            .get(b"P".as_slice())
            .and_then(|o| o.as_i64())
            .unwrap_or(0) as i32;

        let file_key = compute_encryption_key(&[], &o, p, file_id, r, key_len);

        if !authenticate_user_password(&file_key, &u, file_id, r) {
            return Err("PDF is encrypted and could not be decrypted with empty password".into());
        }

        Ok(CryptContext {
            file_key,
            encrypt_obj_num,
        })
    }

    /// Decrypt a byte slice using the per-object RC4 key.
    pub(super) fn decrypt(&self, data: &[u8], obj_num: u32, gen_num: u16) -> Vec<u8> {
        let key = object_key(&self.file_key, obj_num, gen_num);
        rc4_crypt(data, &key)
    }

    /// Returns true if `obj_num` is the /Encrypt dictionary object (must not be decrypted).
    pub(super) fn is_encrypt_dict(&self, obj_num: u32) -> bool {
        self.encrypt_obj_num == Some(obj_num)
    }
}

/// Extract a 32-byte value from a dictionary entry (String or HexString).
fn extract_32_bytes(
    dict: &HashMap<Vec<u8>, PdfObject>,
    key: &[u8],
) -> Result<[u8; 32], Box<dyn Error>> {
    let obj = dict
        .get(key)
        .ok_or_else(|| format!("/Encrypt missing /{}", String::from_utf8_lossy(key)))?;
    let bytes = obj
        .as_string_bytes()
        .ok_or_else(|| format!("/{} is not a string", String::from_utf8_lossy(key)))?;
    if bytes.len() < 32 {
        return Err(format!(
            "/{} too short ({} bytes, need 32)",
            String::from_utf8_lossy(key),
            bytes.len()
        )
        .into());
    }
    let mut result = [0u8; 32];
    result.copy_from_slice(&bytes[..32]);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc4_known_vector_key_wiki() {
        // RC4("Wiki", "pedia") => 0x1021BF0420
        let ciphertext = rc4_crypt(b"pedia", b"Wiki");
        assert_eq!(ciphertext, vec![0x10, 0x21, 0xBF, 0x04, 0x20]);
    }

    #[test]
    fn rc4_known_vector_key_secret() {
        // RC4("Secret", "Attack at dawn") => 0x45A01F645FC35B383552544B9BF5
        let ciphertext = rc4_crypt(b"Attack at dawn", b"Secret");
        assert_eq!(
            ciphertext,
            vec![
                0x45, 0xA0, 0x1F, 0x64, 0x5F, 0xC3, 0x5B, 0x38, 0x35, 0x52, 0x54, 0x4B, 0x9B, 0xF5
            ]
        );
    }

    #[test]
    fn rc4_roundtrip() {
        let key = b"test-key";
        let plaintext = b"Hello, PDF encryption!";
        let ciphertext = rc4_crypt(plaintext, key);
        let decrypted = rc4_crypt(&ciphertext, key);
        assert_eq!(decrypted, plaintext);
    }

    fn hex_decode(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// Values from lease.pdf /Encrypt dictionary and trailer /ID.
    fn lease_pdf_params() -> ([u8; 32], [u8; 32], i32, Vec<u8>) {
        let o = hex_decode("0FF7BB53E8B3E3927EB307F43C73A8448D7152BA4563CDCE92922C1ED88907C4")
            .try_into()
            .unwrap();
        let u = hex_decode("3F3D984D6373D864CEFF9A316915006F28BF4E5E4E758A4164004E56FFFA0108")
            .try_into()
            .unwrap();
        let id = hex_decode("761F8818ADAADB0DF1A344A640333BE5");
        (o, u, -60, id)
    }

    fn make_lease_encrypt_dict() -> (HashMap<Vec<u8>, PdfObject>, Vec<u8>) {
        let (o, u, p, id) = lease_pdf_params();
        let mut dict = HashMap::new();
        dict.insert(b"V".to_vec(), PdfObject::Integer(2));
        dict.insert(b"R".to_vec(), PdfObject::Integer(3));
        dict.insert(b"Length".to_vec(), PdfObject::Integer(128));
        dict.insert(b"P".to_vec(), PdfObject::Integer(p as i64));
        dict.insert(b"O".to_vec(), PdfObject::HexString(o.to_vec()));
        dict.insert(b"U".to_vec(), PdfObject::HexString(u.to_vec()));
        (dict, id)
    }

    #[test]
    fn validate_empty_password_lease_pdf() {
        let (o, u, p, id) = lease_pdf_params();
        let key = compute_encryption_key(&[], &o, p, &id, 3, 16);
        assert_eq!(key.len(), 16, "Key should be 16 bytes for 128-bit RC4");
        assert!(
            authenticate_user_password(&key, &u, &id, 3),
            "Empty password should validate against lease.pdf /U value"
        );
    }

    #[test]
    fn wrong_password_does_not_validate() {
        let (o, u, p, id) = lease_pdf_params();
        let key = compute_encryption_key(b"wrong-password", &o, p, &id, 3, 16);
        assert!(
            !authenticate_user_password(&key, &u, &id, 3),
            "Wrong password should NOT validate"
        );
    }

    #[test]
    fn object_key_length_capped_at_16() {
        let file_key = vec![0xAB; 16];
        let key = object_key(&file_key, 1, 0);
        assert_eq!(key.len(), 16);
    }

    #[test]
    fn object_key_40bit() {
        let file_key = vec![0xCD; 5];
        let key = object_key(&file_key, 10, 0);
        assert_eq!(key.len(), 10);
    }

    #[test]
    fn object_key_different_per_object() {
        let file_key = vec![0x42; 16];
        let key1 = object_key(&file_key, 1, 0);
        let key2 = object_key(&file_key, 2, 0);
        assert_ne!(key1, key2, "Different objects must have different keys");
    }

    #[test]
    fn crypt_context_from_lease_pdf_values() {
        let (dict, file_id) = make_lease_encrypt_dict();
        let ctx = CryptContext::new(&dict, &file_id, Some(1479)).unwrap();
        assert_eq!(ctx.file_key.len(), 16);
        assert!(ctx.is_encrypt_dict(1479));
        assert!(!ctx.is_encrypt_dict(1));
    }

    #[test]
    fn crypt_context_unsupported_version() {
        let mut dict = HashMap::new();
        dict.insert(b"V".to_vec(), PdfObject::Integer(4));
        dict.insert(b"R".to_vec(), PdfObject::Integer(4));
        dict.insert(b"Length".to_vec(), PdfObject::Integer(128));
        dict.insert(b"P".to_vec(), PdfObject::Integer(0));
        dict.insert(b"O".to_vec(), PdfObject::HexString(vec![0; 32]));
        dict.insert(b"U".to_vec(), PdfObject::HexString(vec![0; 32]));

        let result = CryptContext::new(&dict, &[0; 16], None);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("V=4"), "Error should mention version: {err}");
    }

    #[test]
    fn decrypt_roundtrip_with_object_key() {
        let (dict, file_id) = make_lease_encrypt_dict();
        let ctx = CryptContext::new(&dict, &file_id, Some(1479)).unwrap();

        let plaintext = b"Hello, encrypted PDF!";
        let encrypted = ctx.decrypt(plaintext, 42, 0);
        let decrypted = ctx.decrypt(&encrypted, 42, 0);
        assert_eq!(&decrypted, plaintext);
    }
}

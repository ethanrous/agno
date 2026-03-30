use std::collections::HashMap;
use std::fmt;

/// A reference to an indirect PDF object by object number and generation number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObjRef {
    pub num: u32,
    pub gen_num: u16,
}

/// A PDF object as defined in ISO 32000-1 section 7.3.
#[derive(Debug, Clone, PartialEq)]
pub enum PdfObject {
    Null,
    Bool(bool),
    Integer(i64),
    Real(f64),
    Name(Vec<u8>),
    String(Vec<u8>),
    HexString(Vec<u8>),
    Array(Vec<PdfObject>),
    Dictionary(HashMap<Vec<u8>, PdfObject>),
    Stream {
        dict: HashMap<Vec<u8>, PdfObject>,
        data: Vec<u8>,
    },
    Reference(ObjRef),
}

impl PdfObject {
    /// Returns the integer value if this is an Integer.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            PdfObject::Integer(n) => Some(*n),
            PdfObject::Real(f) => Some(*f as i64),
            _ => None,
        }
    }

    /// Returns the float value if this is a Real or Integer.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            PdfObject::Real(f) => Some(*f),
            PdfObject::Integer(n) => Some(*n as f64),
            _ => None,
        }
    }

    /// Returns the raw name bytes if this is a Name.
    pub fn as_name(&self) -> Option<&[u8]> {
        match self {
            PdfObject::Name(n) => Some(n),
            _ => None,
        }
    }

    /// Returns the name as a &str if this is a Name and the bytes are valid UTF-8.
    pub fn as_name_str(&self) -> Option<&str> {
        self.as_name().and_then(|b| std::str::from_utf8(b).ok())
    }

    /// Returns the boolean value if this is a Bool.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            PdfObject::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Returns the array slice if this is an Array.
    pub fn as_array(&self) -> Option<&[PdfObject]> {
        match self {
            PdfObject::Array(a) => Some(a),
            _ => None,
        }
    }

    /// Returns the dictionary map if this is a Dictionary or Stream.
    pub fn as_dict(&self) -> Option<&HashMap<Vec<u8>, PdfObject>> {
        match self {
            PdfObject::Dictionary(d) => Some(d),
            PdfObject::Stream { dict, .. } => Some(dict),
            _ => None,
        }
    }

    /// Returns the raw string bytes if this is a String or HexString.
    pub fn as_string_bytes(&self) -> Option<&[u8]> {
        match self {
            PdfObject::String(s) | PdfObject::HexString(s) => Some(s),
            _ => None,
        }
    }

    /// Returns the ObjRef if this is a Reference.
    pub fn as_reference(&self) -> Option<ObjRef> {
        match self {
            PdfObject::Reference(r) => Some(*r),
            _ => None,
        }
    }

    /// Returns the (dict, data) pair if this is a Stream.
    #[allow(clippy::type_complexity)]
    pub fn as_stream(&self) -> Option<(&HashMap<Vec<u8>, PdfObject>, &[u8])> {
        match self {
            PdfObject::Stream { dict, data } => Some((dict, data)),
            _ => None,
        }
    }

    /// Look up a key in the dictionary (works on Dictionary and Stream).
    pub fn get(&self, key: &[u8]) -> Option<&PdfObject> {
        self.as_dict()?.get(key)
    }

    /// Look up a key and return its integer value.
    pub fn get_i64(&self, key: &[u8]) -> Option<i64> {
        self.get(key)?.as_i64()
    }

    /// Look up a key and return its float value.
    pub fn get_f64(&self, key: &[u8]) -> Option<f64> {
        self.get(key)?.as_f64()
    }

    /// Look up a key and return its name as a &str.
    pub fn get_name_str(&self, key: &[u8]) -> Option<&str> {
        self.get(key)?.as_name_str()
    }
}

impl fmt::Display for PdfObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PdfObject::Null => write!(f, "null"),
            PdfObject::Bool(b) => write!(f, "{b}"),
            PdfObject::Integer(n) => write!(f, "{n}"),
            PdfObject::Real(r) => write!(f, "{r}"),
            PdfObject::Name(n) => {
                write!(f, "/")?;
                f.write_str(std::str::from_utf8(n).unwrap_or("<invalid>"))
            }
            PdfObject::String(s) => {
                write!(f, "(")?;
                for &b in s {
                    if b == b'(' || b == b')' || b == b'\\' {
                        write!(f, "\\{}", b as char)?;
                    } else if b.is_ascii_graphic() || b == b' ' {
                        write!(f, "{}", b as char)?;
                    } else {
                        write!(f, "\\{b:03o}")?;
                    }
                }
                write!(f, ")")
            }
            PdfObject::HexString(s) => {
                write!(f, "<")?;
                for b in s {
                    write!(f, "{b:02X}")?;
                }
                write!(f, ">")
            }
            PdfObject::Array(a) => {
                write!(f, "[")?;
                for (i, obj) in a.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "{obj}")?;
                }
                write!(f, "]")
            }
            PdfObject::Dictionary(d) => {
                write!(f, "<<")?;
                for (k, v) in d {
                    write!(f, "/{} {v} ", std::str::from_utf8(k).unwrap_or("<invalid>"))?;
                }
                write!(f, ">>")
            }
            PdfObject::Stream { dict, data } => {
                write!(f, "<<")?;
                for (k, v) in dict {
                    write!(f, "/{} {v} ", std::str::from_utf8(k).unwrap_or("<invalid>"))?;
                }
                write!(f, ">> stream\n<{} bytes>\nendstream", data.len())
            }
            PdfObject::Reference(r) => write!(f, "{} {} R", r.num, r.gen_num),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_as_f64() {
        let obj = PdfObject::Integer(42);
        assert_eq!(obj.as_f64(), Some(42.0));
        assert_eq!(obj.as_i64(), Some(42));
    }

    #[test]
    fn real_as_i64_truncates() {
        let obj = PdfObject::Real(3.7);
        assert_eq!(obj.as_i64(), Some(3));
    }

    #[test]
    fn dictionary_get() {
        let mut d = HashMap::new();
        d.insert(b"Type".to_vec(), PdfObject::Name(b"Page".to_vec()));
        d.insert(b"Count".to_vec(), PdfObject::Integer(5));
        let obj = PdfObject::Dictionary(d);
        assert_eq!(obj.get_name_str(b"Type"), Some("Page"));
        assert_eq!(obj.get_i64(b"Count"), Some(5));
        assert!(obj.get(b"Missing").is_none());
    }

    #[test]
    fn display_formatting() {
        assert_eq!(format!("{}", PdfObject::Integer(42)), "42");
        assert_eq!(format!("{}", PdfObject::Name(b"Type".to_vec())), "/Type");
        assert_eq!(format!("{}", PdfObject::Bool(true)), "true");
        assert_eq!(format!("{}", PdfObject::Null), "null");
    }
}

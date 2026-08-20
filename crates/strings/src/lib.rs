//! Dual Latin1/UTF-16 JavaScript string representation.
//!
//! JavaScript strings are sequences of 16-bit code units, but most real-world
//! strings are ASCII/Latin1. This crate stores strings in a dual representation
//! — Latin1 (1 byte per code unit) when all code units fit in 0x00..=0xFF,
//! and UTF-16 (2 bytes per code unit) otherwise — saving ~50% memory for the
//! common case.

use std::fmt;
use std::hash::{Hash, Hasher};

// ---------------------------------------------------------------------------
// Hashing helper — FNV-1a over logical 16-bit code units
// ---------------------------------------------------------------------------

const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x00000100000001B3;

fn fnv1a_hash_code_units(iter: impl Iterator<Item = u16>) -> u64 {
    let mut h = FNV_OFFSET;
    for unit in iter {
        let lo = (unit & 0xFF) as u8;
        let hi = (unit >> 8) as u8;
        h ^= lo as u64;
        h = h.wrapping_mul(FNV_PRIME);
        h ^= hi as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

fn compute_hash(data: &JsStringData) -> u64 {
    match data {
        JsStringData::Latin1(bytes) => fnv1a_hash_code_units(bytes.iter().map(|&b| b as u16)),
        JsStringData::Utf16(units) => fnv1a_hash_code_units(units.iter().copied()),
    }
}

// ---------------------------------------------------------------------------
// JsStringData
// ---------------------------------------------------------------------------

/// Internal string data — either Latin1 (1 byte/char) or UTF-16 (2 bytes/char).
#[derive(Debug, Clone)]
pub enum JsStringData {
    /// Latin1 encoding: one byte per code unit (values 0x00..=0xFF).
    Latin1(Box<[u8]>),
    /// UTF-16 encoding: two bytes per code unit.
    Utf16(Box<[u16]>),
}

// ---------------------------------------------------------------------------
// JsString
// ---------------------------------------------------------------------------

/// A JavaScript string with cached hash and length.
///
/// Length is measured in 16-bit code units (as per ECMAScript), NOT Unicode
/// codepoints and NOT bytes.
#[derive(Debug, Clone)]
pub struct JsString {
    data: JsStringData,
    /// Length in 16-bit code units.
    len: u32,
    /// Cached FNV-1a hash computed over logical 16-bit code units.
    hash: u64,
}

// -- Constructors -----------------------------------------------------------

impl JsString {
    /// Creates an empty string (Latin1 representation).
    pub fn empty() -> Self {
        let data = JsStringData::Latin1(Box::new([]));
        Self {
            hash: compute_hash(&data),
            data,
            len: 0,
        }
    }

    /// Creates a string from Latin1 bytes. Each byte maps to a UTF-16 code
    /// unit with the same value (0x00..=0xFF).
    pub fn from_latin1(bytes: &[u8]) -> Self {
        let data = JsStringData::Latin1(bytes.into());
        Self {
            len: bytes.len() as u32,
            hash: compute_hash(&data),
            data,
        }
    }

    /// Creates a string from UTF-16 code units. Unpaired surrogates are
    /// accepted — JavaScript allows them.
    pub fn from_utf16(units: &[u16]) -> Self {
        let data = JsStringData::Utf16(units.into());
        Self {
            len: units.len() as u32,
            hash: compute_hash(&data),
            data,
        }
    }

    /// Creates a string from a Rust `&str`. If every character fits in Latin1
    /// (U+0000..=U+00FF), the string is stored as Latin1. Otherwise it is
    /// converted to UTF-16.
    pub fn from_rust_str(s: &str) -> Self {
        let is_latin1 = s.chars().all(|c| (c as u32) <= 0xFF);

        if is_latin1 {
            let bytes: Vec<u8> = s.chars().map(|c| c as u8).collect();
            Self::from_latin1(&bytes)
        } else {
            let units: Vec<u16> = s.encode_utf16().collect();
            Self::from_utf16(&units)
        }
    }

    /// Creates a string by copying static Latin1 bytes.
    pub fn from_static_latin1(s: &'static [u8]) -> Self {
        Self::from_latin1(s)
    }

    // -- Core operations ----------------------------------------------------

    /// Returns the length in 16-bit code units (ECMAScript `.length`).
    pub fn length(&self) -> u32 {
        self.len
    }

    /// Returns `true` if this string is empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns `true` if the internal representation is Latin1.
    pub fn is_latin1(&self) -> bool {
        matches!(self.data, JsStringData::Latin1(_))
    }

    /// Returns the 16-bit code unit at the given index, or `None` if out of
    /// bounds.
    pub fn char_code_at(&self, index: u32) -> Option<u16> {
        if index >= self.len {
            return None;
        }
        let i = index as usize;
        match &self.data {
            JsStringData::Latin1(bytes) => Some(bytes[i] as u16),
            JsStringData::Utf16(units) => Some(units[i]),
        }
    }

    /// Returns the full Unicode code point at the given code-unit index.
    ///
    /// If the code unit at `index` is a high surrogate (0xD800..=0xDBFF) and
    /// the next code unit is a low surrogate (0xDC00..=0xDFFF), the two are
    /// decoded into a supplementary code point. Otherwise the single code unit
    /// value is returned.
    pub fn code_point_at(&self, index: u32) -> Option<u32> {
        let hi = self.char_code_at(index)? as u32;
        if (0xD800..=0xDBFF).contains(&hi)
            && let Some(lo) = self.char_code_at(index + 1)
        {
            let lo = lo as u32;
            if (0xDC00..=0xDFFF).contains(&lo) {
                return Some((hi - 0xD800) * 0x400 + (lo - 0xDC00) + 0x10000);
            }
        }
        Some(hi)
    }

    /// Concatenates two strings. If either operand is UTF-16, the result is
    /// UTF-16. Otherwise it is Latin1.
    pub fn concat(&self, other: &JsString) -> JsString {
        let new_len = self.len as usize + other.len as usize;
        match (&self.data, &other.data) {
            (JsStringData::Latin1(a), JsStringData::Latin1(b)) => {
                let mut buf = Vec::with_capacity(new_len);
                buf.extend_from_slice(a);
                buf.extend_from_slice(b);
                JsString::from_latin1(&buf)
            }
            _ => {
                let mut buf = Vec::with_capacity(new_len);
                buf.extend(self.code_units());
                buf.extend(other.code_units());
                JsString::from_utf16(&buf)
            }
        }
    }

    /// Returns a substring by code-unit indices `[start..end)`.
    ///
    /// If `start >= end` or `start >= len`, returns the empty string. `end` is
    /// clamped to `len`.
    pub fn slice(&self, start: u32, end: u32) -> JsString {
        let end = end.min(self.len);
        if start >= end {
            return JsString::empty();
        }
        let s = start as usize;
        let e = end as usize;
        match &self.data {
            JsStringData::Latin1(bytes) => JsString::from_latin1(&bytes[s..e]),
            JsStringData::Utf16(units) => JsString::from_utf16(&units[s..e]),
        }
    }

    /// Searches for `needle` starting at code-unit index `from`. Returns the
    /// first index where `needle` matches, or `None`.
    pub fn index_of(&self, needle: &JsString, from: u32) -> Option<u32> {
        if needle.len == 0 {
            return if from <= self.len { Some(from) } else { None };
        }
        if needle.len > self.len {
            return None;
        }
        let last_start = self.len - needle.len;
        let from = from.min(self.len);
        if from > last_start {
            return None;
        }

        // Brute-force code-unit comparison.
        'outer: for i in from..=last_start {
            for j in 0..needle.len {
                if self.char_code_at(i + j) != needle.char_code_at(j) {
                    continue 'outer;
                }
            }
            return Some(i);
        }
        None
    }

    // -- Conversions --------------------------------------------------------

    /// Converts the string to a Rust `String` (UTF-8).
    ///
    /// Unpaired surrogates are replaced with U+FFFD (REPLACEMENT CHARACTER).
    pub fn to_utf8(&self) -> String {
        match &self.data {
            JsStringData::Latin1(bytes) => bytes.iter().map(|&b| b as char).collect(),
            JsStringData::Utf16(units) => String::from_utf16_lossy(units),
        }
    }

    /// Returns the string as a `Vec<u16>` of UTF-16 code units.
    pub fn to_utf16_vec(&self) -> Vec<u16> {
        match &self.data {
            JsStringData::Latin1(bytes) => bytes.iter().map(|&b| b as u16).collect(),
            JsStringData::Utf16(units) => units.to_vec(),
        }
    }

    /// If the internal representation is Latin1, returns a reference to the
    /// bytes. Otherwise returns `None`.
    pub fn as_latin1(&self) -> Option<&[u8]> {
        match &self.data {
            JsStringData::Latin1(bytes) => Some(bytes),
            JsStringData::Utf16(_) => None,
        }
    }

    /// If the internal representation is UTF-16, returns a reference to the
    /// code units. Otherwise returns `None`.
    pub fn as_utf16(&self) -> Option<&[u16]> {
        match &self.data {
            JsStringData::Latin1(_) => None,
            JsStringData::Utf16(units) => Some(units),
        }
    }

    // -- UTF-16 ↔ byte index mapping ----------------------------------------

    /// Returns the length in UTF-16 code units.
    ///
    /// This is identical to [`length`](Self::length) — provided for explicit
    /// naming when you need to emphasize the UTF-16 semantics (e.g., when
    /// interfacing with APIs that use byte indices).
    pub fn utf16_len(&self) -> usize {
        self.len as usize
    }

    /// Convert a UTF-8 byte index within the equivalent Rust `&str` to a
    /// UTF-16 code unit index.
    ///
    /// Given the same source string that was used to construct this `JsString`
    /// (via [`from_rust_str`](Self::from_rust_str)), converts a byte offset
    /// into the UTF-8 encoding to the corresponding UTF-16 code-unit index.
    ///
    /// Returns `None` if `byte_idx` exceeds the UTF-8 byte length of the
    /// string or falls in the middle of a multi-byte UTF-8 sequence.
    ///
    /// # Example
    ///
    /// ```
    /// # use strings::JsString;
    /// let s = JsString::from_rust_str("hi\u{1F600}"); // "hi" + grinning face
    /// // In UTF-8: b'h'=0, b'i'=1, U+1F600 = bytes 2..6 (4 bytes)
    /// assert_eq!(s.byte_index_to_utf16(0), Some(0)); // 'h'
    /// assert_eq!(s.byte_index_to_utf16(1), Some(1)); // 'i'
    /// assert_eq!(s.byte_index_to_utf16(2), Some(2)); // start of emoji
    /// assert_eq!(s.byte_index_to_utf16(6), Some(4)); // past end of emoji
    /// assert_eq!(s.byte_index_to_utf16(3), None);    // mid-sequence
    /// ```
    pub fn byte_index_to_utf16(&self, byte_idx: usize) -> Option<usize> {
        // Reconstruct the UTF-8 form and walk both indices in parallel.
        let utf8 = self.to_utf8();
        if byte_idx > utf8.len() {
            return None;
        }
        if byte_idx == utf8.len() {
            return Some(self.len as usize);
        }
        // Reject byte indices that fall mid-sequence.
        if !utf8.is_char_boundary(byte_idx) {
            return None;
        }

        let mut utf16_idx: usize = 0;
        for (bi, ch) in utf8.char_indices() {
            if bi == byte_idx {
                return Some(utf16_idx);
            }
            utf16_idx += ch.len_utf16();
        }
        // Should only reach here if byte_idx == utf8.len() (handled above).
        None
    }

    /// Convert a UTF-16 code-unit index to the corresponding byte offset
    /// within the equivalent Rust `&str` (UTF-8 encoding).
    ///
    /// Returns `None` if `utf16_idx` exceeds [`utf16_len`](Self::utf16_len)
    /// or points to the second half of a surrogate pair (i.e., between the
    /// high and low surrogates of a supplementary character).
    ///
    /// # Example
    ///
    /// ```
    /// # use strings::JsString;
    /// let s = JsString::from_rust_str("hi\u{1F600}");
    /// assert_eq!(s.utf16_to_byte_index(0), Some(0)); // 'h'
    /// assert_eq!(s.utf16_to_byte_index(1), Some(1)); // 'i'
    /// assert_eq!(s.utf16_to_byte_index(2), Some(2)); // start of emoji
    /// assert_eq!(s.utf16_to_byte_index(3), None);    // low surrogate (mid-pair)
    /// assert_eq!(s.utf16_to_byte_index(4), Some(6)); // past end of emoji
    /// ```
    pub fn utf16_to_byte_index(&self, utf16_idx: usize) -> Option<usize> {
        if utf16_idx > self.len as usize {
            return None;
        }
        if utf16_idx == self.len as usize {
            return Some(self.to_utf8().len());
        }

        let utf8 = self.to_utf8();
        let mut cu_pos: usize = 0;
        for (bi, ch) in utf8.char_indices() {
            if cu_pos == utf16_idx {
                return Some(bi);
            }
            let cu_len = ch.len_utf16();
            // If utf16_idx falls inside a surrogate pair (cu_len == 2 and
            // utf16_idx == cu_pos + 1), it's pointing at the low surrogate.
            if cu_len == 2 && utf16_idx == cu_pos + 1 {
                return None;
            }
            cu_pos += cu_len;
        }
        // cu_pos == utf16_idx == self.len handled above.
        None
    }

    // -- Private helpers ----------------------------------------------------

    /// Iterator over 16-bit code units regardless of internal encoding.
    fn code_units(&self) -> CodeUnitIter<'_> {
        CodeUnitIter {
            data: &self.data,
            pos: 0,
            len: self.len as usize,
        }
    }
}

// ---------------------------------------------------------------------------
// Code unit iterator (private)
// ---------------------------------------------------------------------------

struct CodeUnitIter<'a> {
    data: &'a JsStringData,
    pos: usize,
    len: usize,
}

impl Iterator for CodeUnitIter<'_> {
    type Item = u16;

    fn next(&mut self) -> Option<u16> {
        if self.pos >= self.len {
            return None;
        }
        let val = match self.data {
            JsStringData::Latin1(bytes) => bytes[self.pos] as u16,
            JsStringData::Utf16(units) => units[self.pos],
        };
        self.pos += 1;
        Some(val)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.len - self.pos;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for CodeUnitIter<'_> {}

// ---------------------------------------------------------------------------
// Trait implementations
// ---------------------------------------------------------------------------

impl PartialEq for JsString {
    fn eq(&self, other: &Self) -> bool {
        if self.len != other.len || self.hash != other.hash {
            return false;
        }
        // Code-unit-by-code-unit comparison across encodings.
        match (&self.data, &other.data) {
            (JsStringData::Latin1(a), JsStringData::Latin1(b)) => a == b,
            (JsStringData::Utf16(a), JsStringData::Utf16(b)) => a == b,
            _ => self.code_units().eq(other.code_units()),
        }
    }
}

impl Eq for JsString {}

impl Hash for JsString {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.hash.hash(state);
    }
}

impl fmt::Display for JsString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_utf8())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::hash_map::DefaultHasher;

    fn hash_of(s: &JsString) -> u64 {
        let mut h = DefaultHasher::new();
        s.hash(&mut h);
        h.finish()
    }

    // -- Empty string -------------------------------------------------------

    #[test]
    fn empty_string() {
        let s = JsString::empty();
        assert_eq!(s.length(), 0);
        assert!(s.is_empty());
        assert!(s.is_latin1());
    }

    // -- from_str auto-detect -----------------------------------------------

    #[test]
    fn from_str_ascii_uses_latin1() {
        let s = JsString::from_rust_str("hello");
        assert!(s.is_latin1());
        assert_eq!(s.length(), 5);
        assert_eq!(s.to_utf8(), "hello");
    }

    #[test]
    fn from_str_latin1_extended() {
        // U+00E9 = e-acute, fits in Latin1
        let s = JsString::from_rust_str("\u{00E9}");
        assert!(s.is_latin1());
        assert_eq!(s.length(), 1);
        assert_eq!(s.char_code_at(0), Some(0xE9));
    }

    #[test]
    fn from_str_with_emoji_uses_utf16() {
        let s = JsString::from_rust_str("hi\u{1F600}");
        assert!(!s.is_latin1());
        // "hi" = 2 code units, U+1F600 = surrogate pair = 2 code units
        assert_eq!(s.length(), 4);
    }

    // -- from_latin1 round-trip ---------------------------------------------

    #[test]
    fn from_latin1_round_trip() {
        let bytes = b"test\xFF\x00";
        let s = JsString::from_latin1(bytes);
        assert!(s.is_latin1());
        assert_eq!(s.as_latin1(), Some(bytes.as_slice()));
        assert_eq!(s.length(), 6);
    }

    // -- from_utf16 round-trip ----------------------------------------------

    #[test]
    fn from_utf16_round_trip() {
        let units: Vec<u16> = vec![0x0048, 0x0065, 0x006C, 0x006C, 0x006F];
        let s = JsString::from_utf16(&units);
        assert!(!s.is_latin1());
        assert_eq!(s.as_utf16(), Some(units.as_slice()));
        assert_eq!(s.length(), 5);
    }

    // -- char_code_at -------------------------------------------------------

    #[test]
    fn char_code_at_in_bounds() {
        let s = JsString::from_rust_str("abc");
        assert_eq!(s.char_code_at(0), Some(0x61));
        assert_eq!(s.char_code_at(1), Some(0x62));
        assert_eq!(s.char_code_at(2), Some(0x63));
    }

    #[test]
    fn char_code_at_out_of_bounds() {
        let s = JsString::from_rust_str("ab");
        assert_eq!(s.char_code_at(2), None);
        assert_eq!(s.char_code_at(100), None);
    }

    // -- code_point_at ------------------------------------------------------

    #[test]
    fn code_point_at_basic_plane() {
        let s = JsString::from_rust_str("A");
        assert_eq!(s.code_point_at(0), Some(0x41));
    }

    #[test]
    fn code_point_at_supplementary_plane() {
        // U+1F600 = grinning face
        let s = JsString::from_rust_str("\u{1F600}");
        assert_eq!(s.length(), 2); // surrogate pair
        assert_eq!(s.code_point_at(0), Some(0x1F600));
        // Starting at the low surrogate gives just that code unit
        assert_eq!(s.code_point_at(1), Some(0xDE00));
    }

    // -- concat -------------------------------------------------------------

    #[test]
    fn concat_latin1_latin1() {
        let a = JsString::from_rust_str("hello");
        let b = JsString::from_rust_str(" world");
        let c = a.concat(&b);
        assert!(c.is_latin1());
        assert_eq!(c.to_utf8(), "hello world");
    }

    #[test]
    fn concat_latin1_utf16() {
        let a = JsString::from_rust_str("hi");
        let b = JsString::from_rust_str("\u{1F600}");
        let c = a.concat(&b);
        assert!(!c.is_latin1());
        assert_eq!(c.length(), 4);
        assert_eq!(c.to_utf8(), "hi\u{1F600}");
    }

    #[test]
    fn concat_utf16_utf16() {
        let a = JsString::from_rust_str("\u{1F600}");
        let b = JsString::from_rust_str("\u{1F601}");
        let c = a.concat(&b);
        assert!(!c.is_latin1());
        assert_eq!(c.length(), 4);
        assert_eq!(c.to_utf8(), "\u{1F600}\u{1F601}");
    }

    // -- slice --------------------------------------------------------------

    #[test]
    fn slice_basic() {
        let s = JsString::from_rust_str("hello world");
        let sub = s.slice(0, 5);
        assert_eq!(sub.to_utf8(), "hello");
    }

    #[test]
    fn slice_empty() {
        let s = JsString::from_rust_str("hello");
        let sub = s.slice(3, 3);
        assert!(sub.is_empty());
    }

    #[test]
    fn slice_full() {
        let s = JsString::from_rust_str("hello");
        let sub = s.slice(0, 5);
        assert_eq!(sub.to_utf8(), "hello");
    }

    // -- index_of -----------------------------------------------------------

    #[test]
    fn index_of_found() {
        let haystack = JsString::from_rust_str("hello world");
        let needle = JsString::from_rust_str("world");
        assert_eq!(haystack.index_of(&needle, 0), Some(6));
    }

    #[test]
    fn index_of_not_found() {
        let haystack = JsString::from_rust_str("hello");
        let needle = JsString::from_rust_str("xyz");
        assert_eq!(haystack.index_of(&needle, 0), None);
    }

    #[test]
    fn index_of_from_offset() {
        let haystack = JsString::from_rust_str("abcabc");
        let needle = JsString::from_rust_str("abc");
        assert_eq!(haystack.index_of(&needle, 0), Some(0));
        assert_eq!(haystack.index_of(&needle, 1), Some(3));
    }

    // -- to_utf8 / to_utf16_vec round-trips ---------------------------------

    #[test]
    fn to_utf8_round_trip_latin1() {
        let original = "hello";
        let s = JsString::from_rust_str(original);
        assert_eq!(s.to_utf8(), original);
    }

    #[test]
    fn to_utf16_vec_round_trip() {
        let units: Vec<u16> = "hello \u{1F600}".encode_utf16().collect();
        let s = JsString::from_utf16(&units);
        assert_eq!(s.to_utf16_vec(), units);
    }

    // -- Cross-encoding equality --------------------------------------------

    #[test]
    fn cross_encoding_equality() {
        let latin1 = JsString::from_latin1(b"abc");
        let utf16 = JsString::from_utf16(&[0x61, 0x62, 0x63]);
        assert_eq!(latin1, utf16);
    }

    // -- Cross-encoding hash ------------------------------------------------

    #[test]
    fn cross_encoding_hash() {
        let latin1 = JsString::from_latin1(b"abc");
        let utf16 = JsString::from_utf16(&[0x61, 0x62, 0x63]);
        assert_eq!(hash_of(&latin1), hash_of(&utf16));
    }

    // -- Unpaired surrogates ------------------------------------------------

    #[test]
    fn unpaired_surrogates() {
        // A lone high surrogate — valid in JS strings
        let s = JsString::from_utf16(&[0xD800]);
        assert_eq!(s.length(), 1);
        assert_eq!(s.char_code_at(0), Some(0xD800));
        // code_point_at should return the surrogate value when unpaired
        assert_eq!(s.code_point_at(0), Some(0xD800));
    }

    // -- Display ------------------------------------------------------------

    #[test]
    fn display_shows_correct_text() {
        let s = JsString::from_rust_str("hello");
        assert_eq!(format!("{s}"), "hello");
    }

    #[test]
    fn display_utf16() {
        let s = JsString::from_rust_str("hi\u{1F600}");
        assert_eq!(format!("{s}"), "hi\u{1F600}");
    }

    // -- from_static_latin1 -------------------------------------------------

    #[test]
    fn from_static_latin1_works() {
        static DATA: &[u8] = b"static";
        let s = JsString::from_static_latin1(DATA);
        assert!(s.is_latin1());
        assert_eq!(s.to_utf8(), "static");
    }

    // -- Edge cases: empty strings -------------------------------------------

    #[test]
    fn test_empty_string_concat() {
        let empty = JsString::empty();
        let hello = JsString::from_rust_str("hello");
        let result = empty.concat(&hello);
        assert_eq!(result.to_utf8(), "hello");
        let result2 = hello.concat(&empty);
        assert_eq!(result2.to_utf8(), "hello");
    }

    #[test]
    fn test_two_empty_strings_concat() {
        let a = JsString::empty();
        let b = JsString::empty();
        let result = a.concat(&b);
        assert!(result.is_empty());
        assert_eq!(result.length(), 0);
    }

    #[test]
    fn test_empty_string_slice() {
        let s = JsString::empty();
        let sub = s.slice(0, 0);
        assert!(sub.is_empty());
    }

    #[test]
    fn test_empty_string_index_of() {
        let haystack = JsString::from_rust_str("hello");
        let empty_needle = JsString::empty();
        // Empty needle at from=0 returns Some(0)
        assert_eq!(haystack.index_of(&empty_needle, 0), Some(0));
        // Empty needle at from=5 (end) returns Some(5)
        assert_eq!(haystack.index_of(&empty_needle, 5), Some(5));
        // Empty needle past end returns None
        assert_eq!(haystack.index_of(&empty_needle, 6), None);
    }

    #[test]
    fn test_index_of_empty_in_empty() {
        let empty = JsString::empty();
        let needle = JsString::empty();
        assert_eq!(empty.index_of(&needle, 0), Some(0));
        assert_eq!(empty.index_of(&needle, 1), None);
    }

    // -- Edge cases: single character ----------------------------------------

    #[test]
    fn test_single_char_string() {
        let s = JsString::from_rust_str("a");
        assert_eq!(s.length(), 1);
        assert!(!s.is_empty());
        assert_eq!(s.char_code_at(0), Some(0x61));
        assert_eq!(s.char_code_at(1), None);
    }

    #[test]
    fn test_single_char_code_point_at() {
        let s = JsString::from_rust_str("A");
        assert_eq!(s.code_point_at(0), Some(0x41));
        assert_eq!(s.code_point_at(1), None);
    }

    // -- Edge cases: multi-byte UTF-8 ----------------------------------------

    #[test]
    fn test_multi_byte_utf8_cjk() {
        let s = JsString::from_rust_str("\u{4E16}\u{754C}"); // "world" in Chinese
        assert!(!s.is_latin1());
        assert_eq!(s.length(), 2);
        assert_eq!(s.char_code_at(0), Some(0x4E16));
        assert_eq!(s.char_code_at(1), Some(0x754C));
    }

    #[test]
    fn test_supplementary_plane_string_length() {
        // U+1F600 requires surrogate pair in UTF-16
        let s = JsString::from_rust_str("\u{1F600}");
        assert_eq!(s.length(), 2); // 2 code units (surrogate pair)
    }

    // -- Edge cases: slice boundary conditions -------------------------------

    #[test]
    fn test_slice_start_past_end() {
        let s = JsString::from_rust_str("hello");
        let sub = s.slice(10, 20);
        assert!(sub.is_empty());
    }

    #[test]
    fn test_slice_end_past_length_clamped() {
        let s = JsString::from_rust_str("hello");
        let sub = s.slice(2, 100);
        assert_eq!(sub.to_utf8(), "llo");
    }

    #[test]
    fn test_slice_start_greater_than_end() {
        let s = JsString::from_rust_str("hello");
        let sub = s.slice(3, 1);
        assert!(sub.is_empty());
    }

    #[test]
    fn test_slice_single_char() {
        let s = JsString::from_rust_str("hello");
        let sub = s.slice(2, 3);
        assert_eq!(sub.to_utf8(), "l");
    }

    // -- Edge cases: index_of edge cases ------------------------------------

    #[test]
    fn test_index_of_needle_longer_than_haystack() {
        let haystack = JsString::from_rust_str("hi");
        let needle = JsString::from_rust_str("hello world");
        assert_eq!(haystack.index_of(&needle, 0), None);
    }

    #[test]
    fn test_index_of_from_past_end() {
        let haystack = JsString::from_rust_str("hello");
        let needle = JsString::from_rust_str("h");
        assert_eq!(haystack.index_of(&needle, 100), None);
    }

    #[test]
    fn test_index_of_at_last_position() {
        let haystack = JsString::from_rust_str("abc");
        let needle = JsString::from_rust_str("c");
        assert_eq!(haystack.index_of(&needle, 0), Some(2));
        assert_eq!(haystack.index_of(&needle, 2), Some(2));
        assert_eq!(haystack.index_of(&needle, 3), None);
    }

    // -- Edge cases: cross-encoding operations -------------------------------

    #[test]
    fn test_concat_utf16_latin1() {
        // Reversed order from existing test
        let a = JsString::from_rust_str("\u{1F600}");
        let b = JsString::from_rust_str("hi");
        let c = a.concat(&b);
        assert!(!c.is_latin1());
        assert_eq!(c.length(), 4);
        assert_eq!(c.to_utf8(), "\u{1F600}hi");
    }

    #[test]
    fn test_cross_encoding_index_of() {
        let haystack = JsString::from_latin1(b"abc");
        let needle = JsString::from_utf16(&[0x62]); // 'b' in UTF-16
        assert_eq!(haystack.index_of(&needle, 0), Some(1));
    }

    // -- Edge cases: equality and hashing ------------------------------------

    #[test]
    fn test_different_length_not_equal() {
        let a = JsString::from_rust_str("hello");
        let b = JsString::from_rust_str("hell");
        assert_ne!(a, b);
    }

    #[test]
    fn test_empty_strings_equal() {
        let a = JsString::empty();
        let b = JsString::from_latin1(&[]);
        let c = JsString::from_utf16(&[]);
        assert_eq!(a, b);
        assert_eq!(a, c);
    }

    #[test]
    fn test_hash_consistency_for_equal_strings() {
        let a = JsString::from_rust_str("test");
        let b = JsString::from_rust_str("test");
        assert_eq!(hash_of(&a), hash_of(&b));
    }

    #[test]
    fn test_hash_in_hashset() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(JsString::from_rust_str("a"));
        set.insert(JsString::from_rust_str("a"));
        set.insert(JsString::from_rust_str("b"));
        assert_eq!(set.len(), 2);
    }

    // -- Edge cases: as_latin1 / as_utf16 ------------------------------------

    #[test]
    fn test_as_latin1_on_utf16_string() {
        let s = JsString::from_rust_str("\u{1F600}");
        assert!(s.as_latin1().is_none());
        assert!(s.as_utf16().is_some());
    }

    #[test]
    fn test_as_utf16_on_latin1_string() {
        let s = JsString::from_rust_str("hello");
        assert!(s.as_latin1().is_some());
        assert!(s.as_utf16().is_none());
    }

    // -- Edge cases: to_utf8 with lone surrogates ----------------------------

    #[test]
    fn test_to_utf8_lone_surrogates_replaced() {
        let s = JsString::from_utf16(&[0xD800, 0x0041]); // lone high surrogate + 'A'
        let utf8 = s.to_utf8();
        // Lone surrogates are replaced with U+FFFD
        assert!(utf8.contains('\u{FFFD}'));
        assert!(utf8.contains('A'));
    }

    // -- Edge cases: code_point_at with lone surrogates ----------------------

    #[test]
    fn test_code_point_at_lone_low_surrogate() {
        let s = JsString::from_utf16(&[0xDC00]); // lone low surrogate
        assert_eq!(s.code_point_at(0), Some(0xDC00));
    }

    #[test]
    fn test_code_point_at_high_surrogate_at_end() {
        let s = JsString::from_utf16(&[0x0041, 0xD800]); // 'A' + lone high surrogate
        assert_eq!(s.code_point_at(0), Some(0x41));
        // High surrogate at end has no following code unit
        assert_eq!(s.code_point_at(1), Some(0xD800));
    }

    // -- Edge cases: from_latin1 with all byte values -----------------------

    #[test]
    fn test_from_latin1_all_byte_values() {
        let bytes: Vec<u8> = (0..=255).collect();
        let s = JsString::from_latin1(&bytes);
        assert_eq!(s.length(), 256);
        assert!(s.is_latin1());
        for i in 0..256 {
            assert_eq!(s.char_code_at(i as u32), Some(i as u16));
        }
    }

    // -- utf16_len ----------------------------------------------------------

    #[test]
    fn utf16_len_ascii() {
        let s = JsString::from_rust_str("hello");
        assert_eq!(s.utf16_len(), 5);
    }

    #[test]
    fn utf16_len_empty() {
        let s = JsString::empty();
        assert_eq!(s.utf16_len(), 0);
    }

    #[test]
    fn utf16_len_with_emoji() {
        // U+1F600 = surrogate pair = 2 UTF-16 code units
        let s = JsString::from_rust_str("hi\u{1F600}");
        assert_eq!(s.utf16_len(), 4);
    }

    #[test]
    fn utf16_len_latin1_extended() {
        // e-acute fits in one code unit
        let s = JsString::from_rust_str("\u{00E9}");
        assert_eq!(s.utf16_len(), 1);
    }

    // -- byte_index_to_utf16 ------------------------------------------------

    #[test]
    fn test_byte_to_utf16_ascii() {
        let s = JsString::from_rust_str("hello");
        assert_eq!(s.byte_index_to_utf16(0), Some(0));
        assert_eq!(s.byte_index_to_utf16(1), Some(1));
        assert_eq!(s.byte_index_to_utf16(4), Some(4));
        assert_eq!(s.byte_index_to_utf16(5), Some(5)); // past end
    }

    #[test]
    fn test_byte_to_utf16_latin1_extended() {
        // "\u{00E9}" (e-acute) is 2 bytes in UTF-8, 1 code unit in UTF-16
        let s = JsString::from_rust_str("\u{00E9}n");
        assert_eq!(s.byte_index_to_utf16(0), Some(0)); // start of e-acute
        assert_eq!(s.byte_index_to_utf16(1), None); // mid-sequence
        assert_eq!(s.byte_index_to_utf16(2), Some(1)); // 'n'
        assert_eq!(s.byte_index_to_utf16(3), Some(2)); // past end
    }

    #[test]
    fn test_byte_to_utf16_emoji() {
        // "hi\u{1F600}" = UTF-8: h(1) i(1) emoji(4) = 6 bytes
        //                 UTF-16: h(1) i(1) surrogate-pair(2) = 4 code units
        let s = JsString::from_rust_str("hi\u{1F600}");
        assert_eq!(s.byte_index_to_utf16(0), Some(0)); // 'h'
        assert_eq!(s.byte_index_to_utf16(1), Some(1)); // 'i'
        assert_eq!(s.byte_index_to_utf16(2), Some(2)); // start of emoji
        assert_eq!(s.byte_index_to_utf16(3), None); // mid-sequence
        assert_eq!(s.byte_index_to_utf16(4), None); // mid-sequence
        assert_eq!(s.byte_index_to_utf16(5), None); // mid-sequence
        assert_eq!(s.byte_index_to_utf16(6), Some(4)); // past emoji
    }

    #[test]
    fn test_byte_to_utf16_out_of_bounds() {
        let s = JsString::from_rust_str("abc");
        assert_eq!(s.byte_index_to_utf16(100), None);
    }

    #[test]
    fn test_byte_to_utf16_empty() {
        let s = JsString::from_rust_str("");
        assert_eq!(s.byte_index_to_utf16(0), Some(0));
        assert_eq!(s.byte_index_to_utf16(1), None);
    }

    // -- utf16_to_byte_index ------------------------------------------------

    #[test]
    fn test_utf16_to_byte_ascii() {
        let s = JsString::from_rust_str("hello");
        assert_eq!(s.utf16_to_byte_index(0), Some(0));
        assert_eq!(s.utf16_to_byte_index(1), Some(1));
        assert_eq!(s.utf16_to_byte_index(4), Some(4));
        assert_eq!(s.utf16_to_byte_index(5), Some(5)); // past end
    }

    #[test]
    fn test_utf16_to_byte_latin1_extended() {
        // "\u{00E9}n" = UTF-16 index 0 → byte 0, index 1 → byte 2
        let s = JsString::from_rust_str("\u{00E9}n");
        assert_eq!(s.utf16_to_byte_index(0), Some(0)); // e-acute
        assert_eq!(s.utf16_to_byte_index(1), Some(2)); // 'n'
        assert_eq!(s.utf16_to_byte_index(2), Some(3)); // past end
    }

    #[test]
    fn test_utf16_to_byte_emoji() {
        // "hi\u{1F600}" = UTF-16 indices: 0='h', 1='i', 2=hi_surr, 3=lo_surr
        let s = JsString::from_rust_str("hi\u{1F600}");
        assert_eq!(s.utf16_to_byte_index(0), Some(0)); // 'h'
        assert_eq!(s.utf16_to_byte_index(1), Some(1)); // 'i'
        assert_eq!(s.utf16_to_byte_index(2), Some(2)); // start of emoji
        assert_eq!(s.utf16_to_byte_index(3), None); // low surrogate (mid-pair)
        assert_eq!(s.utf16_to_byte_index(4), Some(6)); // past emoji
    }

    #[test]
    fn test_utf16_to_byte_out_of_bounds() {
        let s = JsString::from_rust_str("abc");
        assert_eq!(s.utf16_to_byte_index(100), None);
    }

    #[test]
    fn test_utf16_to_byte_empty() {
        let s = JsString::from_rust_str("");
        assert_eq!(s.utf16_to_byte_index(0), Some(0));
        assert_eq!(s.utf16_to_byte_index(1), None);
    }

    // -- Round-trip: byte → utf16 → byte ------------------------------------

    #[test]
    fn test_byte_utf16_round_trip_mixed() {
        // "cafe\u{0301}" = c a f e combining-accent
        // UTF-8: c(1) a(1) f(1) e(1) \u{0301}(2) = 6 bytes
        // UTF-16: c(1) a(1) f(1) e(1) \u{0301}(1) = 5 code units
        let s = JsString::from_rust_str("cafe\u{0301}");
        for byte_idx in [0, 1, 2, 3, 4, 6] {
            if let Some(utf16_idx) = s.byte_index_to_utf16(byte_idx)
                && let Some(back) = s.utf16_to_byte_index(utf16_idx)
            {
                assert_eq!(back, byte_idx, "round-trip failed for byte {byte_idx}");
            }
        }
    }

    // -- CJK characters (BMP, multi-byte UTF-8, single UTF-16 code unit) ----

    #[test]
    fn test_byte_to_utf16_cjk() {
        // "\u{4E16}" (CJK "world") = 3 bytes UTF-8, 1 code unit UTF-16
        let s = JsString::from_rust_str("\u{4E16}");
        assert_eq!(s.utf16_len(), 1);
        assert_eq!(s.byte_index_to_utf16(0), Some(0));
        assert_eq!(s.byte_index_to_utf16(1), None); // mid-sequence
        assert_eq!(s.byte_index_to_utf16(2), None); // mid-sequence
        assert_eq!(s.byte_index_to_utf16(3), Some(1)); // past end
    }

    #[test]
    fn test_utf16_to_byte_cjk() {
        let s = JsString::from_rust_str("\u{4E16}");
        assert_eq!(s.utf16_to_byte_index(0), Some(0));
        assert_eq!(s.utf16_to_byte_index(1), Some(3)); // past end
    }

    // -- Multiple emoji in sequence -----------------------------------------

    #[test]
    fn test_multiple_emoji() {
        // "\u{1F389}\u{1F600}" = party popper + grinning face
        // UTF-8: 4+4 = 8 bytes, UTF-16: 2+2 = 4 code units
        let s = JsString::from_rust_str("\u{1F389}\u{1F600}");
        assert_eq!(s.utf16_len(), 4);
        assert_eq!(s.byte_index_to_utf16(0), Some(0)); // start of party popper
        assert_eq!(s.byte_index_to_utf16(4), Some(2)); // start of grinning face
        assert_eq!(s.byte_index_to_utf16(8), Some(4)); // past end

        assert_eq!(s.utf16_to_byte_index(0), Some(0));
        assert_eq!(s.utf16_to_byte_index(1), None); // low surrogate
        assert_eq!(s.utf16_to_byte_index(2), Some(4));
        assert_eq!(s.utf16_to_byte_index(3), None); // low surrogate
        assert_eq!(s.utf16_to_byte_index(4), Some(8));
    }

    // -- Proptest: from_str -> to_utf8 round-trip ---------------------------

    proptest::proptest! {
        #[test]
        fn proptest_from_str_to_utf8_round_trip(s in ".*") {
            let js = JsString::from_rust_str(&s);
            let back = js.to_utf8();
            assert_eq!(s, back);
        }
    }
}

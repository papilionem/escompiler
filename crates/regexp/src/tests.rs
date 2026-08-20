//! Unit tests for regexp.

use crate::flags::RegExpFlags;
use crate::matcher::{JsRegExp, RegExpError};

// -------------------------------------------------------------------------
// Flag parsing
// -------------------------------------------------------------------------

#[test]
fn test_parse_flags_gi() {
    let f = RegExpFlags::parse("gi").unwrap();
    assert!(f.global);
    assert!(f.ignore_case);
    assert!(!f.multiline);
    assert!(!f.dot_all);
    assert!(!f.unicode);
    assert!(!f.sticky);
}

#[test]
fn test_parse_flags_gms() {
    let f = RegExpFlags::parse("gms").unwrap();
    assert!(f.global);
    assert!(!f.ignore_case);
    assert!(f.multiline);
    assert!(f.dot_all);
}

#[test]
fn test_parse_flags_empty() {
    let f = RegExpFlags::parse("").unwrap();
    assert!(!f.global);
    assert!(!f.ignore_case);
}

#[test]
fn test_parse_flags_invalid() {
    let r = RegExpFlags::parse("gx");
    assert!(r.is_err());
}

#[test]
fn test_parse_flags_duplicate() {
    let r = RegExpFlags::parse("gg");
    assert!(r.is_err());
}

// -------------------------------------------------------------------------
// Basic matching
// -------------------------------------------------------------------------

#[test]
fn test_basic_match() {
    let mut re = JsRegExp::new("hello", "").unwrap();
    assert!(re.test("hello world"));
}

#[test]
fn test_no_match() {
    let mut re = JsRegExp::new("xyz", "").unwrap();
    assert!(!re.test("hello"));
}

#[test]
fn test_case_insensitive() {
    let mut re = JsRegExp::new("hello", "i").unwrap();
    assert!(re.test("HELLO"));
}

#[test]
fn test_multiline() {
    let mut re = JsRegExp::new("^bar", "m").unwrap();
    assert!(re.test("foo\nbar"));
}

#[test]
fn test_dot_all() {
    let mut re = JsRegExp::new("foo.bar", "s").unwrap();
    assert!(re.test("foo\nbar"));
}

#[test]
fn test_dot_all_without_flag() {
    let mut re = JsRegExp::new("foo.bar", "").unwrap();
    assert!(!re.test("foo\nbar"));
}

// -------------------------------------------------------------------------
// Global / sticky / lastIndex
// -------------------------------------------------------------------------

#[test]
fn test_global_exec() {
    let mut re = JsRegExp::new(r"\d+", "g").unwrap();
    let m1 = re.exec("abc 123 def 456").unwrap();
    assert_eq!(m1.full_match, "123");
    assert_eq!(m1.index, 4);

    let m2 = re.exec("abc 123 def 456").unwrap();
    assert_eq!(m2.full_match, "456");
    assert_eq!(m2.index, 12);

    // No more matches — lastIndex resets.
    assert!(re.exec("abc 123 def 456").is_none());
    assert_eq!(re.last_index, 0);
}

#[test]
fn test_sticky() {
    let mut re = JsRegExp::new(r"\d+", "y").unwrap();

    // Sticky requires match at lastIndex (0) — "abc" starts with non-digit.
    assert!(!re.test("abc 123"));
    assert_eq!(re.last_index, 0);

    // Set lastIndex to where digits start.
    re.last_index = 4;
    let m = re.exec("abc 123").unwrap();
    assert_eq!(m.full_match, "123");
    assert_eq!(re.last_index, 7);
}

// -------------------------------------------------------------------------
// Capture groups
// -------------------------------------------------------------------------

#[test]
fn test_exec_groups() {
    let mut re = JsRegExp::new(r"(\d{4})-(\d{2})-(\d{2})", "").unwrap();
    let m = re.exec("date: 2026-03-04 end").unwrap();
    assert_eq!(m.full_match, "2026-03-04");
    assert_eq!(m.groups.len(), 3);
    assert_eq!(m.groups[0].as_deref(), Some("2026"));
    assert_eq!(m.groups[1].as_deref(), Some("03"));
    assert_eq!(m.groups[2].as_deref(), Some("04"));
    assert_eq!(m.index, 6);
}

#[test]
fn test_named_groups() {
    let mut re = JsRegExp::new(r"(?P<year>\d{4})-(?P<month>\d{2})", "").unwrap();
    let m = re.exec("2026-03").unwrap();
    assert_eq!(m.full_match, "2026-03");
    assert_eq!(m.groups[0].as_deref(), Some("2026"));
    assert_eq!(m.groups[1].as_deref(), Some("03"));
}

#[test]
fn test_optional_group_unmatched() {
    let mut re = JsRegExp::new(r"(a)(b)?(c)", "").unwrap();
    let m = re.exec("ac").unwrap();
    assert_eq!(m.full_match, "ac");
    assert_eq!(m.groups.len(), 3);
    assert_eq!(m.groups[0].as_deref(), Some("a"));
    assert!(m.groups[1].is_none());
    assert_eq!(m.groups[2].as_deref(), Some("c"));
}

// -------------------------------------------------------------------------
// matchAll
// -------------------------------------------------------------------------

#[test]
fn test_match_all() {
    let mut re = JsRegExp::new(r"\d+", "g").unwrap();
    let matches = re.match_all("a1b2c3");
    assert_eq!(matches.len(), 3);
    assert_eq!(matches[0].full_match, "1");
    assert_eq!(matches[1].full_match, "2");
    assert_eq!(matches[2].full_match, "3");
}

#[test]
fn test_match_all_no_global() {
    let mut re = JsRegExp::new(r"\d+", "").unwrap();
    let matches = re.match_all("a1b2c3");
    // Non-global returns only the first match.
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].full_match, "1");
}

// -------------------------------------------------------------------------
// Edge cases / errors
// -------------------------------------------------------------------------

#[test]
fn test_empty_pattern() {
    let mut re = JsRegExp::new("(?:)", "").unwrap();
    assert!(re.test(""));
    assert!(re.test("anything"));
}

#[test]
fn test_invalid_pattern() {
    let r = JsRegExp::new("[invalid", "");
    assert!(r.is_err());
    let err = r.unwrap_err();
    assert!(matches!(err, RegExpError::InvalidPattern { .. }));
}

#[test]
fn test_lastindex_beyond_input() {
    let mut re = JsRegExp::new("a", "g").unwrap();
    re.last_index = 999;
    assert!(re.exec("abc").is_none());
    assert_eq!(re.last_index, 0);
}

#[test]
fn test_reset() {
    let mut re = JsRegExp::new(r"\d", "g").unwrap();
    re.exec("a1b2").unwrap();
    assert_ne!(re.last_index, 0);
    re.reset();
    assert_eq!(re.last_index, 0);
}

#[test]
fn test_debug_format() {
    let re = JsRegExp::new(r"\d+", "gi").unwrap();
    let dbg = format!("{re:?}");
    assert!(dbg.contains("/\\d+/gi"));
}

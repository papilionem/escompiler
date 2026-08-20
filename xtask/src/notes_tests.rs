//! Unit tests for release-note fragments.
//!
//! Every test here is a shape that would otherwise produce a *plausible but
//! wrong* published entry — a section heading no reader expects, a bullet with
//! no text, generated content spliced into a file that never asked for it.

use super::*;
use std::io::Write;

// No .unwrap()/.expect() anywhere in this file, including fixtures. The
// forbidden-pattern lint exempts them only within +/-12 lines of a
// `#[cfg(test)]` marker, which cannot hold across thirteen tests -- and
// satisfying the rule is better than arranging to fall inside its exemption.
fn frag(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    let Ok(mut f) = fs::File::create(&p) else {
        panic!("test fixture: cannot create {}", p.display())
    };
    if f.write_all(body.as_bytes()).is_err() {
        panic!("test fixture: cannot write {}", p.display())
    }
    p
}

/// Parse a fixture that the test asserts is well formed.
fn must_parse(p: &Path) -> Fragment {
    match parse_fragment(p) {
        Ok(f) => f,
        Err(e) => panic!("fixture should parse: {e}"),
    }
}

const GOOD: &str =
    "---\nsection: Fixed\nsurface: owned\nwitness: none\npr: 97\n---\n\nIt works now.\n";

#[test]
fn test_parse_fragment_happy_path() {
    let d = tempdir();
    let p = frag(&d, "97-x.md", GOOD);
    let f = must_parse(&p);
    assert_eq!(f.section, "Fixed");
    assert_eq!(f.surface, "owned");
    assert_eq!(f.pr, "97");
    assert_eq!(f.body, "It works now.");
}

#[test]
fn test_parse_fragment_no_front_matter_is_error() {
    let d = tempdir();
    let p = frag(&d, "97-x.md", "just prose, no front matter\n");
    assert!(matches!(
        parse_fragment(&p),
        Err(NotesError::NoFrontMatter { .. })
    ));
}

#[test]
fn test_parse_fragment_missing_witness_is_error() {
    // witness: is the field carrying V4's argument. Defaulting it would turn a
    // breaking change into a "fix" without anyone deciding to.
    let d = tempdir();
    let p = frag(
        &d,
        "97-x.md",
        "---\nsection: Fixed\nsurface: owned\npr: 97\n---\n\nBody.\n",
    );
    assert!(matches!(
        parse_fragment(&p),
        Err(NotesError::MissingField {
            field: "witness",
            ..
        })
    ));
}

#[test]
fn test_parse_fragment_rejects_unknown_section() {
    let d = tempdir();
    let p = frag(
        &d,
        "97-x.md",
        "---\nsection: Improved\nsurface: owned\nwitness: none\npr: 97\n---\n\nBody.\n",
    );
    assert!(matches!(
        parse_fragment(&p),
        Err(NotesError::BadSection { .. })
    ));
}

#[test]
fn test_parse_fragment_rejects_unknown_surface() {
    let d = tempdir();
    let p = frag(
        &d,
        "97-x.md",
        "---\nsection: Fixed\nsurface: both\nwitness: none\npr: 97\n---\n\nBody.\n",
    );
    assert!(matches!(
        parse_fragment(&p),
        Err(NotesError::BadSurface { .. })
    ));
}

#[test]
fn test_parse_fragment_rejects_empty_body() {
    let d = tempdir();
    let p = frag(
        &d,
        "97-x.md",
        "---\nsection: Fixed\nsurface: owned\nwitness: none\npr: 97\n---\n\n\n",
    );
    assert!(matches!(
        parse_fragment(&p),
        Err(NotesError::EmptyBody { .. })
    ));
}

#[test]
fn test_load_fragments_skips_the_schema_readme() {
    let d = tempdir();
    frag(&d, "README.md", "# schema, not a fragment\n");
    frag(&d, "97-x.md", GOOD);
    let Ok(all) = load_fragments(&d) else {
        panic!("directory with one fragment should load")
    };
    assert_eq!(all.len(), 1, "README.md must not be parsed as a fragment");
}

#[test]
fn test_load_fragments_missing_dir_is_empty_not_error() {
    let d = tempdir();
    let Ok(all) = load_fragments(&d.join("nope")) else {
        panic!("an absent directory is not an error")
    };
    assert!(all.is_empty());
}

#[test]
fn test_render_groups_by_section_in_fixed_order() {
    let d = tempdir();
    let a = must_parse(&frag(
        &d,
        "1-a.md",
        "---\nsection: Removed\nsurface: owned\nwitness: none\npr: 1\n---\n\nGone.\n",
    ));
    let b = must_parse(&frag(&d, "2-b.md", GOOD));
    let out = render("0.9.0", "2026-08-20", &[a, b]);
    let Some(fixed) = out.find("### Fixed") else {
        panic!("Fixed section present")
    };
    let Some(removed) = out.find("### Removed") else {
        panic!("Removed section present")
    };
    assert!(
        fixed < removed,
        "sections follow the Common Changelog order"
    );
    assert!(out.contains("- It works now. ([#97])"));
}

#[test]
fn test_render_omits_empty_sections() {
    let d = tempdir();
    let f = must_parse(&frag(&d, "1-a.md", GOOD));
    let out = render("0.9.0", "2026-08-20", &[f]);
    assert!(
        !out.contains("### Added"),
        "an empty section must not be printed"
    );
}

#[test]
fn test_splice_replaces_only_the_generated_region() {
    let doc = format!("head\n{BEGIN}\nOLD\n{END}\ntail\n");
    let Ok(out) = splice(&doc, "NEW") else {
        panic!("region present")
    };
    assert!(out.contains("head"), "text before the region survives");
    assert!(out.contains("tail"), "text after the region survives");
    assert!(out.contains("NEW"));
    assert!(
        !out.contains("OLD"),
        "previous generated content is replaced"
    );
}

#[test]
fn test_splice_refuses_when_region_absent() {
    // Refusing beats appending: guessing where generated content belongs is how
    // a generator quietly destroys hand-written text.
    assert!(matches!(
        splice("no markers here\n", "NEW"),
        Err(NotesError::NoRegion)
    ));
}

#[test]
fn test_splice_refuses_reversed_markers() {
    let doc = format!("{END}\n{BEGIN}\n");
    assert!(matches!(splice(&doc, "NEW"), Err(NotesError::NoRegion)));
}

fn tempdir() -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "esc-notes-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = fs::remove_dir_all(&base);
    if fs::create_dir_all(&base).is_err() {
        panic!("test fixture: cannot create temp dir")
    }
    base
}

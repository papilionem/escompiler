//! Release-note fragments: assemble `CHANGELOG.md`, and refuse when a change
//! that needs a note does not have one.
//!
//! ADR-0002 V11 makes the **declared fragment the authority** — `esc-bump`'s
//! computed floor may raise the bump a fragment declares, never lower it. V12
//! makes **absence a refusal**: a range touching semantic source with no
//! fragment is an error, not an empty release note.
//!
//! Three subcommands, and the third is the one that keeps the other two honest:
//!
//! - `notes --version X.Y.Z` assembles the fragments into `CHANGELOG.md`
//! - `notes --check <base>` implements V12 over a git range
//! - `notes --self-test` runs the controls, because a generator that has never
//!   been shown to refuse anything is a generator nobody has tested

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Common Changelog defines exactly these four. Anything else is a typo, and
/// accepting it silently would put a section heading in the published file that
/// no reader expects.
const SECTIONS: [&str; 4] = ["Added", "Changed", "Fixed", "Removed"];

/// Paths whose change requires a fragment (V12). Everything else — docs, the
/// planning tree, CI config — legitimately ships without one.
const SEMANTIC_PREFIXES: [&str; 2] = ["crates/", "xtask/"];

const BEGIN: &str = "<!-- esc:notes-begin -->";
const END: &str = "<!-- esc:notes-end -->";

/// Everything that can go wrong parsing or assembling fragments.
///
/// Each variant names the file and the offending value, because a release-note
/// generator that fails with "invalid fragment" makes the author guess.
#[derive(Debug)]
pub enum NotesError {
    /// A fragment has no `---` front-matter block.
    NoFrontMatter { file: PathBuf },
    /// A required field is absent.
    MissingField { file: PathBuf, field: &'static str },
    /// `section:` is not one of the four Common Changelog sections.
    BadSection { file: PathBuf, got: String },
    /// `surface:` is not `owned` or `tc39`.
    BadSurface { file: PathBuf, got: String },
    /// The fragment has front matter but no prose after it.
    EmptyBody { file: PathBuf },
    /// `CHANGELOG.md` is missing the generated-region markers.
    NoRegion,
    /// An I/O failure, with the path that caused it.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl fmt::Display for NotesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoFrontMatter { file } => write!(
                f,
                "{}: no `---` front-matter block. See .changes/README.md",
                file.display()
            ),
            Self::MissingField { file, field } => {
                write!(f, "{}: front matter has no `{field}:`", file.display())
            }
            Self::BadSection { file, got } => write!(
                f,
                "{}: section `{got}` is not one of {}",
                file.display(),
                SECTIONS.join(", ")
            ),
            Self::BadSurface { file, got } => write!(
                f,
                "{}: surface `{got}` is not `owned` or `tc39`",
                file.display()
            ),
            Self::EmptyBody { file } => write!(
                f,
                "{}: front matter but no prose. A fragment with no text produces \
                 an entry a user cannot read",
                file.display()
            ),
            Self::NoRegion => write!(
                f,
                "CHANGELOG.md has no {BEGIN} / {END} region — refusing rather than \
                 guessing where generated content belongs"
            ),
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
        }
    }
}

/// One parsed release-note fragment.
#[derive(Debug, Clone)]
pub struct Fragment {
    pub file: PathBuf,
    pub section: String,
    pub surface: String,
    pub witness: String,
    pub pr: String,
    pub body: String,
}

impl Fragment {
    /// Render as one Common Changelog bullet, with its PR reference.
    ///
    /// The PR is not decoration: every published entry must give a reader a path
    /// from the claim back to the diff.
    fn to_entry(&self) -> String {
        format!("- {} ([#{}])", self.body.trim(), self.pr)
    }
}

fn read(path: &Path) -> Result<String, NotesError> {
    fs::read_to_string(path).map_err(|source| NotesError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Parse one fragment, rejecting every shape that would produce a misleading
/// entry rather than repairing it.
pub fn parse_fragment(path: &Path) -> Result<Fragment, NotesError> {
    let text = read(path)?;
    let rest = text
        .strip_prefix("---\n")
        .ok_or_else(|| NotesError::NoFrontMatter {
            file: path.to_path_buf(),
        })?;
    let (front, body) = rest
        .split_once("\n---\n")
        .ok_or_else(|| NotesError::NoFrontMatter {
            file: path.to_path_buf(),
        })?;

    let mut fields: BTreeMap<&str, String> = BTreeMap::new();
    for line in front.lines() {
        if let Some((k, v)) = line.split_once(':') {
            fields.insert(k.trim(), v.trim().to_string());
        }
    }

    let get = |key: &'static str| -> Result<String, NotesError> {
        fields
            .get(key)
            .filter(|v| !v.is_empty())
            .cloned()
            .ok_or(NotesError::MissingField {
                file: path.to_path_buf(),
                field: key,
            })
    };

    let section = get("section")?;
    if !SECTIONS.contains(&section.as_str()) {
        return Err(NotesError::BadSection {
            file: path.to_path_buf(),
            got: section,
        });
    }
    let surface = get("surface")?;
    if surface != "owned" && surface != "tc39" {
        return Err(NotesError::BadSurface {
            file: path.to_path_buf(),
            got: surface,
        });
    }
    let witness = get("witness")?;
    let pr = get("pr")?;

    let body = body.trim().to_string();
    if body.is_empty() {
        return Err(NotesError::EmptyBody {
            file: path.to_path_buf(),
        });
    }

    Ok(Fragment {
        file: path.to_path_buf(),
        section,
        surface,
        witness,
        pr,
        body,
    })
}

/// Every fragment in `dir`, sorted by file name so output is deterministic.
///
/// `README.md` is the schema, not a fragment.
pub fn load_fragments(dir: &Path) -> Result<Vec<Fragment>, NotesError> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|source| NotesError::Io {
            path: dir.to_path_buf(),
            source,
        })?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .filter(|p| p.file_name().is_some_and(|n| n != "README.md"))
        .collect();
    paths.sort();

    paths.iter().map(|p| parse_fragment(p)).collect()
}

/// Render the Common Changelog section for one version.
pub fn render(version: &str, date: &str, frags: &[Fragment]) -> String {
    let mut out = format!("## [{version}] - {date}\n");
    for section in SECTIONS {
        let matching: Vec<&Fragment> = frags.iter().filter(|f| f.section == section).collect();
        if matching.is_empty() {
            continue;
        }
        out.push_str(&format!("\n### {section}\n\n"));
        for f in matching {
            out.push_str(&f.to_entry());
            out.push('\n');
        }
    }
    out
}

/// Splice rendered notes into `CHANGELOG.md`'s generated region.
///
/// Refuses when the region markers are absent rather than appending somewhere
/// plausible: guessing where generated content belongs is how a generator
/// quietly destroys hand-written text.
pub fn splice(changelog: &str, rendered: &str) -> Result<String, NotesError> {
    let b = changelog.find(BEGIN).ok_or(NotesError::NoRegion)?;
    let e = changelog.find(END).ok_or(NotesError::NoRegion)?;
    if e < b {
        return Err(NotesError::NoRegion);
    }
    let mut out = String::new();
    out.push_str(&changelog[..b + BEGIN.len()]);
    out.push_str("\n\n");
    out.push_str(rendered.trim_end());
    out.push_str("\n\n");
    out.push_str(&changelog[e..]);
    Ok(out)
}

/// Semantic files with uncommitted modifications.
///
/// [`semantic_changes`] compares committed history, so running the check on a
/// dirty tree computes a diff that is not the diff being proposed. On this very
/// pull request that produced `semantic files changed: 0` and a green result
/// while three `xtask/` files sat modified — the same shape as the diff gates
/// reporting `Examined 0 changed files ... passed`.
///
/// The caller refuses on a non-empty result rather than warning: a check whose
/// answer depends on whether someone remembered to commit is not a check.
pub fn uncommitted_semantic(root: &Path) -> Vec<String> {
    let Ok(out) = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain"])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.get(3..).map(str::to_string))
        .filter(|p| SEMANTIC_PREFIXES.iter().any(|pre| p.starts_with(pre)))
        .collect()
}

/// Files in `base..HEAD` whose change requires a fragment.
pub fn semantic_changes(root: &Path, base: &str) -> Vec<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["diff", "--name-only", &format!("{base}...HEAD")])
        .output();
    let Ok(out) = out else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .filter(|p| SEMANTIC_PREFIXES.iter().any(|pre| p.starts_with(pre)))
        .collect()
}

#[cfg(test)]
#[path = "notes_tests.rs"]
mod tests;

//! The corpus manifest.
//!
//! One file lists every entry the verifier checks, each with the KIND of claim
//! being made about it. The kinds are a closed set, and there is no kind meaning
//! "run it and see" — an entry that does not state what would falsify it cannot
//! be added.

use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct Manifest {
    /// Node version this corpus was verified against, e.g. "v24.14.0".
    /// Checked at run time: a differential against whatever node happens to be
    /// installed is not a differential.
    pub node_pin: String,
    #[serde(default)]
    pub entry: Vec<Entry>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Entry {
    /// Stable id. This is the string the ticket `accept:` blocks join on, so it
    /// must not drift — the plan↔corpus check is a string comparison.
    pub id: String,
    /// Path to the JS program, relative to the repo root.
    pub program: String,
    #[serde(flatten)]
    pub kind: Kind,
    /// Why this entry exists. Printed when the entry FAILS, so it is read at
    /// exactly the moment someone needs it — an unread field is dead weight.
    #[serde(default)]
    pub note: String,
    /// Known-failing, with a reason. Same three rules as the fixture XFAIL
    /// registry: failing while listed is expected, failing unlisted is a hard
    /// failure, and PASSING while listed is ALSO a hard failure — that last rule
    /// is what makes a fix an event the tool announces.
    #[serde(default)]
    pub xfail: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Kind {
    /// stdout byte-identical to the pinned Node, plus stderr CLASS and
    /// exit-status class. The external oracle: the only check here that compares
    /// against something other than ourselves.
    Match,
    /// The compiler must REFUSE, exiting 2 with a declared `ESC-Ennn` code.
    /// Refusal and failure-to-compile must stay distinguishable, since both
    /// otherwise exit 1.
    Refused { code: String },
    /// A property of the built artifact — e.g. a symbol that must be absent.
    Artifact { absent_symbols: Vec<String> },
}

impl Manifest {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read manifest {}: {e}", path.display()))?;
        let m: Manifest = toml::from_str(&text)
            .map_err(|e| format!("cannot parse manifest {}: {e}", path.display()))?;
        // An empty manifest is a failure, never a clean run. This is the
        // "0 entries executed" hazard at its source.
        if m.entry.is_empty() {
            return Err(format!(
                "manifest {} declares zero entries. That is a failure, not a pass — \
                 a verifier with nothing to verify must never report success.",
                path.display()
            ));
        }
        Ok(m)
    }
}

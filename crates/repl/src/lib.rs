//! REPL via Cranelift JIT (Phase 3+).
//!
//! Special commands: :report fn, :ir fn, :type expr, :quit

pub struct ReplSession {
    _private: (),
}

impl ReplSession {
    pub fn new() -> Self {
        Self { _private: () }
    }

    pub fn eval_line(&mut self, _line: &str) -> Result<String, String> {
        Err("REPL not yet implemented (Phase 3+)".to_string())
    }
}

impl Default for ReplSession {
    fn default() -> Self {
        Self::new()
    }
}

/// Start the interactive REPL. Phase 3+.
pub fn run_repl() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("esc repl: Phase 3+ — will use Cranelift JIT for expression-level compilation");
    eprintln!("  commands: :report <fn>, :ir <fn>, :type <expr>, :quit");
    Ok(())
}

#[cfg(test)]
mod tests;

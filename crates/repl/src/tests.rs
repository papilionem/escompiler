use super::*;

#[test]
fn test_repl_session_new() {
    let session = ReplSession::new();
    // Verify construction succeeds (no panic).
    let _ = session;
}

#[test]
fn test_repl_session_default() {
    let session = ReplSession::default();
    let _ = session;
}

#[test]
fn test_eval_line_returns_not_implemented() {
    let mut session = ReplSession::new();
    let result = session.eval_line("1 + 2");
    assert!(result.is_err());
    let msg = result.unwrap_err();
    assert!(
        msg.contains("not yet implemented"),
        "expected 'not yet implemented', got: {msg}"
    );
}

#[test]
fn test_eval_line_empty_input() {
    let mut session = ReplSession::new();
    let result = session.eval_line("");
    assert!(result.is_err());
}

#[test]
fn test_eval_line_special_command() {
    let mut session = ReplSession::new();
    // Special commands (:quit, :ir, etc.) should also fail in Phase 3+ stub.
    let result = session.eval_line(":quit");
    assert!(result.is_err());
}

#[test]
fn test_eval_line_multiline_input() {
    let mut session = ReplSession::new();
    let result = session.eval_line("function foo() {\n  return 42;\n}");
    assert!(result.is_err());
}

#[test]
fn test_run_repl_returns_ok() {
    // The stub run_repl should return Ok, even though it just prints a message.
    let result = run_repl();
    assert!(result.is_ok());
}

#[test]
fn test_multiple_eval_lines() {
    let mut session = ReplSession::new();
    // Session should remain usable after errors.
    let _ = session.eval_line("let x = 1;");
    let _ = session.eval_line("x + 1");
    let result = session.eval_line("console.log(x)");
    assert!(result.is_err());
}

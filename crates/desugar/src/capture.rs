//! Free-variable scanner for closure capture analysis.
//!
//! Walks a function body's AST to collect identifier references,
//! then determines which ones need to be captured from parent scopes.
//! Also detects which identifiers are assigned to (mutated) within the
//! body, enabling the JsBox optimization for captured+mutated variables.

use std::collections::HashSet;

use oxc_ast::ast::{
    ArrayExpressionElement, Expression, FunctionBody, ObjectPropertyKind, Statement,
};

/// How a captured variable should be stored in the closure environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureKind {
    /// Captured but never mutated — copy the value into the env slot.
    ByValue,
    /// Captured AND mutated — allocate a JsBox, store the pointer in the env slot.
    /// All reads/writes go through BoxLoad/BoxStore so mutations are visible
    /// across all closures sharing the same JsBox.
    ByBox,
}

/// Collect all identifier names referenced in a function body.
///
/// This is a shallow scan: it does NOT descend into nested function
/// declarations or expressions (those have their own capture scopes).
pub fn collect_free_identifiers<'a>(body: &'a FunctionBody<'a>) -> HashSet<String> {
    let mut names = HashSet::new();
    for stmt in &body.statements {
        collect_from_statement(stmt, &mut names);
    }
    names
}

/// Collect all identifier names that are assigned to within a function body.
///
/// Scans assignment expressions (`x = ...`, `x += ...`) and update
/// expressions (`x++`, `--x`) to find names that are mutated. This scan
/// descends into nested function bodies because a nested closure mutating
/// a variable also requires the parent to box it.
pub fn collect_mutated_identifiers<'a>(body: &'a FunctionBody<'a>) -> HashSet<String> {
    let mut mutated = HashSet::new();
    for stmt in &body.statements {
        collect_mutations_from_statement(stmt, &mut mutated);
    }
    mutated
}

/// Collect mutated identifiers from a slice of statements (for nested bodies).
fn collect_mutations_from_body_statements<'a>(
    stmts: &'a [Statement<'a>],
    mutated: &mut HashSet<String>,
) {
    for stmt in stmts {
        collect_mutations_from_statement(stmt, mutated);
    }
}

/// Walk a statement to find identifier mutations.
fn collect_mutations_from_statement<'a>(stmt: &'a Statement<'a>, mutated: &mut HashSet<String>) {
    match stmt {
        Statement::VariableDeclaration(decl) => {
            for d in &decl.declarations {
                if let Some(init) = &d.init {
                    collect_mutations_from_expression(init, mutated);
                }
            }
        }
        Statement::ExpressionStatement(expr) => {
            collect_mutations_from_expression(&expr.expression, mutated);
        }
        Statement::ReturnStatement(ret) => {
            if let Some(arg) = &ret.argument {
                collect_mutations_from_expression(arg, mutated);
            }
        }
        Statement::IfStatement(if_stmt) => {
            collect_mutations_from_expression(&if_stmt.test, mutated);
            collect_mutations_from_statement(&if_stmt.consequent, mutated);
            if let Some(alt) = &if_stmt.alternate {
                collect_mutations_from_statement(alt, mutated);
            }
        }
        Statement::BlockStatement(block) => {
            for s in &block.body {
                collect_mutations_from_statement(s, mutated);
            }
        }
        Statement::WhileStatement(w) => {
            collect_mutations_from_expression(&w.test, mutated);
            collect_mutations_from_statement(&w.body, mutated);
        }
        Statement::ForStatement(f) => {
            if let Some(init) = &f.init
                && let Some(expr) = init.as_expression()
            {
                collect_mutations_from_expression(expr, mutated);
            }
            if let Some(test) = &f.test {
                collect_mutations_from_expression(test, mutated);
            }
            if let Some(update) = &f.update {
                collect_mutations_from_expression(update, mutated);
            }
            collect_mutations_from_statement(&f.body, mutated);
        }
        Statement::ThrowStatement(t) => {
            collect_mutations_from_expression(&t.argument, mutated);
        }
        Statement::TryStatement(try_stmt) => {
            for s in &try_stmt.block.body {
                collect_mutations_from_statement(s, mutated);
            }
            if let Some(handler) = &try_stmt.handler {
                for s in &handler.body.body {
                    collect_mutations_from_statement(s, mutated);
                }
            }
            if let Some(finalizer) = &try_stmt.finalizer {
                for s in &finalizer.body {
                    collect_mutations_from_statement(s, mutated);
                }
            }
        }
        Statement::SwitchStatement(sw) => {
            collect_mutations_from_expression(&sw.discriminant, mutated);
            for case in &sw.cases {
                if let Some(test) = &case.test {
                    collect_mutations_from_expression(test, mutated);
                }
                for s in &case.consequent {
                    collect_mutations_from_statement(s, mutated);
                }
            }
        }
        Statement::ForInStatement(fi) => {
            collect_mutations_from_expression(&fi.right, mutated);
            collect_mutations_from_statement(&fi.body, mutated);
        }
        Statement::ForOfStatement(fo) => {
            collect_mutations_from_expression(&fo.right, mutated);
            collect_mutations_from_statement(&fo.body, mutated);
        }
        Statement::LabeledStatement(l) => {
            collect_mutations_from_statement(&l.body, mutated);
        }
        // Descend into nested functions to find mutations to parent variables
        Statement::FunctionDeclaration(func) => {
            if let Some(body) = &func.body {
                collect_mutations_from_body_statements(&body.statements, mutated);
            }
        }
        _ => {}
    }
}

/// Walk an expression to find identifier mutations.
fn collect_mutations_from_expression<'a>(expr: &'a Expression<'a>, mutated: &mut HashSet<String>) {
    match expr {
        Expression::AssignmentExpression(a) => {
            // The LHS identifier is mutated
            if let oxc_ast::ast::AssignmentTarget::AssignmentTargetIdentifier(ident) = &a.left {
                mutated.insert(ident.name.to_string());
            }
            // Walk other LHS forms for nested mutations
            match &a.left {
                oxc_ast::ast::AssignmentTarget::StaticMemberExpression(m) => {
                    collect_mutations_from_expression(&m.object, mutated);
                }
                oxc_ast::ast::AssignmentTarget::ComputedMemberExpression(m) => {
                    collect_mutations_from_expression(&m.object, mutated);
                    collect_mutations_from_expression(&m.expression, mutated);
                }
                _ => {}
            }
            collect_mutations_from_expression(&a.right, mutated);
        }
        Expression::UpdateExpression(u) => {
            if let oxc_ast::ast::SimpleAssignmentTarget::AssignmentTargetIdentifier(ident) =
                &u.argument
            {
                mutated.insert(ident.name.to_string());
            }
        }
        // Descend into nested functions to find mutations to parent variables
        Expression::ArrowFunctionExpression(arrow) => {
            collect_mutations_from_body_statements(&arrow.body.statements, mutated);
        }
        Expression::FunctionExpression(func) => {
            if let Some(body) = &func.body {
                collect_mutations_from_body_statements(&body.statements, mutated);
            }
        }
        // Walk compound expressions
        Expression::BinaryExpression(bin) => {
            collect_mutations_from_expression(&bin.left, mutated);
            collect_mutations_from_expression(&bin.right, mutated);
        }
        Expression::UnaryExpression(u) => {
            collect_mutations_from_expression(&u.argument, mutated);
        }
        Expression::CallExpression(call) => {
            collect_mutations_from_expression(&call.callee, mutated);
            for arg in &call.arguments {
                if let Some(e) = arg.as_expression() {
                    collect_mutations_from_expression(e, mutated);
                }
            }
        }
        Expression::ConditionalExpression(c) => {
            collect_mutations_from_expression(&c.test, mutated);
            collect_mutations_from_expression(&c.consequent, mutated);
            collect_mutations_from_expression(&c.alternate, mutated);
        }
        Expression::LogicalExpression(l) => {
            collect_mutations_from_expression(&l.left, mutated);
            collect_mutations_from_expression(&l.right, mutated);
        }
        Expression::SequenceExpression(s) => {
            for e in &s.expressions {
                collect_mutations_from_expression(e, mutated);
            }
        }
        Expression::ArrayExpression(arr) => {
            for elem in &arr.elements {
                if let Some(e) = elem.as_expression() {
                    collect_mutations_from_expression(e, mutated);
                }
            }
        }
        Expression::ObjectExpression(obj) => {
            for prop in &obj.properties {
                if let ObjectPropertyKind::ObjectProperty(p) = prop {
                    collect_mutations_from_expression(&p.value, mutated);
                }
            }
        }
        Expression::TemplateLiteral(t) => {
            for e in &t.expressions {
                collect_mutations_from_expression(e, mutated);
            }
        }
        Expression::NewExpression(n) => {
            collect_mutations_from_expression(&n.callee, mutated);
            for arg in &n.arguments {
                if let Some(e) = arg.as_expression() {
                    collect_mutations_from_expression(e, mutated);
                }
            }
        }
        Expression::ParenthesizedExpression(p) => {
            collect_mutations_from_expression(&p.expression, mutated);
        }
        Expression::StaticMemberExpression(m) => {
            collect_mutations_from_expression(&m.object, mutated);
        }
        Expression::ComputedMemberExpression(m) => {
            collect_mutations_from_expression(&m.object, mutated);
            collect_mutations_from_expression(&m.expression, mutated);
        }
        Expression::AwaitExpression(a) => {
            collect_mutations_from_expression(&a.argument, mutated);
        }
        Expression::YieldExpression(y) => {
            if let Some(arg) = &y.argument {
                collect_mutations_from_expression(arg, mutated);
            }
        }
        Expression::ChainExpression(chain) => match &chain.expression {
            oxc_ast::ast::ChainElement::CallExpression(call) => {
                collect_mutations_from_expression(&call.callee, mutated);
                for arg in &call.arguments {
                    if let Some(e) = arg.as_expression() {
                        collect_mutations_from_expression(e, mutated);
                    }
                }
            }
            oxc_ast::ast::ChainElement::StaticMemberExpression(m) => {
                collect_mutations_from_expression(&m.object, mutated);
            }
            oxc_ast::ast::ChainElement::ComputedMemberExpression(m) => {
                collect_mutations_from_expression(&m.object, mutated);
                collect_mutations_from_expression(&m.expression, mutated);
            }
            _ => {}
        },
        _ => {}
    }
}

fn collect_from_statement<'a>(stmt: &'a Statement<'a>, names: &mut HashSet<String>) {
    match stmt {
        Statement::VariableDeclaration(decl) => {
            for d in &decl.declarations {
                if let Some(init) = &d.init {
                    collect_from_expression(init, names);
                }
            }
        }
        Statement::ExpressionStatement(expr) => {
            collect_from_expression(&expr.expression, names);
        }
        Statement::ReturnStatement(ret) => {
            if let Some(arg) = &ret.argument {
                collect_from_expression(arg, names);
            }
        }
        Statement::IfStatement(if_stmt) => {
            collect_from_expression(&if_stmt.test, names);
            collect_from_statement(&if_stmt.consequent, names);
            if let Some(alt) = &if_stmt.alternate {
                collect_from_statement(alt, names);
            }
        }
        Statement::BlockStatement(block) => {
            for s in &block.body {
                collect_from_statement(s, names);
            }
        }
        Statement::WhileStatement(w) => {
            collect_from_expression(&w.test, names);
            collect_from_statement(&w.body, names);
        }
        Statement::ForStatement(f) => {
            if let Some(init) = &f.init
                && let Some(expr) = init.as_expression()
            {
                collect_from_expression(expr, names);
            }
            if let Some(test) = &f.test {
                collect_from_expression(test, names);
            }
            if let Some(update) = &f.update {
                collect_from_expression(update, names);
            }
            collect_from_statement(&f.body, names);
        }
        Statement::ThrowStatement(t) => {
            collect_from_expression(&t.argument, names);
        }
        Statement::TryStatement(try_stmt) => {
            for s in &try_stmt.block.body {
                collect_from_statement(s, names);
            }
            if let Some(handler) = &try_stmt.handler {
                for s in &handler.body.body {
                    collect_from_statement(s, names);
                }
            }
            if let Some(finalizer) = &try_stmt.finalizer {
                for s in &finalizer.body {
                    collect_from_statement(s, names);
                }
            }
        }
        Statement::SwitchStatement(sw) => {
            collect_from_expression(&sw.discriminant, names);
            for case in &sw.cases {
                if let Some(test) = &case.test {
                    collect_from_expression(test, names);
                }
                for s in &case.consequent {
                    collect_from_statement(s, names);
                }
            }
        }
        Statement::ForInStatement(fi) => {
            collect_from_expression(&fi.right, names);
            collect_from_statement(&fi.body, names);
        }
        Statement::ForOfStatement(fo) => {
            collect_from_expression(&fo.right, names);
            collect_from_statement(&fo.body, names);
        }
        Statement::LabeledStatement(l) => {
            collect_from_statement(&l.body, names);
        }
        // Function declarations create new scopes, but we still need to
        // scan their bodies for identifiers they reference — those may need
        // to be captured transitively through us from a parent scope.
        Statement::FunctionDeclaration(func) => {
            if let Some(body) = &func.body {
                collect_from_body_statements(&body.statements, names);
            }
        }
        _ => {}
    }
}

fn collect_from_expression<'a>(expr: &'a Expression<'a>, names: &mut HashSet<String>) {
    match expr {
        Expression::Identifier(ident) => {
            names.insert(ident.name.to_string());
        }
        Expression::BinaryExpression(bin) => {
            collect_from_expression(&bin.left, names);
            collect_from_expression(&bin.right, names);
        }
        Expression::UnaryExpression(u) => {
            collect_from_expression(&u.argument, names);
        }
        Expression::UpdateExpression(u) => {
            if let oxc_ast::ast::SimpleAssignmentTarget::AssignmentTargetIdentifier(ident) =
                &u.argument
            {
                names.insert(ident.name.to_string());
            }
        }
        Expression::CallExpression(call) => {
            collect_from_expression(&call.callee, names);
            for arg in &call.arguments {
                if let Some(e) = arg.as_expression() {
                    collect_from_expression(e, names);
                }
            }
        }
        Expression::StaticMemberExpression(m) => {
            collect_from_expression(&m.object, names);
        }
        Expression::ComputedMemberExpression(m) => {
            collect_from_expression(&m.object, names);
            collect_from_expression(&m.expression, names);
        }
        Expression::AssignmentExpression(a) => {
            collect_from_expression(&a.right, names);
            match &a.left {
                oxc_ast::ast::AssignmentTarget::AssignmentTargetIdentifier(ident) => {
                    names.insert(ident.name.to_string());
                }
                oxc_ast::ast::AssignmentTarget::StaticMemberExpression(m) => {
                    collect_from_expression(&m.object, names);
                }
                oxc_ast::ast::AssignmentTarget::ComputedMemberExpression(m) => {
                    collect_from_expression(&m.object, names);
                    collect_from_expression(&m.expression, names);
                }
                _ => {}
            }
        }
        Expression::ConditionalExpression(c) => {
            collect_from_expression(&c.test, names);
            collect_from_expression(&c.consequent, names);
            collect_from_expression(&c.alternate, names);
        }
        Expression::LogicalExpression(l) => {
            collect_from_expression(&l.left, names);
            collect_from_expression(&l.right, names);
        }
        Expression::ArrayExpression(arr) => {
            for elem in &arr.elements {
                match elem {
                    ArrayExpressionElement::Elision(_) => {}
                    _ => {
                        if let Some(e) = elem.as_expression() {
                            collect_from_expression(e, names);
                        }
                    }
                }
            }
        }
        Expression::ObjectExpression(obj) => {
            for prop in &obj.properties {
                if let ObjectPropertyKind::ObjectProperty(p) = prop {
                    if let Some(expr) = p.key.as_expression() {
                        collect_from_expression(expr, names);
                    }
                    collect_from_expression(&p.value, names);
                }
            }
        }
        Expression::TemplateLiteral(t) => {
            for e in &t.expressions {
                collect_from_expression(e, names);
            }
        }
        Expression::NewExpression(n) => {
            collect_from_expression(&n.callee, names);
            for arg in &n.arguments {
                if let Some(e) = arg.as_expression() {
                    collect_from_expression(e, names);
                }
            }
        }
        Expression::SequenceExpression(s) => {
            for e in &s.expressions {
                collect_from_expression(e, names);
            }
        }
        Expression::ParenthesizedExpression(p) => {
            collect_from_expression(&p.expression, names);
        }
        Expression::AwaitExpression(a) => {
            collect_from_expression(&a.argument, names);
        }
        Expression::YieldExpression(y) => {
            if let Some(arg) = &y.argument {
                collect_from_expression(arg, names);
            }
        }
        // Arrow and function expressions create new scopes — but we still
        // need to scan them for references to OUR parent scope variables.
        // The nested function will capture from us, and we capture from parent.
        Expression::ArrowFunctionExpression(arrow) => {
            collect_from_body_statements(&arrow.body.statements, names);
        }
        Expression::FunctionExpression(func) => {
            if let Some(body) = &func.body {
                collect_from_body_statements(&body.statements, names);
            }
        }
        Expression::ChainExpression(chain) => match &chain.expression {
            oxc_ast::ast::ChainElement::CallExpression(call) => {
                collect_from_expression(&call.callee, names);
                for arg in &call.arguments {
                    if let Some(e) = arg.as_expression() {
                        collect_from_expression(e, names);
                    }
                }
            }
            oxc_ast::ast::ChainElement::StaticMemberExpression(m) => {
                collect_from_expression(&m.object, names);
            }
            oxc_ast::ast::ChainElement::ComputedMemberExpression(m) => {
                collect_from_expression(&m.object, names);
                collect_from_expression(&m.expression, names);
            }
            _ => {}
        },
        _ => {}
    }
}

fn collect_from_body_statements<'a>(stmts: &'a [Statement<'a>], names: &mut HashSet<String>) {
    for stmt in stmts {
        collect_from_statement(stmt, names);
    }
}

/// How the `arguments` identifier is used within a function body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgumentsUsage {
    /// `arguments` is never referenced — skip `CreateArguments`.
    Unused,
    /// `arguments` is referenced (e.g., `arguments[0]`, `arguments.length`).
    Used,
}

/// Determine whether a function needs mapped arguments (Tier 4).
///
/// Mapped arguments apply ONLY when ALL of:
/// - Function is in sloppy mode (not strict)
/// - Function has NO rest parameter (`...args`)
/// - Function has NO destructuring parameters
/// - Function has NO default parameter values
/// - Function uses `arguments` (not Unused)
///
/// When mapped, `arguments[i]` and the corresponding named parameter share
/// the same storage so that mutations to one are reflected in the other.
pub fn needs_mapped_arguments(
    is_strict: bool,
    has_rest: bool,
    params: &[oxc_ast::ast::FormalParameter<'_>],
    usage: ArgumentsUsage,
) -> bool {
    if usage == ArgumentsUsage::Unused || is_strict || has_rest {
        return false;
    }

    // Check for destructuring or default parameters
    for param in params {
        // Default parameter
        if param.initializer.is_some() {
            return false;
        }
        // Destructuring parameter (not a simple identifier)
        if !matches!(
            &param.pattern,
            oxc_ast::ast::BindingPattern::BindingIdentifier(_)
        ) {
            return false;
        }
    }

    true
}

/// Scan a function body for references to the `arguments` identifier.
///
/// Does NOT descend into nested function declarations/expressions or arrow
/// functions, since those have their own `arguments` binding (or inherit
/// from the enclosing non-arrow function).
pub fn scan_arguments_usage(body: &FunctionBody<'_>) -> ArgumentsUsage {
    for stmt in &body.statements {
        if scan_stmt_for_arguments(stmt) {
            return ArgumentsUsage::Used;
        }
    }
    ArgumentsUsage::Unused
}

/// Scan a statement for references to the `arguments` identifier.
///
/// Returns `true` as soon as an `arguments` reference is found (short-circuit).
/// Does NOT descend into nested function or arrow bodies.
fn scan_stmt_for_arguments(stmt: &Statement<'_>) -> bool {
    match stmt {
        Statement::VariableDeclaration(decl) => {
            for d in &decl.declarations {
                if let Some(init) = &d.init
                    && scan_expr_for_arguments(init)
                {
                    return true;
                }
            }
            false
        }
        Statement::ExpressionStatement(expr) => scan_expr_for_arguments(&expr.expression),
        Statement::ReturnStatement(ret) => {
            if let Some(arg) = &ret.argument {
                return scan_expr_for_arguments(arg);
            }
            false
        }
        Statement::IfStatement(if_stmt) => {
            scan_expr_for_arguments(&if_stmt.test)
                || scan_stmt_for_arguments(&if_stmt.consequent)
                || if_stmt
                    .alternate
                    .as_ref()
                    .is_some_and(|alt| scan_stmt_for_arguments(alt))
        }
        Statement::BlockStatement(block) => block.body.iter().any(|s| scan_stmt_for_arguments(s)),
        Statement::WhileStatement(w) => {
            scan_expr_for_arguments(&w.test) || scan_stmt_for_arguments(&w.body)
        }
        Statement::DoWhileStatement(dw) => {
            scan_stmt_for_arguments(&dw.body) || scan_expr_for_arguments(&dw.test)
        }
        Statement::ForStatement(f) => {
            if let Some(init) = &f.init
                && let Some(expr) = init.as_expression()
                && scan_expr_for_arguments(expr)
            {
                return true;
            }
            if let Some(test) = &f.test
                && scan_expr_for_arguments(test)
            {
                return true;
            }
            if let Some(update) = &f.update
                && scan_expr_for_arguments(update)
            {
                return true;
            }
            scan_stmt_for_arguments(&f.body)
        }
        Statement::ThrowStatement(t) => scan_expr_for_arguments(&t.argument),
        Statement::TryStatement(try_stmt) => {
            if try_stmt
                .block
                .body
                .iter()
                .any(|s| scan_stmt_for_arguments(s))
            {
                return true;
            }
            if let Some(handler) = &try_stmt.handler
                && handler.body.body.iter().any(|s| scan_stmt_for_arguments(s))
            {
                return true;
            }
            if let Some(finalizer) = &try_stmt.finalizer
                && finalizer.body.iter().any(|s| scan_stmt_for_arguments(s))
            {
                return true;
            }
            false
        }
        Statement::SwitchStatement(sw) => {
            if scan_expr_for_arguments(&sw.discriminant) {
                return true;
            }
            for case in &sw.cases {
                if let Some(test) = &case.test
                    && scan_expr_for_arguments(test)
                {
                    return true;
                }
                if case.consequent.iter().any(|s| scan_stmt_for_arguments(s)) {
                    return true;
                }
            }
            false
        }
        Statement::ForInStatement(fi) => {
            scan_expr_for_arguments(&fi.right) || scan_stmt_for_arguments(&fi.body)
        }
        Statement::ForOfStatement(fo) => {
            scan_expr_for_arguments(&fo.right) || scan_stmt_for_arguments(&fo.body)
        }
        Statement::LabeledStatement(l) => scan_stmt_for_arguments(&l.body),
        // Do NOT descend into nested function bodies — they have their own `arguments`.
        Statement::FunctionDeclaration(_) => false,
        _ => false,
    }
}

/// Scan an expression for references to the `arguments` identifier.
///
/// Returns `true` as soon as an `arguments` reference is found (short-circuit).
/// Does NOT descend into nested function or arrow expression bodies.
fn scan_expr_for_arguments(expr: &Expression<'_>) -> bool {
    match expr {
        Expression::Identifier(ident) => ident.name == "arguments",
        Expression::BinaryExpression(bin) => {
            scan_expr_for_arguments(&bin.left) || scan_expr_for_arguments(&bin.right)
        }
        Expression::UnaryExpression(u) => scan_expr_for_arguments(&u.argument),
        Expression::UpdateExpression(u) => {
            if let oxc_ast::ast::SimpleAssignmentTarget::AssignmentTargetIdentifier(ident) =
                &u.argument
                && ident.name == "arguments"
            {
                return true;
            }
            false
        }
        Expression::CallExpression(call) => {
            if scan_expr_for_arguments(&call.callee) {
                return true;
            }
            for arg in &call.arguments {
                if let Some(e) = arg.as_expression()
                    && scan_expr_for_arguments(e)
                {
                    return true;
                }
            }
            false
        }
        Expression::StaticMemberExpression(m) => scan_expr_for_arguments(&m.object),
        Expression::ComputedMemberExpression(m) => {
            scan_expr_for_arguments(&m.object) || scan_expr_for_arguments(&m.expression)
        }
        Expression::AssignmentExpression(a) => {
            if let oxc_ast::ast::AssignmentTarget::AssignmentTargetIdentifier(ident) = &a.left
                && ident.name == "arguments"
            {
                return true;
            }
            match &a.left {
                oxc_ast::ast::AssignmentTarget::StaticMemberExpression(m)
                    if scan_expr_for_arguments(&m.object) =>
                {
                    return true;
                }
                oxc_ast::ast::AssignmentTarget::ComputedMemberExpression(m)
                    if scan_expr_for_arguments(&m.object)
                        || scan_expr_for_arguments(&m.expression) =>
                {
                    return true;
                }
                _ => {}
            }
            scan_expr_for_arguments(&a.right)
        }
        Expression::ConditionalExpression(c) => {
            scan_expr_for_arguments(&c.test)
                || scan_expr_for_arguments(&c.consequent)
                || scan_expr_for_arguments(&c.alternate)
        }
        Expression::LogicalExpression(l) => {
            scan_expr_for_arguments(&l.left) || scan_expr_for_arguments(&l.right)
        }
        Expression::ArrayExpression(arr) => {
            for elem in &arr.elements {
                if let Some(e) = elem.as_expression()
                    && scan_expr_for_arguments(e)
                {
                    return true;
                }
            }
            false
        }
        Expression::ObjectExpression(obj) => {
            for prop in &obj.properties {
                if let ObjectPropertyKind::ObjectProperty(p) = prop {
                    if let Some(expr) = p.key.as_expression()
                        && scan_expr_for_arguments(expr)
                    {
                        return true;
                    }
                    if scan_expr_for_arguments(&p.value) {
                        return true;
                    }
                }
            }
            false
        }
        Expression::TemplateLiteral(t) => {
            for e in &t.expressions {
                if scan_expr_for_arguments(e) {
                    return true;
                }
            }
            false
        }
        Expression::NewExpression(n) => {
            if scan_expr_for_arguments(&n.callee) {
                return true;
            }
            for arg in &n.arguments {
                if let Some(e) = arg.as_expression()
                    && scan_expr_for_arguments(e)
                {
                    return true;
                }
            }
            false
        }
        Expression::SequenceExpression(s) => {
            s.expressions.iter().any(|e| scan_expr_for_arguments(e))
        }
        Expression::ParenthesizedExpression(p) => scan_expr_for_arguments(&p.expression),
        Expression::AwaitExpression(a) => scan_expr_for_arguments(&a.argument),
        Expression::YieldExpression(y) => {
            if let Some(arg) = &y.argument {
                return scan_expr_for_arguments(arg);
            }
            false
        }
        Expression::ChainExpression(chain) => match &chain.expression {
            oxc_ast::ast::ChainElement::CallExpression(call) => {
                if scan_expr_for_arguments(&call.callee) {
                    return true;
                }
                for arg in &call.arguments {
                    if let Some(e) = arg.as_expression()
                        && scan_expr_for_arguments(e)
                    {
                        return true;
                    }
                }
                false
            }
            oxc_ast::ast::ChainElement::StaticMemberExpression(m) => {
                scan_expr_for_arguments(&m.object)
            }
            oxc_ast::ast::ChainElement::ComputedMemberExpression(m) => {
                scan_expr_for_arguments(&m.object) || scan_expr_for_arguments(&m.expression)
            }
            _ => false,
        },
        // Do NOT descend into nested function/arrow expression bodies.
        Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_) => false,
        _ => false,
    }
}

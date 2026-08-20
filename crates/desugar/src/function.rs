use std::collections::{HashMap, HashSet};

use ir::{IrType, ValueId};
use oxc_ast::ast::{
    ArrowFunctionExpression, BindingPattern, Class, ClassElement, Expression, FormalParameter,
    Function, FunctionBody, MethodDefinitionKind, PropertyKey, Statement, VariableDeclarationKind,
};

use crate::capture::{CaptureKind, collect_free_identifiers, collect_mutated_identifiers};
use crate::lowerer::IrLowerer;
use crate::scope::{ResolveResult, ScopeKind};

/// Check whether a function body contains a direct `eval()` call.
///
/// Scans statements and expressions for an unqualified `eval(...)` call.
/// Stops at function boundaries (nested functions are not included).
/// Used to determine if a function needs a dynamic `EscEnvironment`.
fn body_has_direct_eval(body: Option<&FunctionBody<'_>>) -> bool {
    if let Some(b) = body {
        for stmt in &b.statements {
            if stmt_has_direct_eval(stmt) {
                return true;
            }
        }
    }
    false
}

/// Check whether a statement contains a direct `eval()` call (non-recursive into functions).
fn stmt_has_direct_eval(stmt: &Statement<'_>) -> bool {
    use oxc_ast::ast::*;
    match stmt {
        Statement::ExpressionStatement(expr) => expr_has_direct_eval(&expr.expression),
        Statement::VariableDeclaration(decl) => decl.declarations.iter().any(|d| {
            d.init
                .as_ref()
                .is_some_and(|init| expr_has_direct_eval(init))
        }),
        Statement::ReturnStatement(ret) => ret
            .argument
            .as_ref()
            .is_some_and(|arg| expr_has_direct_eval(arg)),
        Statement::IfStatement(if_stmt) => {
            expr_has_direct_eval(&if_stmt.test)
                || stmt_has_direct_eval(&if_stmt.consequent)
                || if_stmt
                    .alternate
                    .as_ref()
                    .is_some_and(|alt| stmt_has_direct_eval(alt))
        }
        Statement::BlockStatement(block) => block.body.iter().any(|s| stmt_has_direct_eval(s)),
        Statement::ForStatement(f) => {
            let init_eval = match &f.init {
                Some(init) => {
                    if let ForStatementInit::VariableDeclaration(d) = init {
                        d.declarations
                            .iter()
                            .any(|d| d.init.as_ref().is_some_and(|e| expr_has_direct_eval(e)))
                    } else if let Some(e) = init.as_expression() {
                        expr_has_direct_eval(e)
                    } else {
                        false
                    }
                }
                None => false,
            };
            init_eval
                || f.test.as_ref().is_some_and(|t| expr_has_direct_eval(t))
                || f.update.as_ref().is_some_and(|u| expr_has_direct_eval(u))
                || stmt_has_direct_eval(&f.body)
        }
        Statement::WhileStatement(w) => {
            expr_has_direct_eval(&w.test) || stmt_has_direct_eval(&w.body)
        }
        Statement::DoWhileStatement(dw) => {
            expr_has_direct_eval(&dw.test) || stmt_has_direct_eval(&dw.body)
        }
        Statement::TryStatement(t) => {
            t.block.body.iter().any(|s| stmt_has_direct_eval(s))
                || t.handler
                    .as_ref()
                    .is_some_and(|h| h.body.body.iter().any(|s| stmt_has_direct_eval(s)))
                || t.finalizer
                    .as_ref()
                    .is_some_and(|f| f.body.iter().any(|s| stmt_has_direct_eval(s)))
        }
        Statement::SwitchStatement(sw) => {
            expr_has_direct_eval(&sw.discriminant)
                || sw.cases.iter().any(|c| {
                    c.test.as_ref().is_some_and(|t| expr_has_direct_eval(t))
                        || c.consequent.iter().any(|s| stmt_has_direct_eval(s))
                })
        }
        Statement::ThrowStatement(t) => expr_has_direct_eval(&t.argument),
        Statement::LabeledStatement(l) => stmt_has_direct_eval(&l.body),
        Statement::WithStatement(w) => {
            expr_has_direct_eval(&w.object) || stmt_has_direct_eval(&w.body)
        }
        Statement::ForInStatement(f) => {
            expr_has_direct_eval(&f.right) || stmt_has_direct_eval(&f.body)
        }
        Statement::ForOfStatement(f) => {
            expr_has_direct_eval(&f.right) || stmt_has_direct_eval(&f.body)
        }
        // Do NOT recurse into function/class declarations — they have their own scope
        _ => false,
    }
}

/// Check whether an expression contains a direct `eval()` call (non-recursive into functions).
fn expr_has_direct_eval(expr: &Expression<'_>) -> bool {
    use oxc_ast::ast::*;
    match expr {
        Expression::CallExpression(call) => {
            // Direct eval: `eval(...)` where callee is the unqualified identifier `eval`
            if let Expression::Identifier(ident) = &call.callee
                && ident.name.as_str() == "eval"
            {
                return true;
            }
            // Also check callee and arguments for eval
            expr_has_direct_eval(&call.callee)
                || call
                    .arguments
                    .iter()
                    .any(|a| a.as_expression().is_some_and(|e| expr_has_direct_eval(e)))
        }
        Expression::AssignmentExpression(a) => expr_has_direct_eval(&a.right),
        Expression::SequenceExpression(seq) => {
            seq.expressions.iter().any(|e| expr_has_direct_eval(e))
        }
        Expression::ConditionalExpression(c) => {
            expr_has_direct_eval(&c.test)
                || expr_has_direct_eval(&c.consequent)
                || expr_has_direct_eval(&c.alternate)
        }
        Expression::BinaryExpression(b) => {
            expr_has_direct_eval(&b.left) || expr_has_direct_eval(&b.right)
        }
        Expression::LogicalExpression(l) => {
            expr_has_direct_eval(&l.left) || expr_has_direct_eval(&l.right)
        }
        Expression::UnaryExpression(u) => expr_has_direct_eval(&u.argument),
        Expression::UpdateExpression(_) => false, // UpdateExpression argument is an lvalue, not eval
        Expression::ArrayExpression(a) => a.elements.iter().any(|el| match el {
            ArrayExpressionElement::SpreadElement(s) => expr_has_direct_eval(&s.argument),
            ArrayExpressionElement::Elision(_) => false,
            _ => el.as_expression().is_some_and(|e| expr_has_direct_eval(e)),
        }),
        Expression::ObjectExpression(o) => o.properties.iter().any(|p| match p {
            ObjectPropertyKind::ObjectProperty(prop) => expr_has_direct_eval(&prop.value),
            ObjectPropertyKind::SpreadProperty(s) => expr_has_direct_eval(&s.argument),
        }),
        Expression::TemplateLiteral(t) => t.expressions.iter().any(|e| expr_has_direct_eval(e)),
        Expression::TaggedTemplateExpression(t) => expr_has_direct_eval(&t.tag),
        Expression::ComputedMemberExpression(m) => {
            expr_has_direct_eval(&m.object) || expr_has_direct_eval(&m.expression)
        }
        Expression::StaticMemberExpression(m) => expr_has_direct_eval(&m.object),
        Expression::NewExpression(n) => {
            expr_has_direct_eval(&n.callee)
                || n.arguments
                    .iter()
                    .any(|a| a.as_expression().is_some_and(|e| expr_has_direct_eval(e)))
        }
        Expression::AwaitExpression(a) => expr_has_direct_eval(&a.argument),
        Expression::YieldExpression(y) => {
            y.argument.as_ref().is_some_and(|a| expr_has_direct_eval(a))
        }
        Expression::ParenthesizedExpression(p) => expr_has_direct_eval(&p.expression),
        // Do NOT recurse into arrow/function expressions — they have their own scope
        _ => false,
    }
}

/// Check whether a function body contains a `"use strict"` directive prologue.
fn body_has_use_strict(body: Option<&FunctionBody<'_>>) -> bool {
    if let Some(b) = body {
        for d in &b.directives {
            if d.directive.as_str() == "use strict" {
                return true;
            }
        }
    }
    false
}

/// Check whether a function body contains `var` declarations that shadow
/// parameter names. When this is true, the body needs its own scope so that
/// default parameter expressions see the parameter values, not the body `var`s.
///
/// Only scans top-level `var` statements in the body (does not descend into
/// nested functions, since those have their own scope).
fn has_var_shadowing_params(params: &[FormalParameter<'_>], body: &FunctionBody<'_>) -> bool {
    // Collect parameter names
    let mut param_names = HashSet::new();
    for p in params {
        if let BindingPattern::BindingIdentifier(ident) = &p.pattern {
            param_names.insert(ident.name.as_str());
        }
    }
    if param_names.is_empty() {
        return false;
    }

    // Walk top-level body statements for `var` declarations
    for stmt in &body.statements {
        if let Statement::VariableDeclaration(decl) = stmt
            && decl.kind == VariableDeclarationKind::Var
        {
            for declarator in &decl.declarations {
                let names = collect_var_binding_names(&declarator.id);
                for name in &names {
                    if param_names.contains(name.as_str()) {
                        return true;
                    }
                }
            }
        }
        // Also check for-statement init clauses at top level
        if let Statement::ForStatement(for_stmt) = stmt
            && let Some(oxc_ast::ast::ForStatementInit::VariableDeclaration(decl)) = &for_stmt.init
            && decl.kind == VariableDeclarationKind::Var
        {
            for declarator in &decl.declarations {
                let names = collect_var_binding_names(&declarator.id);
                for name in &names {
                    if param_names.contains(name.as_str()) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Collect all binding names from a binding pattern (for var shadowing detection).
fn collect_var_binding_names(pattern: &BindingPattern<'_>) -> Vec<String> {
    let mut names = Vec::new();
    collect_var_binding_names_inner(pattern, &mut names);
    names
}

/// Recursive helper for [`collect_var_binding_names`].
fn collect_var_binding_names_inner(pattern: &BindingPattern<'_>, names: &mut Vec<String>) {
    match pattern {
        BindingPattern::BindingIdentifier(ident) => {
            names.push(ident.name.as_str().to_string());
        }
        BindingPattern::ObjectPattern(obj) => {
            for prop in &obj.properties {
                collect_var_binding_names_inner(&prop.value, names);
            }
            if let Some(rest) = &obj.rest {
                collect_var_binding_names_inner(&rest.argument, names);
            }
        }
        BindingPattern::ArrayPattern(arr) => {
            for elem in arr.elements.iter().flatten() {
                collect_var_binding_names_inner(elem, names);
            }
            if let Some(rest) = &arr.rest {
                collect_var_binding_names_inner(&rest.argument, names);
            }
        }
        BindingPattern::AssignmentPattern(assign) => {
            collect_var_binding_names_inner(&assign.left, names);
        }
    }
}

/// Collect all locally declared variable names in a function body.
///
/// Includes parameter names, `var`/`let`/`const` declarations. Does NOT
/// descend into nested function/arrow declarations (they have their own scope).
/// Used to populate the `EscEnvironment` slot map for poisoned functions.
fn collect_function_local_names(
    params: &[FormalParameter<'_>],
    body: Option<&FunctionBody<'_>>,
) -> Vec<String> {
    let mut names = Vec::new();

    // Collect parameter names
    for p in params {
        let param_names = collect_var_binding_names(&p.pattern);
        names.extend(param_names);
    }

    // Collect body variable names
    if let Some(body) = body {
        collect_body_var_names(&body.statements, &mut names);
    }

    // Deduplicate while preserving order
    let mut seen = HashSet::new();
    names.retain(|n| seen.insert(n.clone()));
    names
}

/// Recursively collect variable names from a statement list (non-recursive into functions).
fn collect_body_var_names(stmts: &[Statement<'_>], names: &mut Vec<String>) {
    for stmt in stmts {
        collect_stmt_var_names(stmt, names);
    }
}

/// Collect variable names from a single statement (non-recursive into functions).
fn collect_stmt_var_names(stmt: &Statement<'_>, names: &mut Vec<String>) {
    use oxc_ast::ast::*;
    match stmt {
        Statement::VariableDeclaration(decl) => {
            for declarator in &decl.declarations {
                names.extend(collect_var_binding_names(&declarator.id));
            }
        }
        Statement::BlockStatement(block) => {
            collect_body_var_names(&block.body, names);
        }
        Statement::IfStatement(if_stmt) => {
            collect_stmt_var_names(&if_stmt.consequent, names);
            if let Some(ref alt) = if_stmt.alternate {
                collect_stmt_var_names(alt, names);
            }
        }
        Statement::ForStatement(f) => {
            if let Some(ForStatementInit::VariableDeclaration(d)) = &f.init {
                for declarator in &d.declarations {
                    names.extend(collect_var_binding_names(&declarator.id));
                }
            }
            collect_stmt_var_names(&f.body, names);
        }
        Statement::ForInStatement(f) => {
            if let ForStatementLeft::VariableDeclaration(d) = &f.left {
                for declarator in &d.declarations {
                    names.extend(collect_var_binding_names(&declarator.id));
                }
            }
            collect_stmt_var_names(&f.body, names);
        }
        Statement::ForOfStatement(f) => {
            if let ForStatementLeft::VariableDeclaration(d) = &f.left {
                for declarator in &d.declarations {
                    names.extend(collect_var_binding_names(&declarator.id));
                }
            }
            collect_stmt_var_names(&f.body, names);
        }
        Statement::WhileStatement(w) => {
            collect_stmt_var_names(&w.body, names);
        }
        Statement::DoWhileStatement(dw) => {
            collect_stmt_var_names(&dw.body, names);
        }
        Statement::TryStatement(t) => {
            collect_body_var_names(&t.block.body, names);
            if let Some(ref h) = t.handler {
                if let Some(ref param) = h.param {
                    names.extend(collect_var_binding_names(&param.pattern));
                }
                collect_body_var_names(&h.body.body, names);
            }
            if let Some(ref f) = t.finalizer {
                collect_body_var_names(&f.body, names);
            }
        }
        Statement::SwitchStatement(sw) => {
            for case in &sw.cases {
                collect_body_var_names(&case.consequent, names);
            }
        }
        Statement::LabeledStatement(l) => {
            collect_stmt_var_names(&l.body, names);
        }
        Statement::WithStatement(w) => {
            collect_stmt_var_names(&w.body, names);
        }
        // Do NOT recurse into function/class declarations
        _ => {}
    }
}

/// Collect free identifiers referenced from `with` statement object expressions
/// in a function body.
///
/// [`collect_free_identifiers`] has no `WithStatement` arm, so a `with(o)` over
/// an outer binding is invisible to closure capture analysis. This walker
/// mirrors its statement recursion and stops at nested function bodies (those
/// run their own capture analysis), collecting only the identifiers that the
/// with-object expression's evaluation actually references.
fn collect_with_object_free_names(body: &FunctionBody<'_>) -> HashSet<String> {
    let mut names = HashSet::new();
    for stmt in &body.statements {
        collect_with_object_names_from_statement(stmt, &mut names);
    }
    names
}

/// Walk a statement, collecting identifier references from `with` object
/// expressions. Non-with statements recurse into their sub-statements so
/// `with` nodes at any depth are found; nested function bodies are not entered.
fn collect_with_object_names_from_statement(stmt: &Statement<'_>, names: &mut HashSet<String>) {
    use oxc_ast::ast::*;
    match stmt {
        Statement::WithStatement(with_stmt) => {
            collect_with_object_names_from_expression(&with_stmt.object, names);
            collect_with_object_names_from_statement(&with_stmt.body, names);
        }
        Statement::BlockStatement(block) => {
            for s in &block.body {
                collect_with_object_names_from_statement(s, names);
            }
        }
        Statement::IfStatement(if_stmt) => {
            collect_with_object_names_from_expression(&if_stmt.test, names);
            collect_with_object_names_from_statement(&if_stmt.consequent, names);
            if let Some(alt) = &if_stmt.alternate {
                collect_with_object_names_from_statement(alt, names);
            }
        }
        Statement::ForStatement(f) => {
            if let Some(init) = &f.init {
                if let ForStatementInit::VariableDeclaration(decl) = init {
                    for d in &decl.declarations {
                        if let Some(init_expr) = &d.init {
                            collect_with_object_names_from_expression(init_expr, names);
                        }
                    }
                } else if let Some(e) = init.as_expression() {
                    collect_with_object_names_from_expression(e, names);
                }
            }
            if let Some(test) = &f.test {
                collect_with_object_names_from_expression(test, names);
            }
            if let Some(update) = &f.update {
                collect_with_object_names_from_expression(update, names);
            }
            collect_with_object_names_from_statement(&f.body, names);
        }
        Statement::ForInStatement(fi) => {
            collect_with_object_names_from_expression(&fi.right, names);
            collect_with_object_names_from_statement(&fi.body, names);
        }
        Statement::ForOfStatement(fo) => {
            collect_with_object_names_from_expression(&fo.right, names);
            collect_with_object_names_from_statement(&fo.body, names);
        }
        Statement::WhileStatement(w) => {
            collect_with_object_names_from_expression(&w.test, names);
            collect_with_object_names_from_statement(&w.body, names);
        }
        Statement::DoWhileStatement(dw) => {
            collect_with_object_names_from_expression(&dw.test, names);
            collect_with_object_names_from_statement(&dw.body, names);
        }
        Statement::SwitchStatement(sw) => {
            collect_with_object_names_from_expression(&sw.discriminant, names);
            for case in &sw.cases {
                if let Some(test) = &case.test {
                    collect_with_object_names_from_expression(test, names);
                }
                for s in &case.consequent {
                    collect_with_object_names_from_statement(s, names);
                }
            }
        }
        Statement::TryStatement(t) => {
            for s in &t.block.body {
                collect_with_object_names_from_statement(s, names);
            }
            if let Some(handler) = &t.handler {
                for s in &handler.body.body {
                    collect_with_object_names_from_statement(s, names);
                }
            }
            if let Some(finalizer) = &t.finalizer {
                for s in &finalizer.body {
                    collect_with_object_names_from_statement(s, names);
                }
            }
        }
        Statement::LabeledStatement(l) => {
            collect_with_object_names_from_statement(&l.body, names);
        }
        // Stop at nested function bodies — they run their own capture analysis.
        Statement::FunctionDeclaration(_) => {}
        _ => {}
    }
}

/// Walk an expression, collecting identifier references (for `with` object
/// expressions). Does not enter nested function bodies.
fn collect_with_object_names_from_expression(expr: &Expression<'_>, names: &mut HashSet<String>) {
    use oxc_ast::ast::*;
    match expr {
        Expression::Identifier(ident) => {
            names.insert(ident.name.to_string());
        }
        Expression::BinaryExpression(bin) => {
            collect_with_object_names_from_expression(&bin.left, names);
            collect_with_object_names_from_expression(&bin.right, names);
        }
        Expression::UnaryExpression(u) => {
            collect_with_object_names_from_expression(&u.argument, names);
        }
        Expression::UpdateExpression(u) => {
            if let SimpleAssignmentTarget::AssignmentTargetIdentifier(ident) = &u.argument {
                names.insert(ident.name.to_string());
            }
        }
        Expression::CallExpression(call) => {
            collect_with_object_names_from_expression(&call.callee, names);
            for arg in &call.arguments {
                if let Some(e) = arg.as_expression() {
                    collect_with_object_names_from_expression(e, names);
                }
            }
        }
        Expression::StaticMemberExpression(m) => {
            collect_with_object_names_from_expression(&m.object, names);
        }
        Expression::ComputedMemberExpression(m) => {
            collect_with_object_names_from_expression(&m.object, names);
            collect_with_object_names_from_expression(&m.expression, names);
        }
        Expression::AssignmentExpression(a) => {
            collect_with_object_names_from_expression(&a.right, names);
            match &a.left {
                AssignmentTarget::AssignmentTargetIdentifier(ident) => {
                    names.insert(ident.name.to_string());
                }
                AssignmentTarget::StaticMemberExpression(m) => {
                    collect_with_object_names_from_expression(&m.object, names);
                }
                AssignmentTarget::ComputedMemberExpression(m) => {
                    collect_with_object_names_from_expression(&m.object, names);
                    collect_with_object_names_from_expression(&m.expression, names);
                }
                _ => {}
            }
        }
        Expression::ConditionalExpression(c) => {
            collect_with_object_names_from_expression(&c.test, names);
            collect_with_object_names_from_expression(&c.consequent, names);
            collect_with_object_names_from_expression(&c.alternate, names);
        }
        Expression::LogicalExpression(l) => {
            collect_with_object_names_from_expression(&l.left, names);
            collect_with_object_names_from_expression(&l.right, names);
        }
        Expression::ArrayExpression(arr) => {
            for elem in &arr.elements {
                if let Some(e) = elem.as_expression() {
                    collect_with_object_names_from_expression(e, names);
                }
            }
        }
        Expression::ObjectExpression(obj) => {
            for prop in &obj.properties {
                match prop {
                    ObjectPropertyKind::ObjectProperty(p) => {
                        if let Some(key_expr) = p.key.as_expression() {
                            collect_with_object_names_from_expression(key_expr, names);
                        }
                        collect_with_object_names_from_expression(&p.value, names);
                    }
                    ObjectPropertyKind::SpreadProperty(sp) => {
                        collect_with_object_names_from_expression(&sp.argument, names);
                    }
                }
            }
        }
        Expression::TemplateLiteral(t) => {
            for e in &t.expressions {
                collect_with_object_names_from_expression(e, names);
            }
        }
        Expression::NewExpression(n) => {
            collect_with_object_names_from_expression(&n.callee, names);
            for arg in &n.arguments {
                if let Some(e) = arg.as_expression() {
                    collect_with_object_names_from_expression(e, names);
                }
            }
        }
        Expression::SequenceExpression(s) => {
            for e in &s.expressions {
                collect_with_object_names_from_expression(e, names);
            }
        }
        Expression::ParenthesizedExpression(p) => {
            collect_with_object_names_from_expression(&p.expression, names);
        }
        Expression::AwaitExpression(a) => {
            collect_with_object_names_from_expression(&a.argument, names);
        }
        Expression::YieldExpression(y) => {
            if let Some(arg) = &y.argument {
                collect_with_object_names_from_expression(arg, names);
            }
        }
        Expression::ChainExpression(chain) => match &chain.expression {
            ChainElement::CallExpression(call) => {
                collect_with_object_names_from_expression(&call.callee, names);
                for arg in &call.arguments {
                    if let Some(e) = arg.as_expression() {
                        collect_with_object_names_from_expression(e, names);
                    }
                }
            }
            ChainElement::StaticMemberExpression(m) => {
                collect_with_object_names_from_expression(&m.object, names);
            }
            ChainElement::ComputedMemberExpression(m) => {
                collect_with_object_names_from_expression(&m.object, names);
                collect_with_object_names_from_expression(&m.expression, names);
            }
            _ => {}
        },
        // Do not enter nested function bodies — they run their own capture analysis.
        Expression::FunctionExpression(_) | Expression::ArrowFunctionExpression(_) => {}
        _ => {}
    }
}

impl IrLowerer {
    pub fn lower_function_declaration(&mut self, func: &Function<'_>) {
        let name = func
            .id
            .as_ref()
            .map(|id| id.name.as_str())
            .unwrap_or("<anonymous>");

        // Strict mode: function declarations named `eval` or `arguments`
        // are a SyntaxError per ES spec (13.1.1 — early errors for strict mode).
        if self.is_strict && (name == "eval" || name == "arguments") {
            self.errors.push(crate::LoweringError {
                message: format!(
                    "SyntaxError: '{}' cannot be used as a function name in strict mode",
                    name
                ),
            });
            return;
        }

        // Analyze captures before lowering the function body
        let captures = if let Some(body) = &func.body {
            self.analyze_captures(body, &func.params.items)
        } else {
            Vec::new()
        };

        // Extract rest parameter name if present
        let rest_name = func.params.rest.as_ref().and_then(|r| {
            if let BindingPattern::BindingIdentifier(ident) = &r.rest.argument {
                Some(ident.name.as_str().to_string())
            } else {
                None
            }
        });

        // A function created inside a `with` body reserves a closure env slot
        // holding the with-environment, so its body keeps resolving with-object
        // properties after the `with` block exits.
        let with_env_slot = self.with_env_slot_for(&captures, None);

        let func_idx = self.lower_function_inner_with_captures(
            name,
            &func.params.items,
            func.body.as_deref(),
            &captures,
            rest_name.as_deref(),
            with_env_slot,
        );

        // Set generator/async flags on the TypedFunction for the
        // generator_transform pass to identify generator/async functions.
        if func.generator {
            self.builder.function_mut(func_idx).is_generator = true;
        }
        if func.r#async {
            self.builder.function_mut(func_idx).is_async = true;
        }

        // Bind the function name in current scope as a closure
        if let Some(id) = &func.id {
            let fn_name = id.name.as_str();
            let var = self.scopes.declare(fn_name);
            let func_ref = self.builder.const_i32(func_idx as i32);
            let extra = u32::from(with_env_slot.is_some());
            let env = self.build_env_for_captures_ext(&captures, extra, with_env_slot);
            let mut flags_bits = 0u32;
            if func.generator {
                flags_bits |= 4;
            }
            // Strict if the enclosing scope is strict OR the function body
            // itself contains a "use strict" directive prologue.
            if self.is_strict || body_has_use_strict(func.body.as_deref()) {
                flags_bits |= 2;
            }
            let flags_val = self.builder.const_i32(flags_bits as i32);
            let closure = self.builder.create_closure(func_ref, env, flags_val);

            // Mark generator functions so the runtime creates a generator object
            // instead of calling the closure directly.
            if func.generator {
                let key_idx = self.intern_string("__is_generator");
                let key = self.builder.const_string(key_idx);
                let true_val = self.builder.const_bool(true);
                self.builder.set_prop(closure, key, true_val);
            }

            // Set function.name and function.length
            let fn_length =
                Self::compute_function_length(&func.params.items, func.params.rest.is_some());
            self.emit_function_name_length(closure, name, fn_length);

            self.builder.write_variable(var, closure);

            // Annex B.3.3: In sloppy mode, a function declaration inside a
            // block (if/for/while/switch/etc.) also creates a var-hoisted
            // binding in the enclosing function scope. When execution reaches
            // the declaration, the var-hoisted binding is updated to point to
            // the closure value.
            if !self.is_strict && self.scopes.is_inside_non_function_block() {
                let hoisted_var = self.scopes.declare_in_function_scope(fn_name);
                self.builder.write_variable(hoisted_var, closure);
            }
        }
    }

    /// Lower a function expression, emitting `CreateClosure` to produce a value.
    ///
    /// For **named** function expressions (`var f = function fact(n) { ... }`),
    /// the function name is bound as an immutable local inside the body via an
    /// extra env slot that holds the closure value. The name is not visible
    /// outside the expression.
    pub fn lower_function_expression(&mut self, func: &Function<'_>) -> ValueId {
        let name = func
            .id
            .as_ref()
            .map(|id| id.name.as_str())
            .unwrap_or("<anonymous>");

        let is_named = name != "<anonymous>";

        // Analyze captures before lowering the function body
        let captures = if let Some(body) = &func.body {
            self.analyze_captures(body, &func.params.items)
        } else {
            Vec::new()
        };

        // Extract rest parameter name if present
        let rest_name = func.params.rest.as_ref().and_then(|r| {
            if let BindingPattern::BindingIdentifier(ident) = &r.rest.argument {
                Some(ident.name.as_str().to_string())
            } else {
                None
            }
        });

        // Compute the self-reference slot index (last slot in env) for named
        // function expressions. The inner function will load from this slot.
        let self_ref_slot = if is_named {
            let base = if captures.is_empty() {
                0u32
            } else {
                captures.iter().map(|c| c.1).max().unwrap_or(0) + 1
            };
            Some(base)
        } else {
            None
        };

        // A function created inside a `with` body reserves a closure env slot
        // holding the with-environment, so its body keeps resolving with-object
        // properties after the `with` block exits.
        let with_env_slot = self.with_env_slot_for(&captures, self_ref_slot);

        let func_idx = self.lower_function_inner_with_captures_ext(
            name,
            &func.params.items,
            func.body.as_deref(),
            &captures,
            rest_name.as_deref(),
            self_ref_slot,
            with_env_slot,
        );

        // Set generator/async flags on the TypedFunction for the
        // generator_transform pass to identify generator/async functions.
        if func.generator {
            self.builder.function_mut(func_idx).is_generator = true;
        }
        if func.r#async {
            self.builder.function_mut(func_idx).is_async = true;
        }

        let func_ref = self.builder.const_i32(func_idx as i32);
        // Build env: allocate an extra slot for the named self-reference (if
        // named) plus one for the with-environment (if inside a with body).
        let extra = u32::from(is_named) + u32::from(with_env_slot.is_some());
        let env = self.build_env_for_captures_ext(&captures, extra, with_env_slot);
        let mut flags_bits = 0u32;
        if func.generator {
            flags_bits |= 4;
        }
        // Strict if the enclosing scope is strict OR the function body
        // itself contains a "use strict" directive prologue.
        if self.is_strict || body_has_use_strict(func.body.as_deref()) {
            flags_bits |= 2;
        }
        let flags_val = self.builder.const_i32(flags_bits as i32);
        let closure = self.builder.create_closure(func_ref, env, flags_val);

        // Store the closure value into the env's self-reference slot so the
        // inner function can load it for recursive calls.
        if let Some(slot) = self_ref_slot {
            self.builder.env_store(env, slot, closure);
        }

        // Mark generator functions so the runtime creates a generator object
        // instead of calling the closure directly.
        if func.generator {
            let key_idx = self.intern_string("__is_generator");
            let key = self.builder.const_string(key_idx);
            let true_val = self.builder.const_bool(true);
            self.builder.set_prop(closure, key, true_val);
        }

        // Set function.name and function.length.
        // If the function has an explicit name, use it. Otherwise, use empty
        // string — the caller (variable declaration lowering) may override
        // with the inferred name from the assignment target.
        let fn_length =
            Self::compute_function_length(&func.params.items, func.params.rest.is_some());
        let display_name = if name == "<anonymous>" { "" } else { name };
        self.emit_function_name_length(closure, display_name, fn_length);

        closure
    }

    /// Lower an arrow function expression, emitting `CreateClosure` with
    /// lexical `this` capture.
    pub fn lower_arrow_function(&mut self, arrow: &ArrowFunctionExpression<'_>) -> ValueId {
        let captures = self.analyze_captures(&arrow.body, &arrow.params.items);

        let rest_name = arrow.params.rest.as_ref().and_then(|r| {
            if let BindingPattern::BindingIdentifier(ident) = &r.rest.argument {
                Some(ident.name.as_str().to_string())
            } else {
                None
            }
        });

        // A function created inside a `with` body reserves a closure env slot
        // holding the with-environment, so its body keeps resolving with-object
        // properties after the `with` block exits.
        let with_env_slot = self.with_env_slot_for(&captures, None);

        let func_idx = if arrow.expression {
            self.lower_function_inner_expression_arrow_with_captures(
                "<arrow>",
                &arrow.params.items,
                &arrow.body,
                &captures,
                with_env_slot,
            )
        } else {
            self.lower_function_inner_with_captures(
                "<arrow>",
                &arrow.params.items,
                Some(&arrow.body),
                &captures,
                rest_name.as_deref(),
                with_env_slot,
            )
        };

        let func_ref = self.builder.const_i32(func_idx as i32);
        // Arrows: is_arrow (bit 0) + strict bit (bit 1)
        let mut arrow_flags = 1u32; // is_arrow
        if self.is_strict {
            arrow_flags |= 2;
        }
        let arrow_flags_val = self.builder.const_i32(arrow_flags as i32);
        let closure = if captures.is_empty() && with_env_slot.is_none() {
            // No captures and no with-environment — arrows still capture `this`
            let env = self.builder.this_value();
            self.builder.create_closure(func_ref, env, arrow_flags_val)
        } else {
            let extra = u32::from(with_env_slot.is_some());
            let env = self.build_env_for_captures_ext(&captures, extra, with_env_slot);
            self.builder.create_closure(func_ref, env, arrow_flags_val)
        };

        // Set function.length (name is empty for arrows; caller may override
        // with the inferred name from the assignment target).
        let fn_length =
            Self::compute_function_length(&arrow.params.items, arrow.params.rest.is_some());
        self.emit_function_name_length(closure, "", fn_length);

        closure
    }

    /// Analyze which variables a function body captures from parent scopes.
    ///
    /// Returns a list of `(name, slot_index, parent_var, kind)` tuples for
    /// captured variables. `kind` is [`CaptureKind::ByBox`] when the variable
    /// is mutated anywhere (in the parent, in this closure, or in any sibling
    /// closure), indicating that all closures must share a JsBox pointer
    /// instead of copying the value.
    fn analyze_captures(
        &mut self,
        body: &FunctionBody<'_>,
        params: &[FormalParameter<'_>],
    ) -> Vec<(String, u32, u32, CaptureKind)> {
        // `collect_free_identifiers` does not descend into `WithStatement`
        // nodes, so a `with(o)` object expression over an outer binding is
        // invisible to capture analysis. Without capturing `o`, lowering the
        // object expression inside this function resolves `o` to an SSA
        // variable owned by a *different* function's namespace, which reads
        // garbage at runtime (a `with` over an outer binding inside a function
        // produced zero output — R1-05e). Merge the with-object's free
        // identifiers into the capture set.
        let mut free_names = collect_free_identifiers(body);
        free_names.extend(collect_with_object_free_names(body));
        let mutated_names = collect_mutated_identifiers(body);

        // Collect parameter names so we don't treat them as captures
        let mut param_names = std::collections::HashSet::new();
        for p in params {
            if let BindingPattern::BindingIdentifier(ident) = &p.pattern {
                param_names.insert(ident.name.as_str().to_string());
            }
        }

        // Check which free names resolve to parent-scope variables
        // We use begin_capture_scope / resolve_with_capture / end_capture_scope
        self.scopes.begin_capture_scope();
        // Push a function scope boundary so resolve_with_capture knows the boundary
        self.scopes.push_scope(ScopeKind::Function);

        // Declare params in this temporary scope so they don't appear as captures
        for name in &param_names {
            self.scopes.declare(name);
        }

        let mut captures = Vec::new();
        let mut next_slot = 0u32;
        for name in &free_names {
            // Skip params, built-in globals, and "undefined".
            // `arguments` is declared locally in each non-arrow function's
            // prologue, so it should not be captured across function
            // boundaries.
            // TODO(Phase H): arrow functions should capture `arguments` from
            // the enclosing non-arrow function via closure env.
            if param_names.contains(name)
                || name == "undefined"
                || name == "this"
                || name == "arguments"
                || crate::globals::is_builtin_global(name)
            {
                continue;
            }

            match self.scopes.resolve_with_capture(name) {
                ResolveResult::Captured { slot, parent_var } => {
                    // Determine capture kind: if mutated in ANY scope (this
                    // closure body, or if the parent already boxed it), use ByBox.
                    let kind = if mutated_names.contains(name)
                        || self.boxed_vars.contains(name.as_str())
                    {
                        CaptureKind::ByBox
                    } else {
                        CaptureKind::ByValue
                    };
                    captures.push((name.clone(), slot, parent_var, kind));
                    if slot >= next_slot {
                        next_slot = slot + 1;
                    }
                }
                ResolveResult::NotFound => {
                    // Variable might be transitively captured — the current
                    // function already captures it from its own parent. In that
                    // case it's in self.captured_vars, and the inner function
                    // should capture it too (we'll load it from our env when
                    // building the inner env via build_env_for_captures).
                    if self.captured_vars.contains_key(name.as_str()) {
                        let slot = next_slot;
                        next_slot += 1;
                        // Transitive captures inherit the parent's capture kind.
                        // If the parent has it boxed, the child must also treat
                        // it as boxed so the JsBox pointer is forwarded.
                        let kind = if mutated_names.contains(name)
                            || self.boxed_vars.contains(name.as_str())
                        {
                            CaptureKind::ByBox
                        } else {
                            CaptureKind::ByValue
                        };
                        // parent_var is unused for transitive captures since
                        // build_env_for_captures checks captured_vars first
                        captures.push((name.clone(), slot, u32::MAX, kind));
                    }
                }
                ResolveResult::Local(_) => {}
            }
        }

        self.scopes.pop_scope();
        let _capture_info = self.scopes.end_capture_scope();

        // Sort by slot index for deterministic output
        captures.sort_by_key(|c| c.1);
        captures
    }

    /// Build an environment object for the given captures at the closure creation site.
    ///
    /// Emits EnvCreate + EnvStore for each captured variable. Returns the env ValueId.
    /// If no captures, returns const_null.
    ///
    /// For `ByValue` captures: stores a copy of the current value into the env slot.
    /// For `ByBox` captures: stores the JsBox *pointer* into the env slot. If the
    /// parent hasn't already allocated a JsBox for this variable, one is allocated
    /// now (via `AllocBox`) and the parent's SSA variable is updated to hold it.
    /// Compute the closure env slot reserved for the active with-environment.
    ///
    /// Returns `None` when the current lowering context is not inside a `with`
    /// body. When `Some(slot)`, the slot sits immediately after the capture
    /// slots (`base`), after the optional named-function-expression
    /// self-reference slot when `self_ref_slot` is `Some`. The caller must
    /// reserve one extra env slot for the closure and arrange for
    /// [`IrLowerer::build_env_for_captures_ext`] to store the with-environment
    /// value there so the inner function body can load it back.
    fn with_env_slot_for(
        &self,
        captures: &[(String, u32, u32, CaptureKind)],
        self_ref_slot: Option<u32>,
    ) -> Option<u32> {
        if self.with_env_var.is_some() {
            let base = captures.iter().map(|c| c.1).max().map_or(0, |m| m + 1);
            Some(base + u32::from(self_ref_slot.is_some()))
        } else {
            None
        }
    }

    /// Build an environment object with optional extra slots beyond captures.
    ///
    /// `extra_slots` additional slots are allocated at the end of the env.
    /// These are used for named function expression self-references (slot
    /// filled after `CreateClosure`).
    ///
    /// `with_env_slot`, when `Some`, reserves an env slot that holds the
    /// with-environment active at the closure's creation site. The closure's
    /// body loads it back and routes dynamic identifier lookups through it, so
    /// a function created inside a `with` body keeps resolving with-object
    /// properties after the `with` block has exited.
    fn build_env_for_captures_ext(
        &mut self,
        captures: &[(String, u32, u32, CaptureKind)],
        extra_slots: u32,
        with_env_slot: Option<u32>,
    ) -> ValueId {
        if captures.is_empty() && extra_slots == 0 && with_env_slot.is_none() {
            return self.builder.const_null();
        }

        let base_count = if captures.is_empty() {
            0
        } else {
            captures.iter().map(|c| c.1).max().unwrap_or(0) + 1
        };
        let slot_count = base_count + extra_slots;
        let env = self.builder.env_create(slot_count);

        for (name, slot, parent_var, kind) in captures {
            match kind {
                CaptureKind::ByValue => {
                    // Read the variable from the current scope (parent of the closure)
                    // It might itself be a captured var from our own env
                    let val = if let Some(our_env) = self.capture_env {
                        if let Some(&our_slot) = self.captured_vars.get(name.as_str()) {
                            self.builder.env_load(our_env, our_slot)
                        } else {
                            self.builder.read_variable(*parent_var, IrType::JSValue)
                        }
                    } else {
                        self.builder.read_variable(*parent_var, IrType::JSValue)
                    };
                    self.builder.env_store(env, *slot, val);
                }
                CaptureKind::ByBox => {
                    // For ByBox captures, we store the JsBox POINTER in the env slot.
                    // The parent scope should already have a JsBox for this variable
                    // (allocated when we detected the variable needs boxing).
                    let box_ptr = if self.boxed_vars.contains(name.as_str()) {
                        // Parent already has a box — read the box pointer
                        if let Some(our_env) = self.capture_env {
                            if let Some(&our_slot) = self.captured_vars.get(name.as_str()) {
                                // Transitive: load box pointer from our env
                                self.builder.env_load(our_env, our_slot)
                            } else {
                                // We're the declaring scope — read box pointer from SSA
                                self.builder.read_variable(*parent_var, IrType::JSValue)
                            }
                        } else {
                            self.builder.read_variable(*parent_var, IrType::JSValue)
                        }
                    } else {
                        // First time we see this variable needs boxing.
                        // Allocate a JsBox with the current value and update the
                        // parent's SSA variable to hold the box pointer.
                        let current_val = if let Some(our_env) = self.capture_env {
                            if let Some(&our_slot) = self.captured_vars.get(name.as_str()) {
                                self.builder.env_load(our_env, our_slot)
                            } else {
                                self.builder.read_variable(*parent_var, IrType::JSValue)
                            }
                        } else {
                            self.builder.read_variable(*parent_var, IrType::JSValue)
                        };
                        let box_val = self.builder.alloc_box(current_val);
                        // Update the parent's SSA variable to be the box pointer
                        // so subsequent reads/writes go through the box.
                        self.builder.write_variable(*parent_var, box_val);
                        self.boxed_vars.insert(name.clone());
                        box_val
                    };
                    self.builder.env_store(env, *slot, box_ptr);
                }
            }
        }

        // Store the active with-environment into its reserved slot so the
        // closure body can load it back and keep resolving with-object
        // properties at call time. `with_env_slot` is only ever `Some` when
        // `self.with_env_var` is set (the caller computed the slot from it).
        if let Some(slot) = with_env_slot
            && let Some(with_env_var) = self.with_env_var
        {
            let with_env = self.builder.read_variable(with_env_var, IrType::JSValue);
            self.builder.env_store(env, slot, with_env);
        }

        env
    }

    /// Lower a function body with capture information, setting up EnvLoad
    /// for captured variables inside the function.
    fn lower_function_inner_with_captures(
        &mut self,
        name: &str,
        params: &[FormalParameter<'_>],
        body: Option<&FunctionBody<'_>>,
        captures: &[(String, u32, u32, CaptureKind)],
        rest_param_name: Option<&str>,
        with_env_slot: Option<u32>,
    ) -> usize {
        self.lower_function_inner_with_captures_ext(
            name,
            params,
            body,
            captures,
            rest_param_name,
            None,
            with_env_slot,
        )
    }

    /// Lower a function body with capture information and optional self-reference slot.
    ///
    /// `self_ref_slot` is `Some(slot_idx)` for named function expressions, where
    /// the closure's own value is stored at that env slot by the caller. Inside the
    /// body, we load from that slot and bind the function name as an immutable local.
    ///
    /// `with_env_slot` is `Some(slot_idx)` when the function is created inside a
    /// `with` body; the slot holds the with-environment so the body keeps resolving
    /// with-object properties at call time.
    ///
    /// Eight parameters keeps the caller explicit about how the closure env is laid
    /// out; each is a distinct axis of the lowering state.
    #[allow(clippy::too_many_arguments)] // each param is a distinct axis of closure-env layout; a struct would hide the caller's obligation to set every one
    fn lower_function_inner_with_captures_ext(
        &mut self,
        name: &str,
        params: &[FormalParameter<'_>],
        body: Option<&FunctionBody<'_>>,
        captures: &[(String, u32, u32, CaptureKind)],
        rest_param_name: Option<&str>,
        self_ref_slot: Option<u32>,
        with_env_slot: Option<u32>,
    ) -> usize {
        // Save current function's state
        let saved_block = self.current_block;
        let saved_break = self.loop_break_target;
        let saved_continue = self.loop_continue_target;
        let saved_terminated = self.terminated;
        let saved_capture_env = self.capture_env.take();
        let saved_captured_vars = std::mem::take(&mut self.captured_vars);
        let saved_is_strict = self.is_strict;
        let saved_const_vars = std::mem::take(&mut self.const_vars);
        let saved_tdz_vars = std::mem::take(&mut self.tdz_vars);
        let saved_boxed_vars = std::mem::take(&mut self.boxed_vars);
        let saved_poisoned_env_var = self.poisoned_env_var.take();
        let saved_poisoned_slot_map = std::mem::take(&mut self.poisoned_slot_map);
        let saved_with_env_var = self.with_env_var.take();
        let saved_with_env_stack = std::mem::take(&mut self.with_env_stack);
        let saved_with_known_props = self.with_known_props.take();
        let saved_with_known_props_stack = std::mem::take(&mut self.with_known_props_stack);

        // Save try/catch/finally state — nested functions must NOT inherit
        // the enclosing function's exception handling context because their
        // blocks live in a separate function namespace.
        let saved_finally_target = self.finally_target.take();
        let saved_finally_return_var = self.finally_return_var.take();
        let saved_finally_has_return_var = self.finally_has_return_var.take();
        let saved_finally_exception_var = self.finally_exception_var.take();
        let saved_finally_has_exception_var = self.finally_has_exception_var.take();
        let saved_finally_catch_redirects = self.finally_catch_redirects_throw;
        let saved_finally_catch_depth = self.finally_catch_depth;
        let saved_finally_has_break_var = self.finally_has_break_var.take();
        let saved_finally_break_target_var = self.finally_break_target_var.take();
        let saved_finally_is_continue_var = self.finally_is_continue_var.take();
        let saved_finally_jump_targets = std::mem::take(&mut self.finally_jump_targets);
        let saved_finally_external_targets = std::mem::take(&mut self.finally_external_targets);
        let saved_catch_target_stack = std::mem::take(&mut self.catch_target_stack);
        let saved_label_targets = std::mem::take(&mut self.label_targets);
        self.finally_catch_redirects_throw = false;
        self.finally_catch_depth = 0;

        // Suspend the outer function so we can build the inner one
        let suspended = self.builder.suspend_function();

        // Build parameter list for the function signature
        let param_list: Vec<(&str, IrType)> = params
            .iter()
            .map(|p| {
                let param_name = match &p.pattern {
                    BindingPattern::BindingIdentifier(ident) => ident.name.as_str(),
                    _ => "_",
                };
                (param_name, IrType::JSValue)
            })
            .collect();

        self.builder
            .begin_function(name, param_list, IrType::JSValue);
        let func_idx = self.function_count;
        self.function_count += 1;

        let entry = self.builder.create_block();
        self.builder.switch_to_block(entry);
        self.builder.seal_block(entry);
        self.current_block = Some(entry);
        self.loop_break_target = None;
        self.loop_continue_target = None;
        self.terminated = false;

        // Set up capture env if this function has captures, a self-ref slot,
        // or a captured with-environment.
        // Named function expressions may have an env even with no captures
        // (the env holds only the self-reference slot).
        let has_env = !captures.is_empty() || self_ref_slot.is_some() || with_env_slot.is_some();
        if has_env {
            // The env is passed as the last parameter (convention: Cranelift lowering
            // reads it from the closure). For now, we use load_param with a special index.
            // Convention: env is param at index = params.len()
            let env_val = self.builder.load_param(params.len() as u32);
            self.capture_env = Some(env_val);
            let mut cap_map = HashMap::new();
            for (cap_name, slot, _, _) in captures {
                cap_map.insert(cap_name.clone(), *slot);
            }
            self.captured_vars = cap_map;
        } else {
            self.capture_env = None;
            self.captured_vars = HashMap::new();
        }

        // Mark ByBox captures in the boxed_vars set so that reads/writes
        // in the closure body go through BoxLoad/BoxStore.
        self.boxed_vars = HashSet::new();
        for (cap_name, _, _, kind) in captures {
            if *kind == CaptureKind::ByBox {
                self.boxed_vars.insert(cap_name.clone());
            }
        }

        // Push function scope
        self.scopes.push_scope(ScopeKind::Function);

        // Initialize captured variables from env slots.
        // For ByValue captures: EnvLoad the value and write it as an SSA variable.
        // For ByBox captures: EnvLoad the JsBox pointer and write it as an SSA
        //   variable. Reads/writes to this variable will go through BoxLoad/BoxStore
        //   (handled by read_boxed_or_var / write_var_by_name).
        if let Some(env) = self.capture_env {
            for (cap_name, &slot) in &self.captured_vars.clone() {
                let var = self.scopes.declare(cap_name);
                let val = self.builder.env_load(env, slot);
                self.builder.write_variable(var, val);
            }
        }

        // A function created inside a `with` body captures the with-environment
        // in a closure env slot. Restore it here so identifier reads in the
        // body resolve through the with-object at runtime.
        if let Some(slot) = with_env_slot
            && let Some(env) = self.capture_env
        {
            let with_val = self.builder.env_load(env, slot);
            let wv = self.alloc_temp_var();
            self.builder.write_variable(wv, with_val);
            self.with_env_var = Some(wv);
        }

        // Detect "use strict" directive BEFORE lowering parameters so that
        // strict-mode identifier restrictions apply to parameter names too.
        if let Some(body) = body {
            for directive in &body.directives {
                if directive.directive.as_str() == "use strict" {
                    self.is_strict = true;
                    break;
                }
            }
        }

        // Strict mode: function names `eval` and `arguments` are SyntaxError
        // per ES spec (13.1.1). This catches the case where the function body
        // itself contains "use strict" (the enclosing-scope case is checked
        // in lower_function_declaration).
        if self.is_strict
            && name != "<anonymous>"
            && name != "<arrow>"
            && (name == "eval" || name == "arguments")
        {
            self.errors.push(crate::LoweringError {
                message: format!(
                    "SyntaxError: '{}' cannot be used as a function name in strict mode",
                    name
                ),
            });
        }

        // Strict mode: check for duplicate parameter names (ES spec 15.1.1).
        // In strict mode, duplicate parameter names are a SyntaxError.
        if self.is_strict {
            let mut seen_params: HashSet<String> = HashSet::new();
            for param in params {
                let param_names = Self::collect_binding_names(&param.pattern);
                for pname in &param_names {
                    if !seen_params.insert(pname.clone()) {
                        self.errors.push(crate::LoweringError {
                            message:
                                "SyntaxError: Duplicate parameter name not allowed in this context"
                                    .to_string(),
                        });
                    }
                }
            }
            // Also check the rest parameter
            if let Some(rest_name) = &rest_param_name
                && !seen_params.insert(rest_name.to_string())
            {
                self.errors.push(crate::LoweringError {
                    message: "SyntaxError: Duplicate parameter name not allowed in this context"
                        .to_string(),
                });
            }
        }

        // Bind the function's own name inside its scope (enables recursion).
        // For named function expressions with self_ref_slot: load the closure
        // value from the env slot (patched by the caller after CreateClosure).
        // For function declarations: use const_i32(func_idx) as a fallback
        // (declarations bind in the outer scope, not via env).
        if name != "<anonymous>" && name != "<arrow>" {
            let self_var = self.scopes.declare(name);
            if let (Some(slot), Some(env)) = (self_ref_slot, self.capture_env) {
                // Named function expression: load self-reference from env
                let self_ref = self.builder.env_load(env, slot);
                self.builder.write_variable(self_var, self_ref);
                // The name is immutable: assignment silently fails (sloppy)
                // or throws TypeError (strict). Add to const_vars.
                self.const_vars.insert(name.to_string());
            } else {
                // Function declaration: bind func index for hoisting
                let self_ref = self.builder.const_i32(func_idx as i32);
                self.builder.write_variable(self_var, self_ref);
            }
        }

        // Declare parameters in scope and write initial values
        for (i, param) in params.iter().enumerate() {
            let param_val = self.builder.load_param(i as u32);

            // Handle default parameter via initializer field.
            // Per the ES spec, default parameters trigger on `undefined`
            // only, not on `null`.
            if let Some(default_expr) = &param.initializer {
                let undef = self.builder.const_undefined();
                let is_undef = self.builder.eq_strict(param_val, undef);
                let then_bb = self.builder.create_block();
                let else_bb = self.builder.create_block();
                let merge_bb = self.builder.create_block();
                let branch_block = self.current_block_id();

                let temp_var = self.alloc_temp_var();
                self.builder.write_variable(temp_var, param_val);
                self.builder.br_if(is_undef, then_bb, else_bb);

                self.builder.switch_to_block(then_bb);
                self.builder.add_predecessor(then_bb, branch_block);
                self.current_block = Some(then_bb);
                let default_val = self.lower_expression(default_expr);
                self.builder.write_variable(temp_var, default_val);
                self.builder.br(merge_bb);
                let then_exit = self.current_block_id();
                self.builder.seal_block(then_bb);

                self.builder.switch_to_block(else_bb);
                self.builder.add_predecessor(else_bb, branch_block);
                self.current_block = Some(else_bb);
                self.builder.br(merge_bb);
                let else_exit = self.current_block_id();
                self.builder.seal_block(else_bb);

                self.builder.switch_to_block(merge_bb);
                self.builder.add_predecessor(merge_bb, then_exit);
                self.builder.add_predecessor(merge_bb, else_exit);
                self.builder.seal_block(merge_bb);
                self.current_block = Some(merge_bb);

                let final_val = self.builder.read_variable(temp_var, IrType::JSValue);
                self.lower_binding_pattern(&param.pattern, final_val);
            } else {
                self.lower_binding_pattern(&param.pattern, param_val);
            }
        }

        // Emit `arguments` object for non-arrow functions, but only
        // when the body actually references `arguments`.
        if name != "<arrow>"
            && let Some(body) = body
        {
            let usage = crate::capture::scan_arguments_usage(body);
            if usage != crate::capture::ArgumentsUsage::Unused {
                // Determine if mapped arguments are needed (sloppy mode only,
                // no rest/destructuring/default params).
                let has_rest = rest_param_name.is_some();
                let _mapped =
                    crate::capture::needs_mapped_arguments(self.is_strict, has_rest, params, usage);
                // TODO(v0.4.34): When _mapped is true, parameters and
                // arguments[i] should share the same storage (JsBox) so
                // that mutations to one are reflected in the other. For now
                // the runtime creates a plain arguments object with
                // `.callee` support; full mapped aliasing is deferred.

                let args_val = self.builder.create_arguments();
                let args_var = self.scopes.declare("arguments");
                self.builder.write_variable(args_var, args_val);
            }
        }

        // Bind rest parameter if present (e.g., `...values`)
        if let Some(rest_name) = rest_param_name {
            let start_idx = params.len() as f64;
            // Emit call to __esc_rt_rest_args(start_index)
            // Pass as NaN-boxed i64 (raw f64 bits) since all runtime args are u64.
            let rt_name_idx = self.intern_string("__esc_rt_rest_args");
            let rt_name = self.builder.const_string(rt_name_idx);
            let start_val = self.builder.const_i64(start_idx.to_bits() as i64);
            let rest_arr = self.builder.call_runtime(rt_name, vec![start_val]);
            let rest_var = self.scopes.declare(rest_name);
            self.builder.write_variable(rest_var, rest_arr);
        }

        // Detect if this function needs a dynamic EscEnvironment (has direct eval).
        // If so, create an EscEnvironment and populate its slot map with all
        // local variable names so eval'd code can access them by name.
        let is_poisoned = body_has_direct_eval(body);
        if is_poisoned {
            let local_names = collect_function_local_names(params, body);
            let slot_count = local_names.len() as u32;

            // Create EscEnvironment via runtime call
            let rt_create_idx = self.intern_string("__esc_rt_esc_env_create");
            let rt_create_name = self.builder.const_string(rt_create_idx);
            let slot_count_val = self.builder.const_i32(slot_count as i32);
            let outer = self.builder.const_undefined();
            let esc_env = self
                .builder
                .call_runtime(rt_create_name, vec![slot_count_val, outer]);

            // Populate slot map: for each variable name, call
            // __esc_rt_esc_env_populate_slot_map(env, name_string, slot_index)
            let rt_populate_idx = self.intern_string("__esc_rt_esc_env_populate_slot_map");
            let rt_populate_name = self.builder.const_string(rt_populate_idx);
            let mut slot_map = HashMap::new();
            for (slot_idx, var_name) in local_names.iter().enumerate() {
                let name_str_idx = self.intern_string(var_name);
                let name_val = self.builder.const_string(name_str_idx);
                let slot_val = self.builder.const_i32(slot_idx as i32);
                self.builder
                    .call_runtime(rt_populate_name, vec![esc_env, name_val, slot_val]);
                slot_map.insert(var_name.clone(), slot_idx as u32);
            }

            // Store initial parameter values into the EscEnvironment slots
            let rt_set_idx = self.intern_string("__esc_rt_esc_env_set_boxed");
            let rt_set_name = self.builder.const_string(rt_set_idx);
            for (i, param) in params.iter().enumerate() {
                if let BindingPattern::BindingIdentifier(ident) = &param.pattern {
                    let param_name = ident.name.as_str();
                    if let Some(&slot) = slot_map.get(param_name) {
                        let param_val = self.builder.load_param(i as u32);
                        let slot_val = self.builder.const_i32(slot as i32);
                        self.builder
                            .call_runtime(rt_set_name, vec![esc_env, slot_val, param_val]);
                    }
                }
            }

            let env_var = self.alloc_temp_var();
            self.builder.write_variable(env_var, esc_env);
            self.poisoned_env_var = Some(env_var);
            self.poisoned_slot_map = slot_map;
        }

        // Lower body
        if let Some(body) = body {
            // Note: "use strict" directive detection already happened before
            // parameter lowering (above) so strict-mode identifier checks
            // apply to parameter names.

            // Check if body `var` declarations shadow parameter names.
            // When they do, we need a separate body scope so that default
            // parameter expressions reference the parameter values, not the
            // body `var` redeclarations. (ES2015 sec 9.2.12 step 28)
            let needs_body_scope = has_var_shadowing_params(params, body);

            if needs_body_scope {
                self.scopes.push_scope(ScopeKind::Block);
                // Re-declare each parameter in the body scope with its
                // current value, so body code sees the new SSA variables
                // while default-parameter closures still close over the
                // parameter-scope originals.
                for param in params {
                    if let BindingPattern::BindingIdentifier(ident) = &param.pattern {
                        let param_name = ident.name.as_str();
                        if let Some(outer_var) = self.scopes.resolve(param_name) {
                            let val = self.builder.read_variable(outer_var, IrType::JSValue);
                            let body_var = self.scopes.declare(param_name);
                            self.builder.write_variable(body_var, val);
                        }
                    }
                }
            }

            // Pre-scan function body for let/const TDZ names
            let (tdz_names, _const_names) = Self::collect_block_lexical_names(&body.statements);
            for tdz_name in &tdz_names {
                self.tdz_vars.insert(tdz_name.clone());
            }

            if body.statements.is_empty() {
                // Empty body (arrow expression bodies handled elsewhere)
            }
            for stmt in &body.statements {
                if self.terminated {
                    break;
                }
                self.lower_statement(stmt);
            }

            if needs_body_scope {
                self.scopes.pop_scope();
            }
        }

        // Ensure function ends with a return
        if !self.terminated {
            let undef = self.builder.const_undefined();
            self.builder.ret(Some(undef));
        }

        self.scopes.pop_scope();
        self.builder.end_function();

        // Resume the outer function
        self.builder.resume_function(suspended);

        // Restore previous function's state
        self.current_block = saved_block;
        self.loop_break_target = saved_break;
        self.loop_continue_target = saved_continue;
        self.terminated = saved_terminated;
        self.capture_env = saved_capture_env;
        self.captured_vars = saved_captured_vars;
        self.is_strict = saved_is_strict;
        self.const_vars = saved_const_vars;
        self.tdz_vars = saved_tdz_vars;
        self.boxed_vars = saved_boxed_vars;
        self.poisoned_env_var = saved_poisoned_env_var;
        self.poisoned_slot_map = saved_poisoned_slot_map;
        self.with_env_var = saved_with_env_var;
        self.with_env_stack = saved_with_env_stack;
        self.with_known_props = saved_with_known_props;
        self.with_known_props_stack = saved_with_known_props_stack;

        // Restore try/catch/finally state
        self.finally_target = saved_finally_target;
        self.finally_return_var = saved_finally_return_var;
        self.finally_has_return_var = saved_finally_has_return_var;
        self.finally_exception_var = saved_finally_exception_var;
        self.finally_has_exception_var = saved_finally_has_exception_var;
        self.finally_catch_redirects_throw = saved_finally_catch_redirects;
        self.finally_catch_depth = saved_finally_catch_depth;
        self.finally_has_break_var = saved_finally_has_break_var;
        self.finally_break_target_var = saved_finally_break_target_var;
        self.finally_is_continue_var = saved_finally_is_continue_var;
        self.finally_jump_targets = saved_finally_jump_targets;
        self.finally_external_targets = saved_finally_external_targets;
        self.catch_target_stack = saved_catch_target_stack;
        self.label_targets = saved_label_targets;

        func_idx
    }

    /// Lower an expression-body arrow function with capture support.
    fn lower_function_inner_expression_arrow_with_captures(
        &mut self,
        name: &str,
        params: &[FormalParameter<'_>],
        body: &FunctionBody<'_>,
        captures: &[(String, u32, u32, CaptureKind)],
        with_env_slot: Option<u32>,
    ) -> usize {
        let saved_block = self.current_block;
        let saved_break = self.loop_break_target;
        let saved_continue = self.loop_continue_target;
        let saved_terminated = self.terminated;
        let saved_capture_env = self.capture_env.take();
        let saved_captured_vars = std::mem::take(&mut self.captured_vars);
        let saved_is_strict = self.is_strict;
        let saved_const_vars = std::mem::take(&mut self.const_vars);
        let saved_tdz_vars = std::mem::take(&mut self.tdz_vars);
        let saved_boxed_vars = std::mem::take(&mut self.boxed_vars);
        let saved_poisoned_env_var = self.poisoned_env_var.take();
        let saved_poisoned_slot_map = std::mem::take(&mut self.poisoned_slot_map);
        let saved_with_env_var = self.with_env_var.take();
        let saved_with_env_stack = std::mem::take(&mut self.with_env_stack);
        let saved_with_known_props = self.with_known_props.take();
        let saved_with_known_props_stack = std::mem::take(&mut self.with_known_props_stack);

        // Save try/catch/finally state (same as lower_function_inner_with_captures_ext)
        let saved_finally_target = self.finally_target.take();
        let saved_finally_return_var = self.finally_return_var.take();
        let saved_finally_has_return_var = self.finally_has_return_var.take();
        let saved_finally_exception_var = self.finally_exception_var.take();
        let saved_finally_has_exception_var = self.finally_has_exception_var.take();
        let saved_finally_catch_redirects = self.finally_catch_redirects_throw;
        let saved_finally_catch_depth = self.finally_catch_depth;
        let saved_finally_has_break_var = self.finally_has_break_var.take();
        let saved_finally_break_target_var = self.finally_break_target_var.take();
        let saved_finally_is_continue_var = self.finally_is_continue_var.take();
        let saved_finally_jump_targets = std::mem::take(&mut self.finally_jump_targets);
        let saved_finally_external_targets = std::mem::take(&mut self.finally_external_targets);
        let saved_catch_target_stack = std::mem::take(&mut self.catch_target_stack);
        let saved_label_targets = std::mem::take(&mut self.label_targets);
        self.finally_catch_redirects_throw = false;
        self.finally_catch_depth = 0;

        let suspended = self.builder.suspend_function();

        let param_list: Vec<(&str, IrType)> = params
            .iter()
            .map(|p| {
                let param_name = match &p.pattern {
                    BindingPattern::BindingIdentifier(ident) => ident.name.as_str(),
                    _ => "_",
                };
                (param_name, IrType::JSValue)
            })
            .collect();

        self.builder
            .begin_function(name, param_list, IrType::JSValue);
        let func_idx = self.function_count;
        self.function_count += 1;

        let entry = self.builder.create_block();
        self.builder.switch_to_block(entry);
        self.builder.seal_block(entry);
        self.current_block = Some(entry);
        self.loop_break_target = None;
        self.loop_continue_target = None;
        self.terminated = false;

        // Set up capture env
        if !captures.is_empty() || with_env_slot.is_some() {
            let env_val = self.builder.load_param(params.len() as u32);
            self.capture_env = Some(env_val);
            let mut cap_map = HashMap::new();
            for (cap_name, slot, _, _) in captures {
                cap_map.insert(cap_name.clone(), *slot);
            }
            self.captured_vars = cap_map;
        } else {
            self.capture_env = None;
            self.captured_vars = HashMap::new();
        }

        // A function created inside a `with` body keeps resolving with-object
        // properties through the with-environment captured in its closure env.
        if let Some(slot) = with_env_slot
            && let Some(env) = self.capture_env
        {
            let with_val = self.builder.env_load(env, slot);
            let wv = self.alloc_temp_var();
            self.builder.write_variable(wv, with_val);
            self.with_env_var = Some(wv);
        }

        // Mark ByBox captures
        self.boxed_vars = HashSet::new();
        for (cap_name, _, _, kind) in captures {
            if *kind == CaptureKind::ByBox {
                self.boxed_vars.insert(cap_name.clone());
            }
        }

        // Push function scope
        self.scopes.push_scope(ScopeKind::Function);

        // Bind parameters — handle both simple identifiers and destructuring patterns
        // (array/object patterns) via the same lower_binding_pattern used by lower_function.
        // Also handles default parameter values (Initializer).
        for (i, param) in params.iter().enumerate() {
            let param_val = self.builder.load_param(i as u32);

            if let Some(default_expr) = &param.initializer {
                // Default parameters trigger on `undefined` only, not `null`.
                let undef = self.builder.const_undefined();
                let is_undef = self.builder.eq_strict(param_val, undef);
                let then_bb = self.builder.create_block();
                let else_bb = self.builder.create_block();
                let merge_bb = self.builder.create_block();
                let branch_block = self.current_block_id();

                let temp_var = self.alloc_temp_var();
                self.builder.write_variable(temp_var, param_val);
                self.builder.br_if(is_undef, then_bb, else_bb);

                self.builder.switch_to_block(then_bb);
                self.builder.add_predecessor(then_bb, branch_block);
                self.current_block = Some(then_bb);
                let default_val = self.lower_expression(default_expr);
                self.builder.write_variable(temp_var, default_val);
                self.builder.br(merge_bb);
                let then_exit = self.current_block_id();
                self.builder.seal_block(then_bb);

                self.builder.switch_to_block(else_bb);
                self.builder.add_predecessor(else_bb, branch_block);
                self.current_block = Some(else_bb);
                self.builder.br(merge_bb);
                let else_exit = self.current_block_id();
                self.builder.seal_block(else_bb);

                self.builder.switch_to_block(merge_bb);
                self.builder.add_predecessor(merge_bb, then_exit);
                self.builder.add_predecessor(merge_bb, else_exit);
                self.builder.seal_block(merge_bb);
                self.current_block = Some(merge_bb);

                let final_val = self.builder.read_variable(temp_var, IrType::JSValue);
                self.lower_binding_pattern(&param.pattern, final_val);
            } else {
                self.lower_binding_pattern(&param.pattern, param_val);
            }
        }

        // Lower the single expression and return it
        if let Some(Statement::ExpressionStatement(expr_stmt)) = body.statements.first() {
            let val = self.lower_expression(&expr_stmt.expression);
            self.builder.ret(Some(val));
            self.terminated = true;
        }

        // Fallback: ensure function ends with return
        if !self.terminated {
            let undef = self.builder.const_undefined();
            self.builder.ret(Some(undef));
        }

        self.scopes.pop_scope();
        self.builder.end_function();
        self.builder.resume_function(suspended);

        self.current_block = saved_block;
        self.loop_break_target = saved_break;
        self.loop_continue_target = saved_continue;
        self.terminated = saved_terminated;
        self.capture_env = saved_capture_env;
        self.captured_vars = saved_captured_vars;
        self.is_strict = saved_is_strict;
        self.const_vars = saved_const_vars;
        self.tdz_vars = saved_tdz_vars;
        self.boxed_vars = saved_boxed_vars;
        self.poisoned_env_var = saved_poisoned_env_var;
        self.poisoned_slot_map = saved_poisoned_slot_map;
        self.with_env_var = saved_with_env_var;
        self.with_env_stack = saved_with_env_stack;
        self.with_known_props = saved_with_known_props;
        self.with_known_props_stack = saved_with_known_props_stack;

        // Restore try/catch/finally state
        self.finally_target = saved_finally_target;
        self.finally_return_var = saved_finally_return_var;
        self.finally_has_return_var = saved_finally_has_return_var;
        self.finally_exception_var = saved_finally_exception_var;
        self.finally_has_exception_var = saved_finally_has_exception_var;
        self.finally_catch_redirects_throw = saved_finally_catch_redirects;
        self.finally_catch_depth = saved_finally_catch_depth;
        self.finally_has_break_var = saved_finally_has_break_var;
        self.finally_break_target_var = saved_finally_break_target_var;
        self.finally_is_continue_var = saved_finally_is_continue_var;
        self.finally_jump_targets = saved_finally_jump_targets;
        self.finally_external_targets = saved_finally_external_targets;
        self.catch_target_stack = saved_catch_target_stack;
        self.label_targets = saved_label_targets;

        func_idx
    }

    /// Lower a function without capture analysis (used for declarations and
    /// internal functions like class constructors/methods).
    fn lower_function_inner(
        &mut self,
        name: &str,
        params: &[FormalParameter<'_>],
        body: Option<&FunctionBody<'_>>,
    ) -> usize {
        self.lower_function_inner_with_captures(name, params, body, &[], None, None)
    }

    /// Lower a class declaration. Emits `CreateObject` for the prototype,
    /// `CreateClosure` for the constructor and methods, and binds the class
    /// name in the current scope.
    pub fn lower_class_declaration(&mut self, class: &Class<'_>) {
        let ctor_closure = self.lower_class_body(class);

        // Bind class name to the constructor closure in the outer scope
        if let Some(id) = &class.id {
            let var = self.scopes.declare(id.name.as_str());
            self.builder.write_variable(var, ctor_closure);
        }
    }

    /// Lower a class expression. Returns the constructor closure as a value.
    ///
    /// For named class expressions (`class Foo { ... }`), the name `Foo` is
    /// only visible inside the class body (analogous to named function
    /// expressions — see v0.3 step 0.3.15). The name is NOT bound in the
    /// outer scope.
    pub fn lower_class_expression(&mut self, class: &Class<'_>) -> ValueId {
        // For named class expressions, the name binding is created inside
        // lower_class_body's scope (pushed and popped internally).
        // Unlike declarations, we do NOT bind the name in the outer scope.
        self.lower_class_body(class)
    }

    /// Shared class lowering logic used by both declarations and expressions.
    ///
    /// Creates the prototype object, lowers the constructor and methods,
    /// sets up the prototype chain for `extends`, and returns the constructor
    /// closure value. The caller is responsible for binding the class name
    /// in the appropriate scope.
    fn lower_class_body(&mut self, class: &Class<'_>) -> ValueId {
        // Class bodies are always strict mode per the spec
        let saved_is_strict = self.is_strict;
        self.is_strict = true;

        let class_name = class
            .id
            .as_ref()
            .map(|id| id.name.as_str())
            .unwrap_or("<anonymous>");

        // For named class expressions, push a scope so the name is only
        // visible inside the class body (not in the outer scope).
        let is_named_expr = class.id.is_some();
        if is_named_expr {
            self.scopes.push_scope(ScopeKind::Block);
        }

        // Handle `extends` clause
        let super_proto = class
            .super_class
            .as_ref()
            .map(|super_class| self.lower_expression(super_class));

        // Create prototype object
        let proto = self.builder.create_object();

        // If there is a superclass, set the prototype chain:
        // Dog.prototype.__proto__ = Animal.prototype
        if let Some(super_val) = super_proto {
            let super_proto_key_idx = self.intern_string("prototype");
            let super_proto_key = self.builder.const_string(super_proto_key_idx);
            let super_proto_obj = self.builder.get_prop(super_val, super_proto_key);
            let proto_link_idx = self.intern_string("__proto__");
            let proto_link_key = self.builder.const_string(proto_link_idx);
            self.builder
                .set_prop(proto, proto_link_key, super_proto_obj);
        }

        // Pre-scan for private fields and methods: allocate unique IDs
        let saved_private_ids = std::mem::take(&mut self.private_name_ids);
        let mut private_field_inits: Vec<(u32, Option<usize>)> = Vec::new(); // (private_id, init_expr_element_index)
        let mut private_method_ids: Vec<(String, u32)> = Vec::new(); // (method_name, private_id)
        for (elem_idx, element) in class.body.body.iter().enumerate() {
            match element {
                ClassElement::PropertyDefinition(prop_def) => {
                    let key_name = prop_def.key.private_name().map(|n| n.as_str().to_string());
                    if let Some(name) = key_name {
                        let pid = self.allocate_private_name_id();
                        self.private_name_ids.insert(name, pid);
                        let has_init = prop_def.value.is_some();
                        private_field_inits
                            .push((pid, if has_init { Some(elem_idx) } else { None }));
                    }
                }
                ClassElement::MethodDefinition(method) => {
                    if let Some(priv_ident) = method.key.private_name() {
                        let name = priv_ident.as_str().to_string();
                        if !self.private_name_ids.contains_key(&name) {
                            let pid = self.allocate_private_name_id();
                            self.private_name_ids.insert(name.clone(), pid);
                            private_method_ids.push((name, pid));
                        }
                    }
                }
                _ => {}
            }
        }

        let has_private_fields = !private_field_inits.is_empty();
        let has_private_methods = !private_method_ids.is_empty();

        // Lower constructor
        let mut ctor_func_idx = None;
        let mut ctor_length = 0u32;
        for element in &class.body.body {
            if let ClassElement::MethodDefinition(method) = element
                && method.kind == MethodDefinitionKind::Constructor
            {
                let method_fn = &method.value;
                ctor_length = Self::compute_function_length(
                    &method_fn.params.items,
                    method_fn.params.rest.is_some(),
                );
                if has_private_fields || has_private_methods {
                    // Build constructor with private field installations
                    let idx = self.lower_constructor_with_private_fields(
                        class_name,
                        &method_fn.params.items,
                        method_fn.body.as_deref(),
                        &private_field_inits,
                        &private_method_ids,
                        class,
                    );
                    ctor_func_idx = Some(idx);
                } else {
                    let idx = self.lower_function_inner(
                        class_name,
                        &method_fn.params.items,
                        method_fn.body.as_deref(),
                    );
                    ctor_func_idx = Some(idx);
                }
            }
        }

        // If no constructor, create a default one.
        // For derived classes, we skip building a separate constructor body;
        // instead we'll directly use the super constructor as the ctor closure.
        if ctor_func_idx.is_none() && super_proto.is_none() {
            if has_private_fields || has_private_methods {
                // Default constructor with private field installations
                let idx = self.lower_constructor_with_private_fields(
                    class_name,
                    &[],
                    None,
                    &private_field_inits,
                    &private_method_ids,
                    class,
                );
                ctor_func_idx = Some(idx);
            } else {
                let saved_block = self.current_block;
                let saved_terminated = self.terminated;
                let suspended = self.builder.suspend_function();

                // Base class default constructor: empty body
                self.builder
                    .begin_function(class_name, vec![], IrType::JSValue);
                self.function_count += 1;
                let entry = self.builder.create_block();
                self.builder.switch_to_block(entry);
                self.builder.seal_block(entry);
                let undef = self.builder.const_undefined();
                self.builder.ret(Some(undef));
                self.builder.end_function();

                self.builder.resume_function(suspended);
                self.current_block = saved_block;
                self.terminated = saved_terminated;
                ctor_func_idx = Some(self.function_count - 1);
            }
        }

        // For derived classes without explicit constructor, use the super
        // class constructor directly so `new Dog("Rex")` calls Animal("Rex")
        // with `this` = new Dog instance.
        let ctor_closure = if let (None, Some(super_val)) = (ctor_func_idx, super_proto) {
            // The super class itself is already a closure — use it as ctor
            super_val
        } else {
            let ctor_ref = self.builder.const_i32(ctor_func_idx.unwrap_or(0) as i32);
            let ctor_env = self.builder.const_null();
            // Classes are always strict
            let ctor_flags = self.builder.const_i32(2); // is_strict
            let c = self.builder.create_closure(ctor_ref, ctor_env, ctor_flags);
            // Set constructor name and length
            self.emit_function_name_length(c, class_name, ctor_length);
            c
        };

        // Set prototype on constructor
        let proto_key_idx = self.intern_string("prototype");
        let proto_key = self.builder.const_string(proto_key_idx);
        self.builder.set_prop(ctor_closure, proto_key, proto);

        // Collect accessor pairs for class methods (getter/setter with same name
        // and same static-ness). We group them so we can emit a single
        // define_accessor call per accessor key.
        //
        // Key = (method_name, is_static), Value = (getter_index, setter_index)
        let mut class_accessor_map: HashMap<(String, bool), (Option<usize>, Option<usize>)> =
            HashMap::new();
        for (i, element) in class.body.body.iter().enumerate() {
            if let ClassElement::MethodDefinition(method) = element {
                let is_accessor = method.kind == MethodDefinitionKind::Get
                    || method.kind == MethodDefinitionKind::Set;
                if !is_accessor {
                    continue;
                }
                let method_name = match &method.key {
                    PropertyKey::StaticIdentifier(ident) => ident.name.as_str().to_string(),
                    _ => "<computed>".to_string(),
                };
                let map_key = (method_name, method.r#static);
                let entry = class_accessor_map.entry(map_key).or_default();
                if method.kind == MethodDefinitionKind::Get {
                    entry.0 = Some(i);
                } else {
                    entry.1 = Some(i);
                }
            }
        }

        // Lower methods, property definitions, and accessor methods.
        // For accessor methods, we accumulate lowered getter/setter closures
        // and emit define_accessor when we've seen both halves of a pair (or
        // the single accessor if unpaired).
        let mut lowered_class_getters: HashMap<(String, bool), ValueId> = HashMap::new();
        let mut lowered_class_setters: HashMap<(String, bool), ValueId> = HashMap::new();

        for (elem_idx, element) in class.body.body.iter().enumerate() {
            if let ClassElement::MethodDefinition(method) = element {
                if method.kind == MethodDefinitionKind::Constructor {
                    continue;
                }

                // Skip private methods — they are installed per-instance
                // in the constructor via InstallPrivateField.
                if method.key.private_name().is_some() {
                    continue;
                }

                let method_name = match &method.key {
                    PropertyKey::StaticIdentifier(ident) => ident.name.as_str(),
                    _ => "<computed>",
                };

                // Handle getter/setter methods via define_accessor
                if method.kind == MethodDefinitionKind::Get
                    || method.kind == MethodDefinitionKind::Set
                {
                    let method_fn = &method.value;
                    let accessor_func_idx = self.lower_function_inner(
                        method_name,
                        &method_fn.params.items,
                        method_fn.body.as_deref(),
                    );

                    let accessor_ref = self.builder.const_i32(accessor_func_idx as i32);
                    let accessor_env = self.builder.const_null();
                    let accessor_flags = self.builder.const_i32(2); // is_strict
                    let accessor_closure =
                        self.builder
                            .create_closure(accessor_ref, accessor_env, accessor_flags);

                    // Set function.name (e.g., "get bar" or "set bar") and length
                    let prefix = if method.kind == MethodDefinitionKind::Get {
                        "get"
                    } else {
                        "set"
                    };
                    let display_name = format!("{prefix} {method_name}");
                    let fn_length = Self::compute_function_length(
                        &method_fn.params.items,
                        method_fn.params.rest.is_some(),
                    );
                    self.emit_function_name_length(accessor_closure, &display_name, fn_length);

                    let map_key = (method_name.to_string(), method.r#static);
                    if method.kind == MethodDefinitionKind::Get {
                        lowered_class_getters.insert(map_key.clone(), accessor_closure);
                    } else {
                        lowered_class_setters.insert(map_key.clone(), accessor_closure);
                    }

                    // Emit define_accessor when we've processed the last entry for this key
                    let pair = class_accessor_map.get(&map_key);
                    let is_last_for_key = match (method.kind, pair) {
                        (MethodDefinitionKind::Get, Some((_, Some(set_idx)))) => {
                            elem_idx > *set_idx
                        }
                        (MethodDefinitionKind::Get, _) => true,
                        (MethodDefinitionKind::Set, Some((Some(get_idx), _))) => {
                            elem_idx > *get_idx
                        }
                        (MethodDefinitionKind::Set, _) => true,
                        _ => true,
                    };

                    if is_last_for_key {
                        let key_idx = self.intern_string(method_name);
                        let key = self.builder.const_string(key_idx);
                        let target = if method.r#static { ctor_closure } else { proto };
                        self.emit_define_accessor(
                            target,
                            key,
                            lowered_class_getters.get(&map_key).copied(),
                            lowered_class_setters.get(&map_key).copied(),
                        );
                    }
                    continue;
                }

                let method_fn = &method.value;
                let method_func_idx = self.lower_function_inner(
                    method_name,
                    &method_fn.params.items,
                    method_fn.body.as_deref(),
                );

                // Create closure for method (classes are always strict)
                let method_ref = self.builder.const_i32(method_func_idx as i32);
                let method_env = self.builder.const_null();
                let method_flags = self.builder.const_i32(2); // is_strict
                let method_closure =
                    self.builder
                        .create_closure(method_ref, method_env, method_flags);

                // Set function.name and function.length for the method
                let fn_length = Self::compute_function_length(
                    &method_fn.params.items,
                    method_fn.params.rest.is_some(),
                );
                self.emit_function_name_length(method_closure, method_name, fn_length);

                let key_idx = self.intern_string(method_name);
                let key = self.builder.const_string(key_idx);

                // Per ES2024 §14.3.7: class methods must be defined with
                // { writable: true, enumerable: false, configurable: true }.
                // Use __esc_rt_define_method instead of set_prop.
                let rt_name_idx = self.intern_string("__esc_rt_define_method");
                let rt_name = self.builder.const_string(rt_name_idx);
                if method.r#static {
                    // Static methods go on the constructor
                    self.builder
                        .call_runtime(rt_name, vec![ctor_closure, key, method_closure]);
                } else {
                    // Instance methods go on the prototype
                    self.builder
                        .call_runtime(rt_name, vec![proto, key, method_closure]);
                }
            }

            // Handle static and instance property definitions (non-private).
            // Static fields are set on the constructor; instance fields on the prototype.
            // Per spec, static field initializers are evaluated in source order AFTER
            // the class is created. Fields without initializers default to `undefined`.
            // Private fields are skipped here — they are installed per-instance in the constructor.
            if let ClassElement::PropertyDefinition(prop_def) = element {
                // Skip private fields — they are installed per-instance in the constructor
                if prop_def.key.private_name().is_some() {
                    continue;
                }

                let val = if let Some(value_expr) = &prop_def.value {
                    self.lower_expression(value_expr)
                } else {
                    // No initializer — default to undefined
                    self.builder.const_undefined()
                };
                let key_name = match &prop_def.key {
                    PropertyKey::StaticIdentifier(ident) => ident.name.as_str(),
                    _ => "<computed>",
                };
                let key_idx = self.intern_string(key_name);
                let key = self.builder.const_string(key_idx);

                if prop_def.r#static {
                    self.builder.set_prop(ctor_closure, key, val);
                } else {
                    self.builder.set_prop(proto, key, val);
                }
            }

            // Handle static initializer blocks.
            // Per spec, static blocks are evaluated in source order, interleaved
            // with static field initializers. The block body is inlined into the
            // class creation sequence. Inside the block, `this` refers to the
            // constructor (the class itself).
            if let ClassElement::StaticBlock(static_block) = element {
                let saved_this_override = self.this_override;
                self.this_override = Some(ctor_closure);
                for stmt in &static_block.body {
                    if self.block_terminated() {
                        break;
                    }
                    self.lower_statement(stmt);
                }
                self.this_override = saved_this_override;
            }
        }

        // Restore private name IDs from before this class
        self.private_name_ids = saved_private_ids;

        // Bind class name inside the class body scope (for named expressions,
        // this is visible only within; for declarations, the caller will
        // also bind in the outer scope).
        if let Some(id) = &class.id {
            let var = self.scopes.declare(id.name.as_str());
            self.builder.write_variable(var, ctor_closure);
        }

        // Pop the named expression scope so the name is not visible outside
        if is_named_expr {
            self.scopes.pop_scope();
        }

        self.is_strict = saved_is_strict;
        ctor_closure
    }

    /// Lower a constructor function with private field installations injected
    /// at the start of the body (after `this` is available).
    ///
    /// For each private field declaration, emits `InstallPrivateField(this, id, init_value)`.
    /// For each private method, emits `InstallPrivateField(this, id, method_closure)`.
    fn lower_constructor_with_private_fields(
        &mut self,
        name: &str,
        params: &[FormalParameter<'_>],
        body: Option<&FunctionBody<'_>>,
        private_field_inits: &[(u32, Option<usize>)],
        private_method_ids: &[(String, u32)],
        class: &Class<'_>,
    ) -> usize {
        // Save current function's state
        let saved_block = self.current_block;
        let saved_break = self.loop_break_target;
        let saved_continue = self.loop_continue_target;
        let saved_terminated = self.terminated;
        let saved_is_strict = self.is_strict;
        let saved_const_vars = std::mem::take(&mut self.const_vars);
        let saved_tdz_vars = std::mem::take(&mut self.tdz_vars);

        let suspended = self.builder.suspend_function();

        // Build parameter list
        let param_list: Vec<(&str, IrType)> = params
            .iter()
            .map(|p| {
                let param_name = match &p.pattern {
                    BindingPattern::BindingIdentifier(ident) => ident.name.as_str(),
                    _ => "_",
                };
                (param_name, IrType::JSValue)
            })
            .collect();

        self.builder
            .begin_function(name, param_list, IrType::JSValue);
        let func_idx = self.function_count;
        self.function_count += 1;

        let entry = self.builder.create_block();
        self.builder.switch_to_block(entry);
        self.builder.seal_block(entry);
        self.current_block = Some(entry);
        self.loop_break_target = None;
        self.loop_continue_target = None;
        self.terminated = false;

        // Push function scope and bind parameters
        self.scopes.push_scope(ScopeKind::Function);
        for (i, param) in params.iter().enumerate() {
            if let BindingPattern::BindingIdentifier(ident) = &param.pattern {
                let var = self.scopes.declare(ident.name.as_str());
                let val = self.builder.load_param(i as u32);
                self.builder.write_variable(var, val);
            }
        }

        // Get `this` — the newly created instance
        let this_val = self.builder.this_value();

        // Install private fields: emit InstallPrivateField(this, private_id, init_value)
        for &(pid, init_elem_idx) in private_field_inits {
            let private_id = self.builder.const_i32(pid as i32);
            let init_val = if let Some(elem_idx) = init_elem_idx {
                // Evaluate the initializer expression
                if let ClassElement::PropertyDefinition(prop_def) = &class.body.body[elem_idx]
                    && let Some(init_expr) = &prop_def.value
                {
                    self.lower_expression(init_expr)
                } else {
                    self.builder.const_undefined()
                }
            } else {
                self.builder.const_undefined()
            };
            self.builder
                .install_private_field(this_val, private_id, init_val);
        }

        // Install private methods: emit InstallPrivateField(this, private_id, method_closure)
        for (method_name, pid) in private_method_ids {
            let private_id = self.builder.const_i32(*pid as i32);
            // Find and lower the private method
            let method_closure = self.lower_private_method_closure(method_name, class);
            self.builder
                .install_private_field(this_val, private_id, method_closure);
        }

        // Lower the original constructor body (if present)
        if let Some(body) = body {
            // Detect "use strict" (class bodies already are, but check anyway)
            self.is_strict = true;

            // Pre-scan for let/const TDZ names
            let (tdz_names, _const_names) = Self::collect_block_lexical_names(&body.statements);
            for tdz_name in &tdz_names {
                self.tdz_vars.insert(tdz_name.clone());
            }

            for stmt in &body.statements {
                if self.terminated {
                    break;
                }
                self.lower_statement(stmt);
            }
        }

        // Ensure function ends with a return
        if !self.terminated {
            let undef = self.builder.const_undefined();
            self.builder.ret(Some(undef));
        }

        self.scopes.pop_scope();
        self.builder.end_function();
        self.builder.resume_function(suspended);

        // Restore state
        self.current_block = saved_block;
        self.loop_break_target = saved_break;
        self.loop_continue_target = saved_continue;
        self.terminated = saved_terminated;
        self.is_strict = saved_is_strict;
        self.const_vars = saved_const_vars;
        self.tdz_vars = saved_tdz_vars;

        func_idx
    }

    /// Lower a private method from a class body into a closure value.
    fn lower_private_method_closure(&mut self, method_name: &str, class: &Class<'_>) -> ValueId {
        // Find the method definition with the matching private name
        for element in &class.body.body {
            if let ClassElement::MethodDefinition(method) = element
                && let Some(priv_ident) = method.key.private_name()
                && priv_ident.as_str() == method_name
            {
                let method_fn = &method.value;
                let method_func_idx = self.lower_function_inner(
                    method_name,
                    &method_fn.params.items,
                    method_fn.body.as_deref(),
                );
                let method_ref = self.builder.const_i32(method_func_idx as i32);
                let method_env = self.builder.const_null();
                let method_flags = self.builder.const_i32(2); // is_strict
                let closure = self
                    .builder
                    .create_closure(method_ref, method_env, method_flags);
                let fn_length = Self::compute_function_length(
                    &method_fn.params.items,
                    method_fn.params.rest.is_some(),
                );
                self.emit_function_name_length(closure, method_name, fn_length);
                return closure;
            }
        }
        // Fallback: should not happen if private_method_ids was correctly computed
        self.builder.const_undefined()
    }

    /// Compute the `function.length` value for a parameter list.
    ///
    /// Per ECMAScript spec, `function.length` is the number of formal parameters
    /// before the first one with a default value or the rest parameter.
    pub(crate) fn compute_function_length(params: &[FormalParameter<'_>], has_rest: bool) -> u32 {
        let mut count = 0u32;
        for param in params {
            // Stop counting at the first parameter with a default value
            if param.initializer.is_some() {
                break;
            }
            // Stop if this parameter itself is an assignment pattern (destructuring default)
            if matches!(&param.pattern, BindingPattern::AssignmentPattern(_)) {
                break;
            }
            count += 1;
        }
        // Rest parameter is separate from params list, so no adjustment needed
        let _ = has_rest;
        count
    }

    /// Emit `SetProp` calls on a closure to set its `.name` and `.length` properties.
    ///
    /// `name_str` is the inferred or explicit function name. If empty, `.name`
    /// will be an empty string (matching the spec for anonymous functions).
    /// `length` is the value computed by [`compute_function_length`].
    pub(crate) fn emit_function_name_length(
        &mut self,
        closure: ValueId,
        name_str: &str,
        length: u32,
    ) {
        // Set .name — only for named functions/classes. Anonymous functions
        // (empty or "<anonymous>") skip this so maybe_infer_function_name can
        // set the name from the assignment target (SetFunctionName, ES2024 §13.15.5.3).
        if !name_str.is_empty() && name_str != "<anonymous>" {
            let name_key_idx = self.intern_string("name");
            let name_key = self.builder.const_string(name_key_idx);
            let name_val_idx = self.intern_string(name_str);
            let name_val = self.builder.const_string(name_val_idx);
            self.builder.set_prop(closure, name_key, name_val);
        }

        // Set .length
        let length_key_idx = self.intern_string("length");
        let length_key = self.builder.const_string(length_key_idx);
        let length_val = self.builder.const_i32(length as i32);
        self.builder.set_prop(closure, length_key, length_val);
    }
}

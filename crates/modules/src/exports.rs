//! Export and import collection from JavaScript/TypeScript source.
//!
//! Uses `parser::parse_with` to walk the oxc AST and extract
//! import/export declarations into owned representations.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Import types
// ---------------------------------------------------------------------------

/// A single import declaration (e.g., `import { foo, bar } from './mod'`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportEntry {
    /// The import specifier string (e.g., `"./foo"`, `"react"`).
    pub source: String,
    /// Individual bindings imported.
    pub bindings: Vec<ImportBinding>,
    /// Filled after module resolution with the target module's id.
    pub resolved_id: Option<u32>,
}

/// A single binding within an import declaration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportBinding {
    /// Name in the source module (e.g., `"default"`, `"foo"`, `"*"`).
    pub imported: String,
    /// Name in the importing module (the local binding).
    pub local: String,
}

// ---------------------------------------------------------------------------
// Export types
// ---------------------------------------------------------------------------

/// A single exported name from a module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportEntry {
    /// The exported name (e.g., `"foo"`, `"default"`).
    pub name: String,
    /// What kind of export this is.
    pub kind: ExportKind,
}

/// The kind of export.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExportKind {
    /// `export { foo }` or `export const foo = ...`
    Named,
    /// `export default ...`
    Default,
    /// `export { foo } from './bar'` or `export * from './bar'`
    ReExport { source: String },
}

// ---------------------------------------------------------------------------
// Collection from source
// ---------------------------------------------------------------------------

/// Extract imports and exports from JavaScript/TypeScript source code.
///
/// Parses the source as an ES module and walks the top-level statements
/// to find import and export declarations.
pub fn collect_imports_exports(
    source: &str,
    filename: &str,
) -> Result<(Vec<ImportEntry>, Vec<ExportEntry>), String> {
    use oxc_ast::ast::Statement;
    use parser::{detect_source_type, parse_with};

    let source_type = detect_source_type(filename).with_module(true);

    parse_with(source, source_type, |program| {
        let mut imports = Vec::new();
        let mut exports = Vec::new();

        for stmt in &program.body {
            match stmt {
                // --- Import declarations ---
                Statement::ImportDeclaration(decl) => {
                    let source_str = decl.source.value.to_string();
                    let mut bindings = Vec::new();

                    if let Some(specifiers) = &decl.specifiers {
                        for spec in specifiers {
                            use oxc_ast::ast::ImportDeclarationSpecifier;
                            match spec {
                                ImportDeclarationSpecifier::ImportSpecifier(s) => {
                                    bindings.push(ImportBinding {
                                        imported: s.imported.name().to_string(),
                                        local: s.local.name.to_string(),
                                    });
                                }
                                ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                                    bindings.push(ImportBinding {
                                        imported: "default".to_string(),
                                        local: s.local.name.to_string(),
                                    });
                                }
                                ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                                    bindings.push(ImportBinding {
                                        imported: "*".to_string(),
                                        local: s.local.name.to_string(),
                                    });
                                }
                            }
                        }
                    }

                    imports.push(ImportEntry {
                        source: source_str,
                        bindings,
                        resolved_id: None,
                    });
                }

                // --- Export named declarations ---
                Statement::ExportNamedDeclaration(decl) => {
                    let re_export_source = decl.source.as_ref().map(|s| s.value.to_string());

                    // Named specifiers: `export { foo, bar }` or `export { foo } from './mod'`
                    for spec in &decl.specifiers {
                        let name = spec.exported.name().to_string();
                        let kind = match &re_export_source {
                            Some(src) => ExportKind::ReExport {
                                source: String::clone(src),
                            },
                            None => ExportKind::Named,
                        };
                        exports.push(ExportEntry { name, kind });
                    }

                    // Declaration: `export const x = ...`, `export function f() {}`, etc.
                    if let Some(declaration) = &decl.declaration {
                        let names = declaration_names(declaration);
                        for name in names {
                            exports.push(ExportEntry {
                                name,
                                kind: ExportKind::Named,
                            });
                        }
                    }
                }

                // --- Export default ---
                Statement::ExportDefaultDeclaration(_) => {
                    exports.push(ExportEntry {
                        name: "default".to_string(),
                        kind: ExportKind::Default,
                    });
                }

                // --- Export all: `export * from './mod'` ---
                Statement::ExportAllDeclaration(decl) => {
                    let source_str = decl.source.value.to_string();
                    let name = match &decl.exported {
                        Some(exported) => exported.name().to_string(),
                        None => "*".to_string(),
                    };
                    exports.push(ExportEntry {
                        name,
                        kind: ExportKind::ReExport { source: source_str },
                    });
                }

                _ => {}
            }
        }

        (imports, exports)
    })
    .map_err(|errors| {
        errors
            .into_iter()
            .map(|e| e.message)
            .collect::<Vec<_>>()
            .join("; ")
    })
}

/// Extract declared names from a declaration node.
fn declaration_names(decl: &oxc_ast::ast::Declaration<'_>) -> Vec<String> {
    use oxc_ast::ast::Declaration;

    match decl {
        Declaration::VariableDeclaration(var) => {
            let mut names = Vec::new();
            for declarator in &var.declarations {
                collect_binding_names(&declarator.id, &mut names);
            }
            names
        }
        Declaration::FunctionDeclaration(f) => {
            if let Some(id) = &f.id {
                vec![id.name.to_string()]
            } else {
                vec![]
            }
        }
        Declaration::ClassDeclaration(c) => {
            if let Some(id) = &c.id {
                vec![id.name.to_string()]
            } else {
                vec![]
            }
        }
        // TS type exports — we still record them
        Declaration::TSTypeAliasDeclaration(t) => {
            vec![t.id.name.to_string()]
        }
        Declaration::TSInterfaceDeclaration(i) => {
            vec![i.id.name.to_string()]
        }
        Declaration::TSEnumDeclaration(e) => {
            vec![e.id.name.to_string()]
        }
        _ => vec![],
    }
}

/// Collect dynamic import specifiers from JavaScript/TypeScript source code.
///
/// Walks the entire AST looking for `import("./mod.js")` expressions where
/// the argument is a string literal or a template literal with no
/// interpolations. Returns the list of discovered specifier strings.
pub fn collect_dynamic_imports(source: &str, filename: &str) -> Result<Vec<String>, String> {
    use parser::{detect_source_type, parse_with};

    let source_type = detect_source_type(filename).with_module(true);

    parse_with(source, source_type, |program| {
        let mut specifiers = Vec::new();
        for stmt in &program.body {
            collect_dynamic_imports_from_stmt(stmt, &mut specifiers);
        }
        specifiers
    })
    .map_err(|errors| {
        errors
            .into_iter()
            .map(|e| e.message)
            .collect::<Vec<_>>()
            .join("; ")
    })
}

/// Walk a statement recursively to find `import()` calls with string literal args.
fn collect_dynamic_imports_from_stmt(
    stmt: &oxc_ast::ast::Statement<'_>,
    specifiers: &mut Vec<String>,
) {
    use oxc_ast::ast::Statement;

    match stmt {
        Statement::ExpressionStatement(expr) => {
            collect_dynamic_imports_from_expr(&expr.expression, specifiers);
        }
        Statement::BlockStatement(block) => {
            for s in &block.body {
                collect_dynamic_imports_from_stmt(s, specifiers);
            }
        }
        Statement::IfStatement(ifs) => {
            collect_dynamic_imports_from_expr(&ifs.test, specifiers);
            collect_dynamic_imports_from_stmt(&ifs.consequent, specifiers);
            if let Some(alt) = &ifs.alternate {
                collect_dynamic_imports_from_stmt(alt, specifiers);
            }
        }
        Statement::ReturnStatement(ret) => {
            if let Some(arg) = &ret.argument {
                collect_dynamic_imports_from_expr(arg, specifiers);
            }
        }
        Statement::VariableDeclaration(decl) => {
            for declarator in &decl.declarations {
                if let Some(init) = &declarator.init {
                    collect_dynamic_imports_from_expr(init, specifiers);
                }
            }
        }
        Statement::ForStatement(f) => {
            if let Some(body) = Some(&f.body) {
                collect_dynamic_imports_from_stmt(body, specifiers);
            }
        }
        Statement::WhileStatement(w) => {
            collect_dynamic_imports_from_stmt(&w.body, specifiers);
        }
        Statement::TryStatement(t) => {
            for s in &t.block.body {
                collect_dynamic_imports_from_stmt(s, specifiers);
            }
            if let Some(handler) = &t.handler {
                for s in &handler.body.body {
                    collect_dynamic_imports_from_stmt(s, specifiers);
                }
            }
            if let Some(finalizer) = &t.finalizer {
                for s in &finalizer.body {
                    collect_dynamic_imports_from_stmt(s, specifiers);
                }
            }
        }
        Statement::SwitchStatement(sw) => {
            for case in &sw.cases {
                for s in &case.consequent {
                    collect_dynamic_imports_from_stmt(s, specifiers);
                }
            }
        }
        Statement::DoWhileStatement(dw) => {
            collect_dynamic_imports_from_stmt(&dw.body, specifiers);
        }
        Statement::ForInStatement(f) => {
            collect_dynamic_imports_from_stmt(&f.body, specifiers);
        }
        Statement::ForOfStatement(f) => {
            collect_dynamic_imports_from_stmt(&f.body, specifiers);
        }
        Statement::LabeledStatement(l) => {
            collect_dynamic_imports_from_stmt(&l.body, specifiers);
        }
        Statement::ThrowStatement(t) => {
            collect_dynamic_imports_from_expr(&t.argument, specifiers);
        }
        // Walk into declarations that may contain function bodies
        _ => {
            collect_dynamic_imports_from_declaration(stmt, specifiers);
        }
    }
}

/// Walk declarations (function, class, export) to find nested import() calls.
fn collect_dynamic_imports_from_declaration(
    stmt: &oxc_ast::ast::Statement<'_>,
    specifiers: &mut Vec<String>,
) {
    use oxc_ast::ast::Statement;

    match stmt {
        Statement::FunctionDeclaration(func) => {
            if let Some(body) = &func.body {
                for s in &body.statements {
                    collect_dynamic_imports_from_stmt(s, specifiers);
                }
            }
        }
        Statement::ClassDeclaration(class) => {
            for elem in &class.body.body {
                if let oxc_ast::ast::ClassElement::MethodDefinition(method) = elem
                    && let Some(body) = &method.value.body
                {
                    for s in &body.statements {
                        collect_dynamic_imports_from_stmt(s, specifiers);
                    }
                }
            }
        }
        Statement::ExportDefaultDeclaration(decl) => match &decl.declaration {
            oxc_ast::ast::ExportDefaultDeclarationKind::FunctionDeclaration(func) => {
                if let Some(body) = &func.body {
                    for s in &body.statements {
                        collect_dynamic_imports_from_stmt(s, specifiers);
                    }
                }
            }
            _ => {
                if let Some(expr) = decl.declaration.as_expression() {
                    collect_dynamic_imports_from_expr(expr, specifiers);
                }
            }
        },
        Statement::ExportNamedDeclaration(decl) => {
            if let Some(oxc_ast::ast::Declaration::FunctionDeclaration(func)) = &decl.declaration
                && let Some(body) = &func.body
            {
                for s in &body.statements {
                    collect_dynamic_imports_from_stmt(s, specifiers);
                }
            }
        }
        _ => {}
    }
}

/// Walk an expression to find `import()` calls with string literal specifiers.
fn collect_dynamic_imports_from_expr(
    expr: &oxc_ast::ast::Expression<'_>,
    specifiers: &mut Vec<String>,
) {
    use oxc_ast::ast::Expression;

    match expr {
        Expression::ImportExpression(import_expr) => {
            // Extract string literal specifier
            match &import_expr.source {
                Expression::StringLiteral(lit) => {
                    let spec = lit.value.to_string();
                    if !specifiers.contains(&spec) {
                        specifiers.push(spec);
                    }
                }
                Expression::TemplateLiteral(tmpl) if tmpl.expressions.is_empty() => {
                    let mut s = String::new();
                    for quasi in &tmpl.quasis {
                        s.push_str(&quasi.value.raw);
                    }
                    if !specifiers.contains(&s) {
                        specifiers.push(s);
                    }
                }
                _ => {} // Non-literal — not discovered at compile time
            }
        }
        Expression::CallExpression(call) => {
            collect_dynamic_imports_from_expr(&call.callee, specifiers);
            for arg in &call.arguments {
                if let Some(e) = arg.as_expression() {
                    collect_dynamic_imports_from_expr(e, specifiers);
                }
            }
        }
        Expression::AssignmentExpression(a) => {
            collect_dynamic_imports_from_expr(&a.right, specifiers);
        }
        Expression::BinaryExpression(b) => {
            collect_dynamic_imports_from_expr(&b.left, specifiers);
            collect_dynamic_imports_from_expr(&b.right, specifiers);
        }
        Expression::LogicalExpression(l) => {
            collect_dynamic_imports_from_expr(&l.left, specifiers);
            collect_dynamic_imports_from_expr(&l.right, specifiers);
        }
        Expression::ConditionalExpression(c) => {
            collect_dynamic_imports_from_expr(&c.test, specifiers);
            collect_dynamic_imports_from_expr(&c.consequent, specifiers);
            collect_dynamic_imports_from_expr(&c.alternate, specifiers);
        }
        Expression::SequenceExpression(s) => {
            for e in &s.expressions {
                collect_dynamic_imports_from_expr(e, specifiers);
            }
        }
        Expression::ArrowFunctionExpression(arrow) => {
            for s in &arrow.body.statements {
                collect_dynamic_imports_from_stmt(s, specifiers);
            }
        }
        Expression::FunctionExpression(func) => {
            if let Some(body) = &func.body {
                for s in &body.statements {
                    collect_dynamic_imports_from_stmt(s, specifiers);
                }
            }
        }
        Expression::AwaitExpression(a) => {
            collect_dynamic_imports_from_expr(&a.argument, specifiers);
        }
        Expression::ParenthesizedExpression(p) => {
            collect_dynamic_imports_from_expr(&p.expression, specifiers);
        }
        Expression::ArrayExpression(arr) => {
            for elem in &arr.elements {
                if let Some(e) = elem.as_expression() {
                    collect_dynamic_imports_from_expr(e, specifiers);
                }
            }
        }
        _ => {}
    }
}

/// Recursively collect binding names from a binding pattern.
fn collect_binding_names(pattern: &oxc_ast::ast::BindingPattern<'_>, names: &mut Vec<String>) {
    use oxc_ast::ast::BindingPattern;
    match pattern {
        BindingPattern::BindingIdentifier(id) => {
            names.push(id.name.to_string());
        }
        BindingPattern::ObjectPattern(obj) => {
            for prop in &obj.properties {
                collect_binding_names(&prop.value, names);
            }
            if let Some(rest) = &obj.rest {
                collect_binding_names(&rest.argument, names);
            }
        }
        BindingPattern::ArrayPattern(arr) => {
            for pat in arr.elements.iter().flatten() {
                collect_binding_names(pat, names);
            }
            if let Some(rest) = &arr.rest {
                collect_binding_names(&rest.argument, names);
            }
        }
        BindingPattern::AssignmentPattern(assign) => {
            collect_binding_names(&assign.left, names);
        }
    }
}

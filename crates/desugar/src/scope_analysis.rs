//! Scope analysis pre-pass for variable classification.
//!
//! Walks the oxc AST before IR lowering to build a scope tree and classify
//! every variable declaration. The result is a [`ScopeAnalysis`] struct that
//! answers "where should this variable live?" for any variable in the program.
//!
//! This pass runs independently of `IrLowerer` and produces data that future
//! phases (v0.3 closures) will consume. For now, it classifies all variables
//! as [`VariableLocation::Stack`] by default; closure-based promotion to
//! [`VariableLocation::Environment`] will be added later.

use std::collections::HashMap;

use oxc_ast::ast::{
    BindingPattern, Expression, FormalParameter, FunctionBody, ObjectPropertyKind, Statement,
    VariableDeclarationKind,
};

use crate::scope::{ScopeKind, VariableLocation};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Unique identifier for a scope in the scope tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScopeId(u32);

impl ScopeId {
    /// The root scope (global or module level).
    pub const ROOT: ScopeId = ScopeId(0);

    /// Construct a `ScopeId` from a raw index.
    ///
    /// Used primarily in tests to iterate over scopes.
    pub fn from_raw(index: u32) -> Self {
        ScopeId(index)
    }
}

/// Unique identifier for a variable declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VarId(u32);

/// How a variable was declared in source code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclarationKind {
    /// `var` — hoisted to enclosing function/global/module scope.
    Var,
    /// `let` — block-scoped.
    Let,
    /// `const` — block-scoped, immutable binding.
    Const,
    /// Function declaration (`function foo() { ... }`).
    Function,
    /// Function/method parameter.
    Param,
    /// Class declaration (`class Foo { ... }`).
    Class,
    /// Catch clause parameter (`catch (e) { ... }`).
    CatchParam,
}

/// Information about a single variable declaration.
#[derive(Debug, Clone)]
pub struct VarInfo {
    /// The variable's name as it appears in source.
    pub name: String,
    /// How the variable was declared.
    pub kind: DeclarationKind,
    /// Which scope owns this declaration (after hoisting for `var`).
    pub scope: ScopeId,
    /// Where the variable will be stored at runtime.
    pub location: VariableLocation,
    /// Whether this variable is referenced from a nested function scope.
    pub is_captured: bool,
    /// Whether this variable is ever assigned to after its declaration.
    pub is_mutated: bool,
}

/// Flags tracking dynamic scope poisoning for `eval` and `with`.
///
/// When a scope contains a direct `eval()` call or a `with` statement, the
/// containing function must use a dynamic environment (`EscEnvironment`)
/// instead of SSA variables, because variable bindings cannot be statically
/// resolved.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScopeFlags {
    /// This scope directly contains a `eval()` call (unqualified, non-shadowed).
    pub calls_eval: bool,
    /// An inner (non-function) scope within the same function calls eval.
    pub inner_calls_eval: bool,
    /// This scope directly contains a `with` statement.
    pub contains_with: bool,
    /// Computed: this function needs an `EscEnvironment` for dynamic lookups.
    ///
    /// Set to `true` when `calls_eval`, `inner_calls_eval`, or `contains_with`
    /// is true on a function (or global/module) scope boundary.
    pub needs_dynamic_env: bool,
}

/// A scope node in the scope tree.
#[derive(Debug, Clone)]
pub struct ScopeNode {
    /// What kind of scope this is.
    pub kind: ScopeKind,
    /// Parent scope (None only for the root).
    pub parent: Option<ScopeId>,
    /// Variables declared in this scope.
    pub variables: HashMap<String, VarId>,
    /// Direct child scopes.
    pub children: Vec<ScopeId>,
    /// Depth from root (root = 0).
    pub depth: u32,
    /// Dynamic scope poisoning flags (eval/with detection).
    pub flags: ScopeFlags,
}

/// Result of the scope analysis pre-pass.
///
/// Contains the full scope tree and variable table for a program. Query
/// with [`location_of`](ScopeAnalysis::location_of) to determine where
/// a variable should be stored at runtime.
#[derive(Debug)]
pub struct ScopeAnalysis {
    scopes: Vec<ScopeNode>,
    variables: Vec<VarInfo>,
    /// Maps (scope_id, name) to var_id for efficient lookups during the walk.
    lookup: HashMap<(ScopeId, String), VarId>,
}

impl ScopeAnalysis {
    /// Return the runtime storage location for a variable.
    pub fn location_of(&self, var: VarId) -> VariableLocation {
        self.variables[var.0 as usize].location
    }

    /// Return full information about a variable.
    pub fn var_info(&self, var: VarId) -> &VarInfo {
        &self.variables[var.0 as usize]
    }

    /// Return a scope node by its id.
    pub fn scope(&self, id: ScopeId) -> &ScopeNode {
        &self.scopes[id.0 as usize]
    }

    /// Return the total number of scopes in the tree.
    pub fn scope_count(&self) -> usize {
        self.scopes.len()
    }

    /// Return the total number of declared variables.
    pub fn variable_count(&self) -> usize {
        self.variables.len()
    }

    /// Check whether the function scope containing `scope_id` needs a
    /// dynamic environment (`EscEnvironment`) due to `eval` or `with`.
    pub fn needs_dynamic_env(&self, scope_id: ScopeId) -> bool {
        let fn_scope = self.enclosing_function_scope(scope_id);
        self.scopes[fn_scope.0 as usize].flags.needs_dynamic_env
    }

    /// Return the flags for a given scope.
    pub fn scope_flags(&self, id: ScopeId) -> &ScopeFlags {
        &self.scopes[id.0 as usize].flags
    }

    /// Find the nearest enclosing function (or global/module) scope.
    fn enclosing_function_scope(&self, from: ScopeId) -> ScopeId {
        let mut id = from;
        loop {
            let kind = self.scopes[id.0 as usize].kind;
            if matches!(
                kind,
                ScopeKind::Function | ScopeKind::Global | ScopeKind::Module
            ) {
                return id;
            }
            match self.scopes[id.0 as usize].parent {
                Some(parent) => id = parent,
                None => return id,
            }
        }
    }

    /// Resolve a variable name starting from the given scope, walking up
    /// the scope chain. Returns the [`VarId`] if found.
    pub fn resolve(&self, name: &str, from_scope: ScopeId) -> Option<VarId> {
        let mut scope_id = Some(from_scope);
        while let Some(sid) = scope_id {
            if let Some(&var_id) = self.lookup.get(&(sid, name.to_string())) {
                return Some(var_id);
            }
            scope_id = self.scopes[sid.0 as usize].parent;
        }
        None
    }

    /// Check whether a variable is captured by any nested function.
    pub fn is_captured(&self, var: VarId) -> bool {
        self.variables[var.0 as usize].is_captured
    }

    /// Return the root scope id.
    pub fn root_scope(&self) -> ScopeId {
        ScopeId::ROOT
    }
}

// ---------------------------------------------------------------------------
// Builder (used only during the analysis walk)
// ---------------------------------------------------------------------------

/// Mutable builder for constructing the scope tree during the AST walk.
struct ScopeBuilder {
    scopes: Vec<ScopeNode>,
    variables: Vec<VarInfo>,
    lookup: HashMap<(ScopeId, String), VarId>,
    current: ScopeId,
    /// Whether this program is an ES module (always strict mode).
    is_module: bool,
    /// Tracks "use strict" directives per function.
    /// Maps function scope ID to true if that function has "use strict".
    strict_functions: HashMap<ScopeId, bool>,
    /// Per-scope cursor tracking which child has been visited next during
    /// the reference walk (phase 2). Maps parent scope → next child index.
    child_cursors: HashMap<ScopeId, usize>,
}

impl ScopeBuilder {
    fn new(root_kind: ScopeKind) -> Self {
        let root = ScopeNode {
            kind: root_kind,
            parent: None,
            variables: HashMap::new(),
            children: Vec::new(),
            depth: 0,
            flags: ScopeFlags::default(),
        };
        Self {
            scopes: vec![root],
            variables: Vec::new(),
            lookup: HashMap::new(),
            current: ScopeId::ROOT,
            is_module: false,
            strict_functions: HashMap::new(),
            child_cursors: HashMap::new(),
        }
    }

    /// Push a new child scope under the current scope and enter it.
    fn push_scope(&mut self, kind: ScopeKind) -> ScopeId {
        let parent_depth = self.scopes[self.current.0 as usize].depth;
        let id = ScopeId(self.scopes.len() as u32);
        self.scopes.push(ScopeNode {
            kind,
            parent: Some(self.current),
            variables: HashMap::new(),
            children: Vec::new(),
            depth: parent_depth + 1,
            flags: ScopeFlags::default(),
        });
        self.scopes[self.current.0 as usize].children.push(id);
        self.current = id;
        id
    }

    /// Pop the current scope and return to the parent.
    fn pop_scope(&mut self) {
        if let Some(parent) = self.scopes[self.current.0 as usize].parent {
            self.current = parent;
        }
    }

    /// Declare a variable in a specific scope.
    fn declare_in(&mut self, scope: ScopeId, name: &str, kind: DeclarationKind) -> VarId {
        // If already declared in this scope, return the existing var.
        if let Some(&var_id) = self.lookup.get(&(scope, name.to_string())) {
            return var_id;
        }

        let var_id = VarId(self.variables.len() as u32);
        self.variables.push(VarInfo {
            name: name.to_string(),
            kind,
            scope,
            location: VariableLocation::Stack,
            is_captured: false,
            is_mutated: false,
        });
        self.scopes[scope.0 as usize]
            .variables
            .insert(name.to_string(), var_id);
        self.lookup.insert((scope, name.to_string()), var_id);
        var_id
    }

    /// Declare a variable in the current scope.
    fn declare(&mut self, name: &str, kind: DeclarationKind) -> VarId {
        self.declare_in(self.current, name, kind)
    }

    /// Declare a `var` in the nearest enclosing var-scope (function/global/module).
    fn declare_var_hoisted(&mut self, name: &str) -> VarId {
        let target = self.find_var_scope(self.current);
        self.declare_in(target, name, DeclarationKind::Var)
    }

    /// Find the nearest enclosing scope that acts as a `var` boundary.
    fn find_var_scope(&self, from: ScopeId) -> ScopeId {
        let mut id = from;
        loop {
            if self.scopes[id.0 as usize].kind.is_var_scope() {
                return id;
            }
            match self.scopes[id.0 as usize].parent {
                Some(parent) => id = parent,
                None => return id,
            }
        }
    }

    /// Find the nearest enclosing function (or global/module) scope.
    fn find_function_scope(&self, from: ScopeId) -> ScopeId {
        let mut id = from;
        loop {
            let kind = self.scopes[id.0 as usize].kind;
            if matches!(
                kind,
                ScopeKind::Function | ScopeKind::Global | ScopeKind::Module
            ) {
                return id;
            }
            match self.scopes[id.0 as usize].parent {
                Some(parent) => id = parent,
                None => return id,
            }
        }
    }

    /// Resolve a variable name from the current scope upward.
    fn resolve(&self, name: &str) -> Option<VarId> {
        self.resolve_from(name, self.current)
    }

    /// Resolve a variable name starting from a specific scope.
    fn resolve_from(&self, name: &str, from: ScopeId) -> Option<VarId> {
        let mut scope_id = Some(from);
        while let Some(sid) = scope_id {
            if let Some(&var_id) = self.lookup.get(&(sid, name.to_string())) {
                return Some(var_id);
            }
            scope_id = self.scopes[sid.0 as usize].parent;
        }
        None
    }

    /// Check if a scope is strictly inside a function boundary relative
    /// to the scope that owns the given variable.
    fn is_across_function_boundary(&self, reference_scope: ScopeId, var_scope: ScopeId) -> bool {
        let ref_fn = self.find_function_scope(reference_scope);
        let var_fn = self.find_function_scope(var_scope);
        ref_fn != var_fn
    }

    /// Mark a variable as captured.
    fn mark_captured(&mut self, var: VarId) {
        self.variables[var.0 as usize].is_captured = true;
    }

    /// Mark the current scope as containing a direct `eval()` call.
    fn mark_calls_eval(&mut self) {
        self.scopes[self.current.0 as usize].flags.calls_eval = true;
    }

    /// Mark the current scope as containing a `with` statement.
    fn mark_contains_with(&mut self) {
        self.scopes[self.current.0 as usize].flags.contains_with = true;
    }

    /// Check whether `eval` is locally bound (shadowed) in any scope from
    /// the current scope up to (and including) the enclosing function boundary.
    ///
    /// If `eval` is bound as a variable (via `var eval`, `let eval`, `const eval`,
    /// or as a parameter), then `eval(...)` is NOT a direct eval call.
    fn is_eval_shadowed(&self) -> bool {
        let mut id = self.current;
        loop {
            if self.lookup.contains_key(&(id, "eval".to_string())) {
                return true;
            }
            let kind = self.scopes[id.0 as usize].kind;
            if matches!(
                kind,
                ScopeKind::Function | ScopeKind::Global | ScopeKind::Module
            ) {
                // Reached function boundary — eval is not shadowed locally.
                return false;
            }
            match self.scopes[id.0 as usize].parent {
                Some(parent) => id = parent,
                None => return false,
            }
        }
    }

    /// Finish building and return the immutable analysis result.
    fn finish(self) -> ScopeAnalysis {
        ScopeAnalysis {
            scopes: self.scopes,
            variables: self.variables,
            lookup: self.lookup,
        }
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Run scope analysis on a parsed program.
///
/// Walks the AST to build a scope tree, classify all variable declarations,
/// and detect cross-function captures. Call this before IR lowering.
///
/// `is_module` should be `true` for ES module source (always strict mode).
pub fn analyze_scopes(program: &oxc_ast::ast::Program<'_>, is_module: bool) -> ScopeAnalysis {
    let root_kind = if is_module {
        ScopeKind::Module
    } else {
        ScopeKind::Global
    };
    let mut builder = ScopeBuilder::new(root_kind);
    builder.is_module = is_module;

    // Phase 1: Walk declarations to build scope tree and declare variables.
    // Also detects `with` statements during declaration walk.
    for stmt in &program.body {
        walk_statement_decls(&mut builder, stmt);
    }

    // Phase 2: Walk references to detect captures AND direct eval calls.
    // Reset to root scope and child cursors for the reference walk.
    builder.current = ScopeId::ROOT;
    builder.child_cursors.clear();
    for stmt in &program.body {
        walk_statement_refs(&mut builder, stmt);
    }

    // Phase 2.5: Propagate eval/with flags upward and compute needs_dynamic_env.
    propagate_poisoning(&mut builder);

    // Phase 3: Assign locations based on capture + mutation + poisoning info.
    assign_locations(&mut builder);

    builder.finish()
}

// ---------------------------------------------------------------------------
// Phase 1: Declaration walk
// ---------------------------------------------------------------------------

/// Walk a statement to discover and register variable declarations.
fn walk_statement_decls(b: &mut ScopeBuilder, stmt: &Statement<'_>) {
    match stmt {
        Statement::VariableDeclaration(decl) => {
            for declarator in &decl.declarations {
                let names = collect_binding_names(&declarator.id);
                for name in names {
                    match decl.kind {
                        VariableDeclarationKind::Var => {
                            b.declare_var_hoisted(&name);
                        }
                        VariableDeclarationKind::Let
                        | VariableDeclarationKind::Using
                        | VariableDeclarationKind::AwaitUsing => {
                            b.declare(&name, DeclarationKind::Let);
                        }
                        VariableDeclarationKind::Const => {
                            b.declare(&name, DeclarationKind::Const);
                        }
                    }
                }
                // Walk initializer expressions for nested functions/classes
                if let Some(init) = &declarator.init {
                    walk_expression_decls(b, init);
                }
            }
        }

        Statement::FunctionDeclaration(func) => {
            // Function declarations are hoisted to the enclosing var-scope.
            if let Some(id) = &func.id {
                let var_scope = b.find_var_scope(b.current);
                b.declare_in(var_scope, id.name.as_str(), DeclarationKind::Function);
            }
            // Walk the function body in a new function scope.
            if let Some(body) = &func.body {
                walk_function_body_decls(b, &func.params.items, body);
            }
        }

        Statement::ClassDeclaration(class) => {
            if let Some(id) = &class.id {
                b.declare(id.name.as_str(), DeclarationKind::Class);
            }
            // Walk class body for method scopes
            walk_class_body_decls(b, class);
        }

        Statement::BlockStatement(block) => {
            b.push_scope(ScopeKind::Block);
            for s in &block.body {
                walk_statement_decls(b, s);
            }
            b.pop_scope();
        }

        Statement::IfStatement(if_stmt) => {
            walk_statement_decls(b, &if_stmt.consequent);
            if let Some(alt) = &if_stmt.alternate {
                walk_statement_decls(b, alt);
            }
        }

        Statement::WhileStatement(w) => {
            walk_statement_decls(b, &w.body);
        }

        Statement::DoWhileStatement(d) => {
            walk_statement_decls(b, &d.body);
        }

        Statement::ForStatement(f) => {
            // for-statement may have its own block scope for `let`/`const` init.
            b.push_scope(ScopeKind::Block);
            if let Some(init) = &f.init {
                match init {
                    oxc_ast::ast::ForStatementInit::VariableDeclaration(decl) => {
                        for declarator in &decl.declarations {
                            let names = collect_binding_names(&declarator.id);
                            for name in names {
                                match decl.kind {
                                    VariableDeclarationKind::Var => {
                                        b.declare_var_hoisted(&name);
                                    }
                                    VariableDeclarationKind::Let
                                    | VariableDeclarationKind::Using
                                    | VariableDeclarationKind::AwaitUsing => {
                                        b.declare(&name, DeclarationKind::Let);
                                    }
                                    VariableDeclarationKind::Const => {
                                        b.declare(&name, DeclarationKind::Const);
                                    }
                                }
                            }
                            if let Some(init_expr) = &declarator.init {
                                walk_expression_decls(b, init_expr);
                            }
                        }
                    }
                    _ => {
                        if let Some(expr) = init.as_expression() {
                            walk_expression_decls(b, expr);
                        }
                    }
                }
            }
            walk_statement_decls(b, &f.body);
            b.pop_scope();
        }

        Statement::ForInStatement(fi) => {
            b.push_scope(ScopeKind::Block);
            if let oxc_ast::ast::ForStatementLeft::VariableDeclaration(decl) = &fi.left {
                for declarator in &decl.declarations {
                    let names = collect_binding_names(&declarator.id);
                    for name in names {
                        match decl.kind {
                            VariableDeclarationKind::Var => {
                                b.declare_var_hoisted(&name);
                            }
                            _ => {
                                b.declare(&name, DeclarationKind::Let);
                            }
                        }
                    }
                }
            }
            walk_expression_decls(b, &fi.right);
            walk_statement_decls(b, &fi.body);
            b.pop_scope();
        }

        Statement::ForOfStatement(fo) => {
            b.push_scope(ScopeKind::Block);
            if let oxc_ast::ast::ForStatementLeft::VariableDeclaration(decl) = &fo.left {
                for declarator in &decl.declarations {
                    let names = collect_binding_names(&declarator.id);
                    for name in names {
                        match decl.kind {
                            VariableDeclarationKind::Var => {
                                b.declare_var_hoisted(&name);
                            }
                            _ => {
                                b.declare(&name, DeclarationKind::Let);
                            }
                        }
                    }
                }
            }
            walk_expression_decls(b, &fo.right);
            walk_statement_decls(b, &fo.body);
            b.pop_scope();
        }

        Statement::SwitchStatement(sw) => {
            // Switch body shares a single block scope for all cases.
            b.push_scope(ScopeKind::Block);
            for case in &sw.cases {
                for s in &case.consequent {
                    walk_statement_decls(b, s);
                }
            }
            b.pop_scope();
        }

        Statement::TryStatement(try_stmt) => {
            // Try block
            b.push_scope(ScopeKind::Block);
            for s in &try_stmt.block.body {
                walk_statement_decls(b, s);
            }
            b.pop_scope();

            // Catch clause — uses Catch scope kind for the error binding
            if let Some(handler) = &try_stmt.handler {
                b.push_scope(ScopeKind::Catch);
                if let Some(param) = &handler.param {
                    let names = collect_binding_names(&param.pattern);
                    for name in names {
                        b.declare(&name, DeclarationKind::CatchParam);
                    }
                }
                for s in &handler.body.body {
                    walk_statement_decls(b, s);
                }
                b.pop_scope();
            }

            // Finally block
            if let Some(finalizer) = &try_stmt.finalizer {
                b.push_scope(ScopeKind::Block);
                for s in &finalizer.body {
                    walk_statement_decls(b, s);
                }
                b.pop_scope();
            }
        }

        Statement::WithStatement(with_stmt) => {
            // Mark the current scope as containing a `with` statement.
            // This poisons the enclosing function scope.
            b.mark_contains_with();
            walk_expression_decls(b, &with_stmt.object);
            b.push_scope(ScopeKind::With);
            walk_statement_decls(b, &with_stmt.body);
            b.pop_scope();
        }

        Statement::LabeledStatement(l) => {
            walk_statement_decls(b, &l.body);
        }

        Statement::ExpressionStatement(expr) => {
            walk_expression_decls(b, &expr.expression);
        }

        Statement::ReturnStatement(ret) => {
            if let Some(arg) = &ret.argument {
                walk_expression_decls(b, arg);
            }
        }

        Statement::ThrowStatement(t) => {
            walk_expression_decls(b, &t.argument);
        }

        // Import/export declarations don't introduce new scopes but may
        // bind names — handled at the module level by the lowerer.
        _ => {}
    }
}

/// Walk an expression to discover nested function/class scopes.
fn walk_expression_decls(b: &mut ScopeBuilder, expr: &Expression<'_>) {
    match expr {
        Expression::FunctionExpression(func) => {
            if let Some(body) = &func.body {
                walk_function_body_decls(b, &func.params.items, body);
            }
        }

        Expression::ArrowFunctionExpression(arrow) => {
            walk_function_body_decls(b, &arrow.params.items, &arrow.body);
        }

        Expression::ClassExpression(class) => {
            walk_class_body_decls(b, class);
        }

        // Walk children of compound expressions
        Expression::AssignmentExpression(a) => {
            walk_expression_decls(b, &a.right);
        }
        Expression::BinaryExpression(bin) => {
            walk_expression_decls(b, &bin.left);
            walk_expression_decls(b, &bin.right);
        }
        Expression::LogicalExpression(l) => {
            walk_expression_decls(b, &l.left);
            walk_expression_decls(b, &l.right);
        }
        Expression::UnaryExpression(u) => {
            walk_expression_decls(b, &u.argument);
        }
        Expression::ConditionalExpression(c) => {
            walk_expression_decls(b, &c.test);
            walk_expression_decls(b, &c.consequent);
            walk_expression_decls(b, &c.alternate);
        }
        Expression::CallExpression(call) => {
            walk_expression_decls(b, &call.callee);
            for arg in &call.arguments {
                if let Some(e) = arg.as_expression() {
                    walk_expression_decls(b, e);
                }
            }
        }
        Expression::NewExpression(n) => {
            walk_expression_decls(b, &n.callee);
            for arg in &n.arguments {
                if let Some(e) = arg.as_expression() {
                    walk_expression_decls(b, e);
                }
            }
        }
        Expression::ArrayExpression(arr) => {
            for elem in &arr.elements {
                if let Some(e) = elem.as_expression() {
                    walk_expression_decls(b, e);
                }
            }
        }
        Expression::ObjectExpression(obj) => {
            for prop in &obj.properties {
                if let ObjectPropertyKind::ObjectProperty(p) = prop {
                    walk_expression_decls(b, &p.value);
                }
            }
        }
        Expression::SequenceExpression(s) => {
            for e in &s.expressions {
                walk_expression_decls(b, e);
            }
        }
        Expression::TemplateLiteral(t) => {
            for e in &t.expressions {
                walk_expression_decls(b, e);
            }
        }
        Expression::ParenthesizedExpression(p) => {
            walk_expression_decls(b, &p.expression);
        }
        Expression::StaticMemberExpression(m) => {
            walk_expression_decls(b, &m.object);
        }
        Expression::ComputedMemberExpression(m) => {
            walk_expression_decls(b, &m.object);
            walk_expression_decls(b, &m.expression);
        }
        Expression::AwaitExpression(a) => {
            walk_expression_decls(b, &a.argument);
        }
        Expression::YieldExpression(y) => {
            if let Some(arg) = &y.argument {
                walk_expression_decls(b, arg);
            }
        }
        _ => {}
    }
}

/// Walk a function body (parameters + body statements) in a new Function scope.
fn walk_function_body_decls(
    b: &mut ScopeBuilder,
    params: &[FormalParameter<'_>],
    body: &FunctionBody<'_>,
) {
    let fn_scope = b.push_scope(ScopeKind::Function);

    // Detect "use strict" directive in this function body.
    for directive in &body.directives {
        if directive.directive.as_str() == "use strict" {
            b.strict_functions.insert(fn_scope, true);
            break;
        }
    }

    // Declare parameters
    for param in params {
        let names = collect_binding_names(&param.pattern);
        for name in names {
            b.declare(&name, DeclarationKind::Param);
        }
    }

    // Walk body statements
    for stmt in &body.statements {
        walk_statement_decls(b, stmt);
    }

    b.pop_scope();
}

/// Walk a class body to discover method/constructor scopes.
fn walk_class_body_decls(b: &mut ScopeBuilder, class: &oxc_ast::ast::Class<'_>) {
    for element in &class.body.body {
        if let oxc_ast::ast::ClassElement::MethodDefinition(method) = element {
            let func = &method.value;
            if let Some(body) = &func.body {
                walk_function_body_decls(b, &func.params.items, body);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 2: Reference walk (detect captures)
// ---------------------------------------------------------------------------

/// Walk statements to find variable references and detect captures.
fn walk_statement_refs(b: &mut ScopeBuilder, stmt: &Statement<'_>) {
    match stmt {
        Statement::VariableDeclaration(decl) => {
            for declarator in &decl.declarations {
                if let Some(init) = &declarator.init {
                    walk_expression_refs(b, init);
                }
            }
        }

        Statement::ExpressionStatement(expr) => {
            walk_expression_refs(b, &expr.expression);
        }

        Statement::ReturnStatement(ret) => {
            if let Some(arg) = &ret.argument {
                walk_expression_refs(b, arg);
            }
        }

        Statement::ThrowStatement(t) => {
            walk_expression_refs(b, &t.argument);
        }

        Statement::IfStatement(if_stmt) => {
            walk_expression_refs(b, &if_stmt.test);
            walk_statement_refs(b, &if_stmt.consequent);
            if let Some(alt) = &if_stmt.alternate {
                walk_statement_refs(b, alt);
            }
        }

        Statement::BlockStatement(block) => {
            // Enter the matching block scope that was created in phase 1.
            // We track scope progression by advancing through children.
            let saved = b.current;
            advance_to_child_scope(b, ScopeKind::Block);
            for s in &block.body {
                walk_statement_refs(b, s);
            }
            b.current = saved;
        }

        Statement::WhileStatement(w) => {
            walk_expression_refs(b, &w.test);
            walk_statement_refs(b, &w.body);
        }

        Statement::DoWhileStatement(d) => {
            walk_statement_refs(b, &d.body);
            walk_expression_refs(b, &d.test);
        }

        Statement::ForStatement(f) => {
            let saved = b.current;
            advance_to_child_scope(b, ScopeKind::Block);
            if let Some(init) = &f.init
                && let Some(expr) = init.as_expression()
            {
                walk_expression_refs(b, expr);
            }
            if let Some(test) = &f.test {
                walk_expression_refs(b, test);
            }
            if let Some(update) = &f.update {
                walk_expression_refs(b, update);
            }
            walk_statement_refs(b, &f.body);
            b.current = saved;
        }

        Statement::ForInStatement(fi) => {
            let saved = b.current;
            advance_to_child_scope(b, ScopeKind::Block);
            walk_expression_refs(b, &fi.right);
            walk_statement_refs(b, &fi.body);
            b.current = saved;
        }

        Statement::ForOfStatement(fo) => {
            let saved = b.current;
            advance_to_child_scope(b, ScopeKind::Block);
            walk_expression_refs(b, &fo.right);
            walk_statement_refs(b, &fo.body);
            b.current = saved;
        }

        Statement::SwitchStatement(sw) => {
            walk_expression_refs(b, &sw.discriminant);
            let saved = b.current;
            advance_to_child_scope(b, ScopeKind::Block);
            for case in &sw.cases {
                if let Some(test) = &case.test {
                    walk_expression_refs(b, test);
                }
                for s in &case.consequent {
                    walk_statement_refs(b, s);
                }
            }
            b.current = saved;
        }

        Statement::TryStatement(try_stmt) => {
            // Try block
            {
                let saved = b.current;
                advance_to_child_scope(b, ScopeKind::Block);
                for s in &try_stmt.block.body {
                    walk_statement_refs(b, s);
                }
                b.current = saved;
            }

            // Catch clause
            if let Some(handler) = &try_stmt.handler {
                let saved = b.current;
                advance_to_child_scope(b, ScopeKind::Catch);
                for s in &handler.body.body {
                    walk_statement_refs(b, s);
                }
                b.current = saved;
            }

            // Finally block
            if let Some(finalizer) = &try_stmt.finalizer {
                let saved = b.current;
                advance_to_child_scope(b, ScopeKind::Block);
                for s in &finalizer.body {
                    walk_statement_refs(b, s);
                }
                b.current = saved;
            }
        }

        Statement::WithStatement(with_stmt) => {
            walk_expression_refs(b, &with_stmt.object);
            let saved = b.current;
            advance_to_child_scope(b, ScopeKind::With);
            walk_statement_refs(b, &with_stmt.body);
            b.current = saved;
        }

        Statement::LabeledStatement(l) => {
            walk_statement_refs(b, &l.body);
        }

        Statement::FunctionDeclaration(func) => {
            if let Some(body) = &func.body {
                walk_function_body_refs(b, &func.params.items, body);
            }
        }

        Statement::ClassDeclaration(class) => {
            walk_class_body_refs(b, class);
        }

        _ => {}
    }
}

/// Walk an expression to find variable references and detect captures.
fn walk_expression_refs(b: &mut ScopeBuilder, expr: &Expression<'_>) {
    match expr {
        Expression::Identifier(ident) => {
            let name = ident.name.as_str();
            // Skip well-known globals that are never user-declared
            if matches!(name, "undefined" | "NaN" | "Infinity") {
                return;
            }
            if let Some(var_id) = b.resolve(name) {
                let var_scope = b.variables[var_id.0 as usize].scope;
                if b.is_across_function_boundary(b.current, var_scope) {
                    b.mark_captured(var_id);
                }
            }
        }

        Expression::AssignmentExpression(a) => {
            // The left-hand side is a mutation
            if let oxc_ast::ast::AssignmentTarget::AssignmentTargetIdentifier(ident) = &a.left {
                let name = ident.name.as_str();
                if let Some(var_id) = b.resolve(name) {
                    b.variables[var_id.0 as usize].is_mutated = true;
                    let var_scope = b.variables[var_id.0 as usize].scope;
                    if b.is_across_function_boundary(b.current, var_scope) {
                        b.mark_captured(var_id);
                    }
                }
            }
            walk_expression_refs(b, &a.right);
            // Walk complex LHS
            match &a.left {
                oxc_ast::ast::AssignmentTarget::StaticMemberExpression(m) => {
                    walk_expression_refs(b, &m.object);
                }
                oxc_ast::ast::AssignmentTarget::ComputedMemberExpression(m) => {
                    walk_expression_refs(b, &m.object);
                    walk_expression_refs(b, &m.expression);
                }
                _ => {}
            }
        }

        Expression::UpdateExpression(u) => {
            if let oxc_ast::ast::SimpleAssignmentTarget::AssignmentTargetIdentifier(ident) =
                &u.argument
            {
                let name = ident.name.as_str();
                if let Some(var_id) = b.resolve(name) {
                    b.variables[var_id.0 as usize].is_mutated = true;
                    let var_scope = b.variables[var_id.0 as usize].scope;
                    if b.is_across_function_boundary(b.current, var_scope) {
                        b.mark_captured(var_id);
                    }
                }
            }
        }

        Expression::FunctionExpression(func) => {
            if let Some(body) = &func.body {
                walk_function_body_refs(b, &func.params.items, body);
            }
        }

        Expression::ArrowFunctionExpression(arrow) => {
            walk_function_body_refs(b, &arrow.params.items, &arrow.body);
        }

        Expression::ClassExpression(class) => {
            walk_class_body_refs(b, class);
        }

        Expression::BinaryExpression(bin) => {
            walk_expression_refs(b, &bin.left);
            walk_expression_refs(b, &bin.right);
        }

        Expression::LogicalExpression(l) => {
            walk_expression_refs(b, &l.left);
            walk_expression_refs(b, &l.right);
        }

        Expression::UnaryExpression(u) => {
            walk_expression_refs(b, &u.argument);
        }

        Expression::ConditionalExpression(c) => {
            walk_expression_refs(b, &c.test);
            walk_expression_refs(b, &c.consequent);
            walk_expression_refs(b, &c.alternate);
        }

        Expression::CallExpression(call) => {
            // Detect direct eval: `eval(...)` where `eval` is the unqualified
            // identifier and is NOT shadowed by a local binding.
            if let Expression::Identifier(ident) = &call.callee
                && ident.name.as_str() == "eval"
                && !b.is_eval_shadowed()
            {
                b.mark_calls_eval();
            }
            walk_expression_refs(b, &call.callee);
            for arg in &call.arguments {
                if let Some(e) = arg.as_expression() {
                    walk_expression_refs(b, e);
                }
            }
        }

        Expression::NewExpression(n) => {
            walk_expression_refs(b, &n.callee);
            for arg in &n.arguments {
                if let Some(e) = arg.as_expression() {
                    walk_expression_refs(b, e);
                }
            }
        }

        Expression::ArrayExpression(arr) => {
            for elem in &arr.elements {
                if let Some(e) = elem.as_expression() {
                    walk_expression_refs(b, e);
                }
            }
        }

        Expression::ObjectExpression(obj) => {
            for prop in &obj.properties {
                if let ObjectPropertyKind::ObjectProperty(p) = prop {
                    if let Some(expr) = p.key.as_expression() {
                        walk_expression_refs(b, expr);
                    }
                    walk_expression_refs(b, &p.value);
                }
            }
        }

        Expression::StaticMemberExpression(m) => {
            walk_expression_refs(b, &m.object);
        }

        Expression::ComputedMemberExpression(m) => {
            walk_expression_refs(b, &m.object);
            walk_expression_refs(b, &m.expression);
        }

        Expression::TemplateLiteral(t) => {
            for e in &t.expressions {
                walk_expression_refs(b, e);
            }
        }

        Expression::SequenceExpression(s) => {
            for e in &s.expressions {
                walk_expression_refs(b, e);
            }
        }

        Expression::ParenthesizedExpression(p) => {
            walk_expression_refs(b, &p.expression);
        }

        Expression::AwaitExpression(a) => {
            walk_expression_refs(b, &a.argument);
        }

        Expression::YieldExpression(y) => {
            if let Some(arg) = &y.argument {
                walk_expression_refs(b, arg);
            }
        }

        Expression::ChainExpression(chain) => match &chain.expression {
            oxc_ast::ast::ChainElement::CallExpression(call) => {
                walk_expression_refs(b, &call.callee);
                for arg in &call.arguments {
                    if let Some(e) = arg.as_expression() {
                        walk_expression_refs(b, e);
                    }
                }
            }
            oxc_ast::ast::ChainElement::StaticMemberExpression(m) => {
                walk_expression_refs(b, &m.object);
            }
            oxc_ast::ast::ChainElement::ComputedMemberExpression(m) => {
                walk_expression_refs(b, &m.object);
                walk_expression_refs(b, &m.expression);
            }
            _ => {}
        },

        _ => {}
    }
}

/// Walk a function body for references, entering its Function scope.
fn walk_function_body_refs(
    b: &mut ScopeBuilder,
    _params: &[FormalParameter<'_>],
    body: &FunctionBody<'_>,
) {
    let saved = b.current;
    advance_to_child_scope(b, ScopeKind::Function);
    for stmt in &body.statements {
        walk_statement_refs(b, stmt);
    }
    b.current = saved;
}

/// Walk class body methods for references.
fn walk_class_body_refs(b: &mut ScopeBuilder, class: &oxc_ast::ast::Class<'_>) {
    for element in &class.body.body {
        if let oxc_ast::ast::ClassElement::MethodDefinition(method) = element {
            let func = &method.value;
            if let Some(body) = &func.body {
                walk_function_body_refs(b, &func.params.items, body);
            }
        }
    }
}

/// Advance `b.current` to the next child scope of the given kind.
///
/// During the reference walk (phase 2), we must enter the same scopes
/// that were created during the declaration walk (phase 1). We track
/// a per-scope cursor so that when a parent has multiple children of
/// the same kind (e.g., two function declarations), each call advances
/// to the NEXT unvisited child rather than always returning the first.
fn advance_to_child_scope(b: &mut ScopeBuilder, expected_kind: ScopeKind) {
    let parent = b.current;
    let cursor = b.child_cursors.entry(parent).or_insert(0);
    let children = &b.scopes[parent.0 as usize].children;

    // Start from the cursor position and find the next child of the
    // expected kind.
    while *cursor < children.len() {
        let child_id = children[*cursor];
        *cursor += 1;
        if b.scopes[child_id.0 as usize].kind == expected_kind {
            b.current = child_id;
            return;
        }
    }
    // If no matching child found (shouldn't happen in well-formed ASTs),
    // stay in the current scope — the analysis will still be safe but
    // may miss some captures.
}

// ---------------------------------------------------------------------------
// Phase 2.5: Poisoning propagation
// ---------------------------------------------------------------------------

/// Propagate `calls_eval` and `contains_with` flags upward through the
/// scope tree and compute `needs_dynamic_env` for function scopes.
///
/// For each scope that has `calls_eval = true`:
/// - All ancestor scopes within the same function get `inner_calls_eval = true`
/// - The enclosing function scope gets `needs_dynamic_env = true`
///   UNLESS the enclosing function is strict mode (strict eval creates its
///   own scope and does not affect the enclosing scope)
///
/// For each scope that has `contains_with = true`:
/// - The enclosing function scope gets `needs_dynamic_env = true`
fn propagate_poisoning(b: &mut ScopeBuilder) {
    let scope_count = b.scopes.len();

    // Pass 1: Propagate calls_eval upward within function boundaries.
    // We process scopes from deepest to shallowest (children before parents).
    // Since scopes are created in DFS order, reversing gives us children first.
    for i in (0..scope_count).rev() {
        let scope_id = ScopeId(i as u32);

        if b.scopes[i].flags.calls_eval {
            // Check if this eval is in a strict-mode context.
            // Strict mode eval creates its own scope, so it does NOT poison.
            let fn_scope_id = b.find_function_scope(scope_id);
            let is_strict = b.is_module
                || b.scopes[fn_scope_id.0 as usize].kind == ScopeKind::Module
                || b.strict_functions.contains_key(&fn_scope_id);

            if is_strict {
                // Strict eval does not poison the enclosing function.
                // Clear the flag so it doesn't propagate.
                b.scopes[i].flags.calls_eval = false;
                continue;
            }

            // Mark the enclosing function scope as needing a dynamic env.
            let fn_scope = b.find_function_scope(scope_id);
            b.scopes[fn_scope.0 as usize].flags.needs_dynamic_env = true;

            // If the eval is in a nested block/catch within the function
            // (not directly on the function scope), propagate inner_calls_eval
            // up through intermediate scopes to the function boundary.
            if scope_id != fn_scope {
                let mut parent_id = b.scopes[i].parent;
                while let Some(pid) = parent_id {
                    b.scopes[pid.0 as usize].flags.inner_calls_eval = true;
                    let parent_kind = b.scopes[pid.0 as usize].kind;
                    if matches!(
                        parent_kind,
                        ScopeKind::Function | ScopeKind::Global | ScopeKind::Module
                    ) {
                        break;
                    }
                    parent_id = b.scopes[pid.0 as usize].parent;
                }
            }
        }
    }

    // Pass 2: Propagate contains_with to the enclosing function scope.
    for i in 0..scope_count {
        if b.scopes[i].flags.contains_with {
            let fn_scope = b.find_function_scope(ScopeId(i as u32));
            b.scopes[fn_scope.0 as usize].flags.needs_dynamic_env = true;
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 3: Location assignment
// ---------------------------------------------------------------------------

/// Assign [`VariableLocation`] for all variables.
///
/// Variables in poisoned function scopes (where `needs_dynamic_env` is true)
/// are promoted to [`VariableLocation::Environment`] because their bindings
/// cannot be statically resolved at compile time.
fn assign_locations(b: &mut ScopeBuilder) {
    // Build a set of function scopes that need dynamic environments.
    let poisoned_fn_scopes: Vec<ScopeId> = b
        .scopes
        .iter()
        .enumerate()
        .filter(|(_, scope)| scope.flags.needs_dynamic_env)
        .map(|(i, _)| ScopeId(i as u32))
        .collect();

    for var in &mut b.variables {
        // Check if this variable's enclosing function scope is poisoned.
        let var_fn_scope = find_function_scope_of(&b.scopes, var.scope);
        if poisoned_fn_scopes.contains(&var_fn_scope) {
            var.location = VariableLocation::Environment;
        } else {
            var.location = VariableLocation::Stack;
        }
    }
}

/// Find the nearest function/global/module scope for a given scope ID,
/// operating on the scope slice directly (no mutable borrow needed).
fn find_function_scope_of(scopes: &[ScopeNode], from: ScopeId) -> ScopeId {
    let mut id = from;
    loop {
        let kind = scopes[id.0 as usize].kind;
        if matches!(
            kind,
            ScopeKind::Function | ScopeKind::Global | ScopeKind::Module
        ) {
            return id;
        }
        match scopes[id.0 as usize].parent {
            Some(parent) => id = parent,
            None => return id,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Collect all bound names from a binding pattern.
///
/// Handles identifiers, array destructuring, and object destructuring.
fn collect_binding_names(pattern: &BindingPattern<'_>) -> Vec<String> {
    let mut names = Vec::new();
    collect_binding_names_inner(pattern, &mut names);
    names
}

/// Recursive helper for collecting names from binding patterns.
fn collect_binding_names_inner(pattern: &BindingPattern<'_>, names: &mut Vec<String>) {
    match pattern {
        BindingPattern::BindingIdentifier(ident) => {
            names.push(ident.name.as_str().to_string());
        }
        BindingPattern::ObjectPattern(obj) => {
            for prop in &obj.properties {
                collect_binding_names_inner(&prop.value, names);
            }
            if let Some(rest) = &obj.rest {
                collect_binding_names_inner(&rest.argument, names);
            }
        }
        BindingPattern::ArrayPattern(arr) => {
            for elem in arr.elements.iter().flatten() {
                collect_binding_names_inner(elem, names);
            }
            if let Some(rest) = &arr.rest {
                collect_binding_names_inner(&rest.argument, names);
            }
        }
        BindingPattern::AssignmentPattern(assign) => {
            collect_binding_names_inner(&assign.left, names);
        }
    }
}

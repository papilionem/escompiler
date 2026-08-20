use std::collections::{HashMap, HashSet};

/// The kind of scope in the JavaScript scope hierarchy.
///
/// Determines variable visibility rules: `var` hoists to the nearest
/// `Function` or `Global` scope, while `let`/`const` stay in their
/// enclosing `Block` (or `Catch`/`Module`) scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScopeKind {
    /// Top-level script scope (sloppy mode by default).
    Global,
    /// Function body scope (`function`, arrow, method, constructor).
    Function,
    /// Block scope (`{ }`, `for`, `if`, `switch`).
    Block,
    /// Catch clause scope — owns the error binding (`catch (e) { ... }`).
    Catch,
    /// `with` statement scope — all property lookups are dynamic.
    With,
    /// ES module top-level scope (always strict mode).
    Module,
}

impl ScopeKind {
    /// Returns `true` for scope kinds that act as `var` hoisting boundaries.
    ///
    /// `var` declarations hoist to the nearest `Function`, `Global`, or
    /// `Module` scope.
    pub fn is_var_scope(self) -> bool {
        matches!(self, Self::Function | Self::Global | Self::Module)
    }
}

/// Where a variable is stored at runtime.
///
/// Determined by scope analysis before IR lowering begins. Most variables
/// live on the stack as SSA values. Variables captured by closures that
/// mutate them must be promoted to heap-allocated environment slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VariableLocation {
    /// Local SSA variable on the stack (most common, fastest).
    Stack,
    /// Heap-allocated environment slot (for captured+mutated variables in closures).
    Environment,
    /// Property on the global object.
    Global,
}

/// Tracks which variables a closure captures from parent scopes.
///
/// Each captured variable is assigned a sequential slot index in the
/// closure's environment object.
#[derive(Debug, Clone, Default)]
pub struct CaptureInfo {
    /// Maps captured variable names to their environment slot index.
    pub captured_vars: HashMap<String, u32>,
    next_slot: u32,
}

impl CaptureInfo {
    /// Record a variable as captured, assigning it the next slot index.
    /// Returns the slot index (existing if already captured).
    pub fn add(&mut self, name: &str) -> u32 {
        if let Some(&slot) = self.captured_vars.get(name) {
            return slot;
        }
        let slot = self.next_slot;
        self.next_slot += 1;
        self.captured_vars.insert(name.to_string(), slot);
        slot
    }

    /// Number of captured variables (environment slot count).
    pub fn slot_count(&self) -> u32 {
        self.next_slot
    }

    /// Returns true if no variables were captured.
    pub fn is_empty(&self) -> bool {
        self.captured_vars.is_empty()
    }
}

struct Scope {
    variables: HashMap<String, u32>,
    kind: ScopeKind,
    /// Names declared with `let`/`const` in this scope (for duplicate detection
    /// and var/let conflict checking).
    let_const_vars: HashSet<String>,
}

/// Manages the lexical scope chain during AST-to-IR lowering.
///
/// Supports variable declaration, resolution, and capture analysis for
/// closures that reference variables across function boundaries.
pub struct ScopeStack {
    scopes: Vec<Scope>,
    next_var: u32,
    /// Stack of capture scopes, pushed when entering a closure body.
    capture_scopes: Vec<CaptureInfo>,
    /// Depth at which each capture scope was pushed (index into `scopes`).
    capture_depths: Vec<usize>,
}

impl Default for ScopeStack {
    fn default() -> Self {
        Self::new()
    }
}

impl ScopeStack {
    pub fn new() -> Self {
        Self {
            scopes: vec![Scope {
                variables: HashMap::new(),
                kind: ScopeKind::Global,
                let_const_vars: HashSet::new(),
            }],
            next_var: 0,
            capture_scopes: Vec::new(),
            capture_depths: Vec::new(),
        }
    }

    pub fn push_scope(&mut self, kind: ScopeKind) {
        self.scopes.push(Scope {
            variables: HashMap::new(),
            kind,
            let_const_vars: HashSet::new(),
        });
    }

    pub fn pop_scope(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    /// Return the kind of the current (innermost) scope.
    pub fn current_scope_kind(&self) -> ScopeKind {
        self.scopes
            .last()
            .map(|s| s.kind)
            .unwrap_or(ScopeKind::Global)
    }

    /// Return `true` if we are currently inside a block scope that is NOT
    /// the direct body of a function (i.e., this is a block inside `if`,
    /// `for`, `while`, `switch`, etc.). Used for Annex B.3.3 detection.
    pub fn is_inside_non_function_block(&self) -> bool {
        // Walk from innermost scope outward. If the first var-scope
        // boundary we hit is a Function/Global/Module, and there is at
        // least one Block scope between us and it, then we are inside a
        // non-function-body block.
        for scope in self.scopes.iter().rev() {
            match scope.kind {
                ScopeKind::Block | ScopeKind::Catch => return true,
                ScopeKind::Function | ScopeKind::Global | ScopeKind::Module => return false,
                ScopeKind::With => continue,
            }
        }
        false
    }

    /// Begin a capture scope. Call this before lowering a closure body.
    /// The current scope depth is recorded so we know which variables
    /// are "across the function boundary."
    pub fn begin_capture_scope(&mut self) {
        self.capture_scopes.push(CaptureInfo::default());
        self.capture_depths.push(self.scopes.len());
    }

    /// End the current capture scope and return what was captured.
    pub fn end_capture_scope(&mut self) -> CaptureInfo {
        self.capture_depths.pop();
        self.capture_scopes.pop().unwrap_or_default()
    }

    /// Declare a new variable in the current scope and return its SSA variable number.
    pub fn declare(&mut self, name: &str) -> u32 {
        let var = self.next_var;
        self.next_var += 1;
        if let Some(scope) = self.scopes.last_mut() {
            scope.variables.insert(name.to_string(), var);
        }
        var
    }

    /// Check if a `let`/`const` with the given name already exists in the
    /// current (top) scope. Returns `true` if it would be a duplicate.
    pub fn has_duplicate_let_const(&self, name: &str) -> bool {
        self.scopes
            .last()
            .is_some_and(|scope| scope.let_const_vars.contains(name))
    }

    /// Mark a variable as `let`/`const`-declared in the current scope.
    ///
    /// Call this after `declare()` for `let`/`const` bindings so that
    /// duplicate detection and var/let conflict checking work correctly.
    pub fn mark_let_const(&mut self, name: &str) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.let_const_vars.insert(name.to_string());
        }
    }

    /// Check if a `let`/`const` with the given name exists in any block scope
    /// between the current scope and the enclosing function/global scope.
    ///
    /// Used to detect `var`/`let` conflicts: `let x = 1; var x = 2;` is illegal.
    pub fn has_let_const_conflict(&self, name: &str) -> bool {
        for scope in self.scopes.iter().rev() {
            if scope.let_const_vars.contains(name) {
                return true;
            }
            // Stop at function, module, or global scope boundary
            if scope.kind.is_var_scope() {
                return false;
            }
        }
        false
    }

    /// Resolve a variable name to its SSA variable number by walking up the scope chain.
    pub fn resolve(&self, name: &str) -> Option<u32> {
        for scope in self.scopes.iter().rev() {
            if let Some(&var) = scope.variables.get(name) {
                return Some(var);
            }
        }
        None
    }

    /// Resolve a variable name, stopping at function boundaries.
    ///
    /// Only finds variables declared within the current (innermost) function
    /// scope and its inner block scopes. Returns `None` if the variable
    /// is only available in a parent function scope or global scope.
    pub fn resolve_local(&self, name: &str) -> Option<u32> {
        for scope in self.scopes.iter().rev() {
            if let Some(&var) = scope.variables.get(name) {
                return Some(var);
            }
            // Stop after checking the innermost Function scope — anything
            // below it belongs to a parent function or global scope.
            if scope.kind == ScopeKind::Function {
                return None;
            }
        }
        None
    }

    /// Resolve a variable, detecting cross-function-boundary references.
    ///
    /// If the variable is found in a scope above the current capture boundary
    /// (i.e., in a parent function), returns `Err(slot_index)` indicating
    /// the variable should be loaded from the closure environment.
    /// If found in the current function's scope, returns `Ok(var)`.
    /// If not found at all, returns `Ok(None)`.
    pub fn resolve_with_capture(&mut self, name: &str) -> ResolveResult {
        // If we have no capture scope active, fall back to normal resolve
        if self.capture_scopes.is_empty() {
            return match self.resolve(name) {
                Some(var) => ResolveResult::Local(var),
                None => ResolveResult::NotFound,
            };
        }

        let capture_depth = *self.capture_depths.last().unwrap_or(&0);

        // Walk scopes from innermost to outermost
        let mut depth = self.scopes.len();
        for scope in self.scopes.iter().rev() {
            depth -= 1;
            if let Some(&var) = scope.variables.get(name) {
                if depth < capture_depth {
                    // Variable is in a parent function scope — it's captured.
                    // capture_scopes is guaranteed non-empty here (guarded by
                    // the early return at the top of this method).
                    let Some(capture_info) = self.capture_scopes.last_mut() else {
                        unreachable!("BUG: capture_scopes empty despite is_empty() check");
                    };
                    let slot = capture_info.add(name);
                    return ResolveResult::Captured {
                        slot,
                        parent_var: var,
                    };
                }
                // Variable is in current function scope — local
                return ResolveResult::Local(var);
            }
        }

        ResolveResult::NotFound
    }

    /// Resolve a variable name, stopping at the nearest `With` scope boundary.
    ///
    /// Only finds variables declared within the `with` body (block scopes
    /// inside the `With` scope). Returns `None` if the variable is only
    /// available outside the `with` scope — in that case, the caller should
    /// route the lookup through the dynamic `EscEnvironment`.
    ///
    /// Variables declared with `let`/`const` inside the `with` body are
    /// lexically scoped and NOT affected by the with-object, so they must
    /// be resolved normally.
    pub fn resolve_within_with(&self, name: &str) -> Option<u32> {
        for scope in self.scopes.iter().rev() {
            if let Some(&var) = scope.variables.get(name) {
                return Some(var);
            }
            // Stop at the With scope boundary — anything below it is
            // outside the with body and should use dynamic lookup.
            if scope.kind == ScopeKind::With {
                return None;
            }
        }
        None
    }

    /// Check if a binding exists in any enclosing scope without creating one.
    pub fn has_binding(&self, name: &str) -> bool {
        self.resolve(name).is_some()
    }

    /// Resolve or declare (for undeclared global assignments).
    pub fn resolve_or_declare(&mut self, name: &str) -> u32 {
        if let Some(var) = self.resolve(name) {
            var
        } else {
            self.declare(name)
        }
    }

    /// Declare a variable in the nearest enclosing function or global scope.
    ///
    /// This implements `var` hoisting: `var` declarations are visible throughout
    /// their enclosing function, not just the current block.
    pub fn declare_in_function_scope(&mut self, name: &str) -> u32 {
        // Walk backwards to find the nearest Function or Global scope
        let target_idx = self
            .scopes
            .iter()
            .enumerate()
            .rev()
            .find(|(_, s)| s.kind.is_var_scope())
            .map(|(i, _)| i)
            .unwrap_or(0);

        // If already declared in that scope, return existing var
        if let Some(&var) = self.scopes[target_idx].variables.get(name) {
            return var;
        }

        let var = self.next_var;
        self.next_var += 1;
        self.scopes[target_idx]
            .variables
            .insert(name.to_string(), var);
        var
    }
}

/// Result of resolving a variable with capture analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveResult {
    /// Variable found in current function scope.
    Local(u32),
    /// Variable found across a function boundary — needs EnvLoad.
    Captured {
        /// Slot index in the closure environment.
        slot: u32,
        /// SSA variable number in the parent scope (for EnvStore at creation site).
        parent_var: u32,
    },
    /// Variable not found in any scope.
    NotFound,
}

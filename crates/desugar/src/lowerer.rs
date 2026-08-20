use std::collections::{HashMap, HashSet};

use ir::builder::{TypedIrBuilder, TypedModule};
use ir::{BlockId, IrType, Op, ValueId};
use thiserror::Error;

use crate::scope::ScopeStack;

/// Break and continue targets for a labeled statement.
pub(crate) struct LabelTarget {
    /// Block to jump to on `break label`.
    pub break_bb: BlockId,
    /// Block to jump to on `continue label` (Some only for loop labels).
    pub continue_bb: Option<BlockId>,
}

/// The kind of export recorded during lowering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportKind {
    /// `export { foo }` or `export const foo = ...`
    Named,
    /// `export default ...`
    Default,
    /// `export { foo } from './bar'` or `export * from './bar'`
    ReExport {
        /// The source module specifier.
        source: String,
    },
}

/// The declaration kind of an exported binding, used to determine whether
/// an import can be a direct value or must go through a getter for live
/// binding semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportDeclKind {
    /// `const` declaration — immutable, can be inlined at import site.
    Const,
    /// `let` declaration — mutable, requires a getter for live binding.
    Let,
    /// `var` declaration — mutable, requires a getter for live binding.
    Var,
    /// `function` declaration — hoisted and immutable in practice, can be inlined.
    Function,
    /// `class` declaration — similar to const, can be inlined.
    Class,
    /// Unknown or re-exported binding — declaration kind not available.
    Unknown,
}

impl ExportDeclKind {
    /// Whether this declaration kind requires a live binding getter.
    ///
    /// `let` and `var` declarations are mutable and need getters so that
    /// importing modules always read the current value.
    pub fn needs_getter(self) -> bool {
        matches!(self, Self::Let | Self::Var)
    }
}

/// A single export recorded during AST-to-IR lowering.
///
/// Captures the exported name and its kind so the driver can build
/// a cross-module export map without re-parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportInfo {
    /// The exported name (e.g., `"foo"`, `"default"`).
    pub name: String,
    /// What kind of export this is.
    pub kind: ExportKind,
    /// The declaration kind of the exported binding (const, let, var, function, etc.).
    pub decl_kind: ExportDeclKind,
}

/// The output of lowering a JavaScript source file into typed IR.
pub struct LoweringResult {
    /// The compiled IR module containing all functions.
    pub module: TypedModule,
    /// Any errors encountered during lowering (syntax errors, unsupported constructs).
    pub errors: Vec<LoweringError>,
    /// Deliberate refusals: constructs this compiler declines to compile, each
    /// carrying a declared code. Kept separate from `errors` so the driver can
    /// map them to exit 2 without string-matching a message.
    pub refusals: Vec<Refusal>,
    /// Interned string constants referenced by `ConstString` instructions.
    pub string_table: Vec<String>,
    /// Exports recorded during lowering (populated only for ES module sources).
    pub exports: Vec<ExportInfo>,
    /// Whether this module uses top-level `await` (ES2022).
    ///
    /// True only for ES modules whose top-level body contains one or more `await`
    /// expressions. When true, the entry function is marked `is_async = true` so
    /// the generator transform pass converts it into an async state machine.
    /// Scripts never have this flag set.
    pub has_top_level_await: bool,
    /// Module specifiers discovered from `import()` expressions with string
    /// literal arguments. These are resolved at compile time and added to the
    /// module graph alongside static imports.
    pub dynamic_imports: Vec<String>,
    /// Whether the source uses FFI features (extern declarations, native bindings).
    ///
    /// Set to `true` when the lowerer encounters FFI-related constructs.
    /// The compilation pipeline checks this flag against the `allow_ffi`
    /// permission and emits ESC-E700 if FFI is used without permission.
    pub has_ffi_usage: bool,
    /// Whether any `eval()` call was encountered in the source.
    pub has_eval: bool,
    /// Whether any `new Function()` or `Function()` constructor call was
    /// encountered in the source.
    pub has_function_constructor: bool,
}

/// A deliberate, declared refusal to compile a program.
///
/// Distinct from [`LoweringError`] on purpose. An error means *the compiler could
/// not do its job*; a refusal means *the compiler will not, and says exactly why*.
/// The sealed v0.9 rung requires the two to stay distinguishable all the way out
/// to the process exit status — a refusal exits **2**, a compile failure exits 1 —
/// because a caller that cannot tell them apart treats "this feature does not
/// exist yet" and "your program is broken" as the same event.
#[derive(Debug, Clone)]
pub struct Refusal {
    /// The declared diagnostic code, e.g. `ESC-E300`.
    pub code: &'static str,
    /// What was refused and why, in one sentence.
    pub message: String,
}

/// An error produced during AST-to-IR lowering.
#[derive(Debug, Clone, Error)]
#[error("lowering error: {message}")]
pub struct LoweringError {
    /// Human-readable description of what went wrong.
    pub message: String,
}

/// Check if an identifier name is a compile-time platform constant.
///
/// Recognized constants: `__esc_platform`, `__esc_arch`, `__esc_build_mode`.
/// These are replaced at compile time with `ConstString` values and should
/// not trigger `ReferenceError` for undeclared identifiers.
pub(crate) fn is_platform_constant(name: &str) -> bool {
    matches!(name, "__esc_platform" | "__esc_arch" | "__esc_build_mode")
}

/// Check if the entry function (index 0) of a module contains any `Await` opcodes.
///
/// Used to detect top-level `await` in ES modules. Only checks the entry function
/// itself (function 0), not nested functions — inner async functions contain their
/// own awaits, which do not make the module's entry async.
fn entry_function_has_await(module: &TypedModule) -> bool {
    let Some(func) = module.functions.first() else {
        return false;
    };
    for block in &func.blocks {
        for instr in &block.instructions {
            if instr.op == Op::Await {
                return true;
            }
        }
    }
    false
}

/// Lower a JavaScript source string (parsed as ES module) into typed IR.
///
/// Module mode implies strict mode per the ECMAScript spec.
pub fn lower_program(source: &str) -> Result<LoweringResult, Vec<LoweringError>> {
    lower_source(source, oxc_span::SourceType::mjs())
}

/// Lower a JavaScript source string (parsed as script / CommonJS) into typed IR.
///
/// Script mode uses sloppy mode by default; strict mode is only enabled when
/// a `"use strict"` directive is present.
pub fn lower_script(source: &str) -> Result<LoweringResult, Vec<LoweringError>> {
    lower_source(source, oxc_span::SourceType::cjs())
}

/// Lower a source string with explicit source type into typed IR.
pub fn lower_source(
    source: &str,
    source_type: oxc_span::SourceType,
) -> Result<LoweringResult, Vec<LoweringError>> {
    lower_source_with_build_mode(source, source_type, "debug")
}

/// Lower a source string with explicit source type and build mode into typed IR.
///
/// The `build_mode` parameter controls the value of the `__esc_build_mode`
/// compile-time constant (typically `"debug"` or `"release"`).
pub fn lower_source_with_build_mode(
    source: &str,
    source_type: oxc_span::SourceType,
    build_mode: &str,
) -> Result<LoweringResult, Vec<LoweringError>> {
    let build_mode = build_mode.to_string();
    let result = parser::parse_with(source, source_type, |program| {
        let mut lowerer = IrLowerer::with_build_mode(&build_mode);

        // Begin the top-level "main" function
        lowerer
            .builder
            .begin_function("main", vec![], IrType::JSValue);
        lowerer.function_count += 1;

        let entry = lowerer.builder.create_block();
        lowerer.builder.switch_to_block(entry);
        lowerer.builder.seal_block(entry);
        lowerer.current_block = Some(entry);

        // Detect "use strict" directive at program level
        for directive in &program.directives {
            if directive.directive.as_str() == "use strict" {
                lowerer.is_strict = true;
                break;
            }
        }

        // ES modules are always strict
        if program.source_type.is_module() {
            lowerer.is_strict = true;
        }

        // Pre-scan top-level for let/const TDZ names
        {
            let (tdz_names, _const_names) = IrLowerer::collect_block_lexical_names(&program.body);
            for name in tdz_names {
                lowerer.tdz_vars.insert(name);
            }
        }

        // Lower all top-level statements
        for stmt in &program.body {
            if lowerer.terminated {
                break;
            }
            lowerer.lower_statement(stmt);
        }

        // Ensure the main function has a return
        if !lowerer.block_terminated() {
            let undef = lowerer.builder.const_undefined();
            lowerer.builder.ret(Some(undef));
        }

        lowerer.builder.end_function();
        lowerer.builder.set_entry(0);

        let errors = lowerer.errors.clone();
        let refusals = lowerer.refusals.clone();
        let string_table = lowerer.string_table.clone();
        let exports = lowerer.recorded_exports.clone();
        let dynamic_imports = lowerer.dynamic_imports.clone();
        let has_eval = lowerer.has_eval;
        let has_function_constructor = lowerer.has_function_constructor;
        let mut module = lowerer.builder.finish();

        // Detect top-level await in ES modules.
        // If the entry function (index 0) contains any Await opcodes and this
        // is a module, mark it as async so generator_transform converts it
        // into a state machine that returns a Promise.
        let is_module = program.source_type.is_module();
        let has_tla = is_module && entry_function_has_await(&module);
        if has_tla {
            module.functions[0].is_async = true;
        }

        LoweringResult {
            module,
            errors,
            refusals,
            string_table,
            exports,
            has_top_level_await: has_tla,
            dynamic_imports,
            has_ffi_usage: false,
            has_eval,
            has_function_constructor,
        }
    });

    match result {
        Ok(lowering_result) => {
            if lowering_result.errors.is_empty() {
                Ok(lowering_result)
            } else {
                Err(lowering_result.errors)
            }
        }
        Err(parse_errors) => Err(parse_errors
            .into_iter()
            .map(|e| LoweringError { message: e.message })
            .collect()),
    }
}

/// Stateful lowerer that walks the oxc AST and emits SSA IR instructions.
///
/// Holds the IR builder, scope stack, loop/label targets, and various flags
/// needed to correctly translate JavaScript semantics (strict mode, TDZ,
/// closures, try/catch/finally) into the IR.
pub struct IrLowerer {
    pub(crate) builder: TypedIrBuilder,
    pub(crate) scopes: ScopeStack,
    pub(crate) current_block: Option<BlockId>,
    pub(crate) loop_break_target: Option<BlockId>,
    pub(crate) loop_continue_target: Option<BlockId>,
    /// Map of label names to their break/continue targets for labeled statements.
    pub(crate) label_targets: HashMap<String, LabelTarget>,
    /// When set, the next loop to set its continue target will also update
    /// this label's `continue_bb` in `label_targets`. Consumed after use.
    pub(crate) active_label: Option<String>,
    pub(crate) terminated: bool,
    pub(crate) errors: Vec<LoweringError>,
    pub(crate) refusals: Vec<Refusal>,
    pub(crate) string_table: Vec<String>,
    pub(crate) string_map: HashMap<String, u32>,
    pub(crate) function_count: usize,
    pub(crate) next_temp_var: u32,
    /// The current function's environment parameter value (None for top-level).
    pub(crate) capture_env: Option<ValueId>,
    /// Maps captured variable names to env slot indices in the current function.
    pub(crate) captured_vars: HashMap<String, u32>,
    /// Names of variables that are stored as JsBox pointers in the current
    /// function. Reads go through `BoxLoad`, writes go through `BoxStore`.
    /// This is set for both the declaring function (after `AllocBox`) and
    /// for closures that capture a ByBox variable.
    pub(crate) boxed_vars: HashSet<String>,
    /// Whether we are currently in strict mode (`"use strict"` or class body).
    pub(crate) is_strict: bool,
    /// Monotonically increasing counter for inline cache site IDs.
    pub(crate) ic_counter: u32,
    /// When true, `lower_binding_pattern` uses `declare_in_function_scope`
    /// instead of `declare`. Set temporarily for `var` declarations.
    pub(crate) var_hoist: bool,
    /// Names declared with `const` in the current function scope.
    /// Used to detect and reject reassignment at compile time.
    pub(crate) const_vars: HashSet<String>,
    /// Names declared with `let`/`const` that have not yet been initialized
    /// (the declaration statement has not been lowered yet). Used to detect
    /// TDZ (Temporal Dead Zone) violations at compile time.
    pub(crate) tdz_vars: HashSet<String>,
    /// When inside a try-with-finally, the block to branch to for finally.
    /// A `return` inside the try/catch body branches here instead of `ret`.
    pub(crate) finally_target: Option<BlockId>,
    /// SSA variable (JSValue) for storing a pending return value.
    pub(crate) finally_return_var: Option<u32>,
    /// SSA variable (JSValue) for a flag: truthy = pending return.
    pub(crate) finally_has_return_var: Option<u32>,
    /// SSA variable (JSValue) for storing an exception value.
    pub(crate) finally_exception_var: Option<u32>,
    /// SSA variable (JSValue) for a flag: truthy = pending exception.
    pub(crate) finally_has_exception_var: Option<u32>,
    /// When true, `throw` in the current scope should redirect to
    /// `finally_target` instead of emitting `Op::Throw`. Set during
    /// catch body lowering when there is a finally block.
    pub(crate) finally_catch_redirects_throw: bool,
    /// The depth of `catch_target_stack` when `finally_catch_redirects_throw`
    /// was enabled.  If the stack grows beyond this depth, we know a nested
    /// try-catch has been entered and throws should go to that inner catch
    /// handler, not to the finally redirect target.
    pub(crate) finally_catch_depth: usize,
    /// Stack of catch block targets, mirroring Cranelift's `try_catch_stack`.
    /// Pushed on `TryBegin`, popped on `TryEnd`. Used by `emit_finally_completion`
    /// to embed the correct catch target on `Rethrow` instructions, since the
    /// Cranelift backend's sequential `try_catch_stack` may be stale.
    pub(crate) catch_target_stack: Vec<BlockId>,
    /// SSA variable (JSValue) for a flag: truthy = pending break or continue.
    /// Used to track non-local break/continue that must pass through finally.
    pub(crate) finally_has_break_var: Option<u32>,
    /// SSA variable (JSValue) encoding the jump target index for pending
    /// break/continue through finally. The index maps into the per-finally
    /// `finally_jump_targets` table.
    pub(crate) finally_break_target_var: Option<u32>,
    /// SSA variable (JSValue) for a flag distinguishing break (0.0) from
    /// continue (1.0) completions.
    pub(crate) finally_is_continue_var: Option<u32>,
    /// Mapping from numeric index to `BlockId` for break/continue targets
    /// registered at the current try-finally level. Built as break/continue
    /// statements are lowered inside the try body.
    pub(crate) finally_jump_targets: Vec<BlockId>,
    /// Set of break/continue target `BlockId`s that existed BEFORE the
    /// current try-finally was entered. A break/continue to one of these
    /// targets must route through the finally block. Targets created INSIDE
    /// the try body (e.g., by a switch or inner loop) are NOT in this set
    /// and can be branched to directly.
    pub(crate) finally_external_targets: HashSet<BlockId>,
    /// When set, `ThisExpression` resolves to this value instead of emitting
    /// `Op::ThisValue`. Used inside static initializer blocks where `this`
    /// refers to the class constructor, not the enclosing function's `this`.
    pub(crate) this_override: Option<ValueId>,
    /// Monotonically increasing counter for private name IDs.
    /// Each `#field` or `#method` in each class gets a globally unique ID.
    pub(crate) next_private_id: u32,
    /// Maps private field names to their compile-time private name IDs
    /// within the current class being lowered. Cleared between classes.
    pub(crate) private_name_ids: HashMap<String, u32>,
    /// Exports recorded during lowering (export declarations and re-exports).
    pub(crate) recorded_exports: Vec<ExportInfo>,
    /// The build mode string ("debug" or "release") for the `__esc_build_mode`
    /// compile-time constant. Defaults to "debug".
    pub(crate) build_mode: String,
    /// When inside a `with` statement body, this SSA variable holds the
    /// current `EscEnvironment` pointer (returned by `__esc_rt_with_env_create`).
    /// Identifier reads/writes for non-lexical names route through dynamic
    /// lookup using this environment. `None` when not inside a `with` body.
    pub(crate) with_env_var: Option<u32>,
    /// Stack of saved `with_env_var` values for nested `with` statements.
    /// Each entry is the `with_env_var` from the enclosing `with` scope.
    pub(crate) with_env_stack: Vec<Option<u32>>,
    /// Tier 0 `with` optimization: when the with-object is an object literal
    /// (e.g., `with({x: 1, y: 2})`), this holds the known property names AND
    /// the SSA value of the object. Identifier lookups for known names can skip
    /// the dynamic `EscEnvironment` and emit direct property access instead.
    pub(crate) with_known_props: Option<(HashSet<String>, ValueId)>,
    /// Stack of saved `with_known_props` values for nested `with` statements.
    pub(crate) with_known_props_stack: Vec<Option<(HashSet<String>, ValueId)>>,
    /// When inside a function marked `needs_dynamic_env` (contains direct eval),
    /// this SSA variable holds the `EscEnvironment` pointer created at function
    /// entry. Variable reads/writes route through `__esc_rt_esc_env_get`/`set`
    /// so eval'd code can see the same bindings.
    /// `None` when the current function is not poisoned.
    pub(crate) poisoned_env_var: Option<u32>,
    /// Maps variable names to `EscEnvironment` slot indices in the current
    /// poisoned function. Used to translate variable accesses to env slot
    /// get/set calls.
    pub(crate) poisoned_slot_map: HashMap<String, u32>,
    /// Module specifiers from `import()` expressions with string literal args.
    /// Collected during lowering so the module pipeline can discover dynamically
    /// imported modules at compile time.
    pub(crate) dynamic_imports: Vec<String>,
    /// Whether any `eval()` call was encountered during lowering (including
    /// calls that were successfully inlined at compile time).
    pub(crate) has_eval: bool,
    /// Whether any `new Function()` or `Function()` constructor call was
    /// encountered during lowering.
    pub(crate) has_function_constructor: bool,
}

impl Default for IrLowerer {
    fn default() -> Self {
        Self::new()
    }
}

impl IrLowerer {
    /// Create a new IR lowerer.
    pub fn new() -> Self {
        Self {
            builder: TypedIrBuilder::new(),
            scopes: ScopeStack::new(),
            current_block: None,
            loop_break_target: None,
            loop_continue_target: None,
            label_targets: HashMap::new(),
            active_label: None,
            terminated: false,
            errors: Vec::new(),
            refusals: Vec::new(),
            string_table: Vec::new(),
            string_map: HashMap::new(),
            function_count: 0,
            next_temp_var: 10000,
            capture_env: None,
            captured_vars: HashMap::new(),
            boxed_vars: HashSet::new(),
            is_strict: false,
            ic_counter: 0,
            var_hoist: false,
            const_vars: HashSet::new(),
            tdz_vars: HashSet::new(),
            finally_target: None,
            finally_return_var: None,
            finally_has_return_var: None,
            finally_exception_var: None,
            finally_has_exception_var: None,
            finally_catch_redirects_throw: false,
            finally_catch_depth: 0,
            catch_target_stack: Vec::new(),
            finally_has_break_var: None,
            finally_break_target_var: None,
            finally_is_continue_var: None,
            finally_jump_targets: Vec::new(),
            finally_external_targets: HashSet::new(),
            this_override: None,
            next_private_id: 0,
            private_name_ids: HashMap::new(),
            recorded_exports: Vec::new(),
            build_mode: "debug".to_string(),
            with_env_var: None,
            with_env_stack: Vec::new(),
            with_known_props: None,
            with_known_props_stack: Vec::new(),
            poisoned_env_var: None,
            poisoned_slot_map: HashMap::new(),
            dynamic_imports: Vec::new(),
            has_eval: false,
            has_function_constructor: false,
        }
    }

    /// Create a new IR lowerer with the specified build mode.
    ///
    /// The `build_mode` is used for the `__esc_build_mode` compile-time constant.
    /// Valid values are `"debug"` and `"release"`.
    pub fn with_build_mode(build_mode: &str) -> Self {
        let mut lowerer = Self::new();
        lowerer.build_mode = build_mode.to_string();
        lowerer
    }

    /// Emit a compile-time platform constant as a `ConstString` instruction.
    ///
    /// Returns `Some(ValueId)` if `name` matches a recognized platform constant
    /// (`__esc_platform`, `__esc_arch`, `__esc_build_mode`), `None` otherwise.
    pub(crate) fn emit_platform_constant(&mut self, name: &str) -> Option<ValueId> {
        let value = match name {
            "__esc_platform" => std::env::consts::OS,
            "__esc_arch" => std::env::consts::ARCH,
            "__esc_build_mode" => {
                return Some({
                    let idx = self.intern_string(&self.build_mode.clone());
                    self.builder.const_string(idx)
                });
            }
            _ => return None,
        };
        let idx = self.intern_string(value);
        Some(self.builder.const_string(idx))
    }

    /// Allocate the next inline cache site ID.
    pub(crate) fn next_ic_id(&mut self) -> u32 {
        let id = self.ic_counter;
        self.ic_counter += 1;
        id
    }

    /// Intern a string into the module string table, returning its index.
    pub(crate) fn intern_string(&mut self, s: &str) -> u32 {
        if let Some(&idx) = self.string_map.get(s) {
            idx
        } else {
            let idx = self.string_table.len() as u32;
            self.string_table.push(s.to_string());
            self.string_map.insert(s.to_string(), idx);
            idx
        }
    }

    /// Allocate a globally unique private name ID for a `#field` or `#method`.
    pub(crate) fn allocate_private_name_id(&mut self) -> u32 {
        let id = self.next_private_id;
        self.next_private_id += 1;
        id
    }

    /// Allocate a temporary SSA variable number for phis and intermediate values.
    pub(crate) fn alloc_temp_var(&mut self) -> u32 {
        let var = self.next_temp_var;
        self.next_temp_var += 1;
        var
    }

    /// Check if the current block has already been terminated.
    pub(crate) fn block_terminated(&self) -> bool {
        self.terminated || self.current_block.is_none()
    }

    /// Check if a name is declared in any reachable scope (local, captured, or
    /// built-in global). Used to distinguish undeclared identifiers for
    /// `typeof` special-casing (which must not throw a `ReferenceError`).
    pub(crate) fn is_declared_name(&self, name: &str) -> bool {
        // Well-known constants
        if matches!(name, "undefined" | "Infinity" | "NaN") {
            return true;
        }
        // Compile-time platform constants
        if is_platform_constant(name) {
            return true;
        }
        // Local scope
        let resolved = if self.capture_env.is_some() {
            self.scopes.resolve_local(name)
        } else {
            self.scopes.resolve(name)
        };
        if resolved.is_some() {
            return true;
        }
        // Captured from parent
        if self.captured_vars.contains_key(name) {
            return true;
        }
        // Built-in globals (console, Math, JSON, etc.)
        crate::globals::is_builtin_global(name)
    }

    /// Resolve a variable for assignment. In strict mode, assigning to an
    /// undeclared variable emits a `ReferenceError` throw and returns `None`.
    /// In sloppy mode, falls back to `resolve_or_declare` (auto-declares).
    /// Assignment to `const` variables emits a `TypeError` and returns `None`.
    pub(crate) fn resolve_for_assignment(&mut self, name: &str) -> Option<u32> {
        // TDZ check: assigning to a let/const variable before its declaration
        if self.tdz_vars.contains(name) {
            self.emit_tdz_error(name);
            return None;
        }

        // Check for const reassignment (applies in both strict and sloppy mode)
        if self.const_vars.contains(name) {
            self.emit_const_assignment_error();
            return None;
        }

        if self.is_strict {
            // In strict mode, check if the variable exists in any scope
            // (local, captured, or built-in global).
            if self.is_declared_name(name) {
                // Variable exists — resolve it normally
                let resolved = if self.capture_env.is_some() {
                    self.scopes
                        .resolve_local(name)
                        .unwrap_or_else(|| self.scopes.resolve_or_declare(name))
                } else {
                    self.scopes.resolve_or_declare(name)
                };
                Some(resolved)
            } else {
                // Undeclared variable in strict mode — emit ReferenceError
                let fn_idx = self.intern_string("__esc_rt_throw_reference_error");
                let fn_id = self.builder.const_string(fn_idx);
                let name_idx = self.intern_string(name);
                let name_id = self.builder.const_string(name_idx);
                self.builder.call_runtime(fn_id, vec![name_id]);
                None
            }
        } else {
            // Sloppy mode — auto-declare if not found (creates a local SSA
            // variable so subsequent reads resolve). The caller may additionally
            // emit a globalThis write for implicit global semantics.
            let var = if self.capture_env.is_some() {
                self.scopes
                    .resolve_local(name)
                    .unwrap_or_else(|| self.scopes.resolve_or_declare(name))
            } else {
                self.scopes.resolve_or_declare(name)
            };
            Some(var)
        }
    }

    /// Emit a `TypeError: Assignment to constant variable.` throw.
    pub(crate) fn emit_const_assignment_error(&mut self) {
        let fn_idx = self.intern_string("__esc_rt_throw_type_error");
        let fn_id = self.builder.const_string(fn_idx);
        let msg_idx = self.intern_string("Assignment to constant variable.");
        let msg_id = self.builder.const_string(msg_idx);
        self.builder.call_runtime(fn_id, vec![msg_id]);
    }

    /// Emit a `ReferenceError: Cannot access '<name>' before initialization` throw.
    pub(crate) fn emit_tdz_error(&mut self, name: &str) {
        let fn_idx = self.intern_string("__esc_rt_throw_tdz_error");
        let fn_id = self.builder.const_string(fn_idx);
        let name_idx = self.intern_string(name);
        let name_id = self.builder.const_string(name_idx);
        self.builder.call_runtime(fn_id, vec![name_id]);
    }

    /// Emit a property set, choosing strict or sloppy mode based on `is_strict`.
    ///
    /// In strict mode, emits `SetPropStrict` which throws `TypeError` when the
    /// property cannot be set (frozen, sealed, or non-extensible object).
    /// In sloppy mode, emits `SetProp` which silently ignores errors.
    pub(crate) fn emit_set_prop(&mut self, obj: ValueId, key: ValueId, val: ValueId) {
        if self.is_strict {
            self.builder.set_prop_strict(obj, key, val);
        } else {
            self.builder.set_prop(obj, key, val);
        }
    }

    /// Set the loop break and continue targets for a new loop iteration.
    ///
    /// If `active_label` is set (meaning this loop is the body of a labeled
    /// statement), also updates that label's `continue_bb` in `label_targets`
    /// so that `continue label` can jump to the correct block.
    pub(crate) fn set_loop_targets(&mut self, break_bb: BlockId, continue_bb: BlockId) {
        self.loop_break_target = Some(break_bb);
        self.loop_continue_target = Some(continue_bb);

        // If this loop is the direct body of a labeled statement, record
        // the continue target so `continue label` works for nested loops.
        if let Some(label) = self.active_label.take()
            && let Some(target) = self.label_targets.get_mut(&label)
        {
            target.continue_bb = Some(continue_bb);
        }
    }

    /// Return the current block, panicking with a BUG message if it is `None`.
    ///
    /// This is an internal invariant: during lowering, `current_block` should
    /// always be `Some` when we need to read it (we only clear it on
    /// termination, and callers guard on `self.terminated`). A `None` here
    /// indicates a compiler bug, not a user error.
    pub(crate) fn current_block_id(&self) -> BlockId {
        let Some(block) = self.current_block else {
            unreachable!(
                "BUG: current_block is None — lowering produced code without an active block"
            );
        };
        block
    }

    /// Write a value to a named variable, updating both the SSA variable
    /// and the closure environment slot if the variable is captured.
    ///
    /// For boxed variables (JsBox captures), the value is written via
    /// `BoxStore` into the JsBox pointer held by the SSA variable, so
    /// the mutation is visible to all closures sharing the same box.
    pub(crate) fn write_var_by_name(&mut self, name: &str, var: u32, val: ValueId) {
        if self.boxed_vars.contains(name) {
            // Variable is boxed — write through the JsBox pointer
            let box_ptr = self.builder.read_variable(var, IrType::JSValue);
            self.builder.box_store(box_ptr, val);
            // No need to update the SSA variable (it still holds the box pointer)
            // and no need to update env slot (env holds the same box pointer)
        } else {
            self.builder.write_variable(var, val);
            // If this variable is captured, also store it back to the env slot
            if let Some(env) = self.capture_env
                && let Some(&slot) = self.captured_vars.get(name)
            {
                self.builder.env_store(env, slot, val);
            }
        }
        // If inside a poisoned function, also store to the EscEnvironment
        // so eval'd code sees the updated value.
        if let Some(env_var) = self.poisoned_env_var
            && let Some(&slot) = self.poisoned_slot_map.get(name)
        {
            let env = self.builder.read_variable(env_var, IrType::JSValue);
            let rt_set_idx = self.intern_string("__esc_rt_esc_env_set_boxed");
            let rt_set_name = self.builder.const_string(rt_set_idx);
            let slot_val = self.builder.const_i32(slot as i32);
            self.builder
                .call_runtime(rt_set_name, vec![env, slot_val, val]);
        }
    }

    /// Read a variable by name, dereferencing through JsBox if the variable
    /// is boxed (captured+mutated). Returns the actual value, not the box pointer.
    pub(crate) fn read_boxed_or_var(&mut self, name: &str, var: u32) -> ValueId {
        if self.boxed_vars.contains(name) {
            let box_ptr = self.builder.read_variable(var, IrType::JSValue);
            self.builder.box_load(box_ptr)
        } else {
            self.builder.read_variable(var, IrType::JSValue)
        }
    }
}

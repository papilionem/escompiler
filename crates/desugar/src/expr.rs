use std::collections::HashMap;

use ir::{IrType, ValueId};
use oxc_ast::ast::{
    ArrayExpressionElement, AssignmentOperator, AssignmentTarget, BinaryOperator, Expression,
    LogicalOperator, ObjectPropertyKind, PropertyKey, SimpleAssignmentTarget, UnaryOperator,
    UpdateOperator,
};

use crate::globals;
use crate::lowerer::IrLowerer;

impl IrLowerer {
    pub fn lower_expression(&mut self, expr: &Expression<'_>) -> ValueId {
        match expr {
            // Every numeric literal is f64. A JavaScript Number IS an f64 — there
            // is no integer type in the language — so narrowing integral literals
            // to i32 was not an optimisation, it was a different language.
            //
            // The previous form took the i32 branch for any integral value in
            // range, which made `7 / 2` compute `3`, `1e5 * 1e5` wrap to
            // 1410065408, and `1 / 0` trap with SIGILL instead of yielding
            // Infinity. See docs/research/51-arithmetic-fix-scoping.md.
            //
            // This is only half the fix: the I32 arithmetic arm in
            // crates/types/src/specialize.rs must go too, because inference types
            // bitwise results as I32, so `(a|0) + (b|0)` would still wrap.
            Expression::NumericLiteral(lit) => self.builder.const_f64(lit.value),

            Expression::StringLiteral(lit) => {
                let idx = self.intern_string(&lit.value);
                self.builder.const_string(idx)
            }

            Expression::BooleanLiteral(lit) => self.builder.const_bool(lit.value),

            Expression::NullLiteral(_) => self.builder.const_null(),

            Expression::Identifier(ident) => {
                let name = ident.name.as_str();
                if name == "undefined" {
                    return self.builder.const_undefined();
                }
                // `globalThis` — emit runtime call to get the global object.
                if name == "globalThis" {
                    let rt_name_idx = self.intern_string("__esc_rt_get_global_this");
                    let rt_name = self.builder.const_string(rt_name_idx);
                    return self.builder.call_runtime(rt_name, vec![]);
                }
                // TDZ check: if the variable is in the temporal dead zone,
                // accessing it is a ReferenceError.
                if self.tdz_vars.contains(name) {
                    self.emit_tdz_error(name);
                    return self.builder.const_undefined();
                }
                // Inside a `with` scope: check if the name is lexically
                // declared within the with body. If not, route through
                // dynamic EscEnvironment lookup.
                if let Some(env_var) = self.with_env_var {
                    // Check for variables declared inside the with body
                    // (let/const — lexically scoped, not affected by with-object)
                    if let Some(var) = self.scopes.resolve_within_with(name) {
                        return self.read_boxed_or_var(name, var);
                    }
                    // Tier 0 optimization: if the with-object is a known literal
                    // and the name is in its property set, emit direct get_prop.
                    if let Some((ref known, obj_val)) = self.with_known_props
                        && known.contains(name)
                    {
                        let key_idx = self.intern_string(name);
                        let key = self.builder.const_string(key_idx);
                        let ic_id = self.next_ic_id();
                        let ic_val = self.builder.const_i32(ic_id as i32);
                        return self.builder.ic_get_prop(obj_val, key, ic_val);
                    }
                    // Not lexically declared in the with body — use dynamic lookup
                    let env = self.builder.read_variable(env_var, IrType::JSValue);
                    let name_idx = self.intern_string(name);
                    let name_val = self.builder.const_string(name_idx);
                    return self.builder.env_lookup(env, name_val);
                }
                // Inside a poisoned function (contains direct eval): variables
                // in the slot map must be read from the EscEnvironment so eval'd
                // code sees the same bindings.
                if let Some(env_var) = self.poisoned_env_var
                    && let Some(&slot) = self.poisoned_slot_map.get(name)
                {
                    let env = self.builder.read_variable(env_var, IrType::JSValue);
                    let rt_get_idx = self.intern_string("__esc_rt_esc_env_get_boxed");
                    let rt_get_name = self.builder.const_string(rt_get_idx);
                    let slot_val = self.builder.const_i32(slot as i32);
                    return self.builder.call_runtime(rt_get_name, vec![env, slot_val]);
                }
                // When inside a closure with captures, use local-only resolution
                // to avoid crossing function boundaries into parent SSA scope.
                let resolved = if self.capture_env.is_some() {
                    self.scopes.resolve_local(name)
                } else {
                    self.scopes.resolve(name)
                };
                if let Some(var) = resolved {
                    self.read_boxed_or_var(name, var)
                } else if let Some(&slot) = self.captured_vars.get(name) {
                    // Variable captured from parent scope — load from environment
                    if let Some(env) = self.capture_env {
                        let env_val = self.builder.env_load(env, slot);
                        // If this captured var is boxed, the env holds a JsBox
                        // pointer — dereference it to get the actual value.
                        if self.boxed_vars.contains(name) {
                            self.builder.box_load(env_val)
                        } else {
                            env_val
                        }
                    } else {
                        self.builder.const_undefined()
                    }
                } else if name == "console" {
                    // `console` is a real namespace object (typeof "object"),
                    // not just a call-position fast path. Resolve it to the
                    // console singleton at runtime so that `typeof console`
                    // and `var f = console.error` read real values (R1-05b).
                    //
                    // Kept separate from the generic LoadGlobal path below
                    // because the runtime global registry (`__esc_rt_get_global`)
                    // intentionally does not create `console`: call-position
                    // `console.log(...)` is lowered to a direct `__esc_rt_console_log`
                    // fast path, and creating `console` eagerly there would be
                    // dead work on every program. `console` is instead created
                    // lazily by `__esc_rt_get_console` when read as a value.
                    let rt_name_idx = self.intern_string("__esc_rt_get_console");
                    let rt_name = self.builder.const_string(rt_name_idx);
                    self.builder.call_runtime(rt_name, vec![])
                } else if globals::is_builtin_global(name) {
                    // Emit LoadGlobal so the runtime resolves the built-in
                    // global to a real object (e.g., Array, Object, Math).
                    let idx = self.intern_string(name);
                    self.builder.load_global(idx)
                } else if let Some(val) = self.emit_platform_constant(name) {
                    // Compile-time platform constants: __esc_platform, __esc_arch,
                    // __esc_build_mode. These are emitted as ConstString values and
                    // are shadowed by local variables (scope resolution above).
                    val
                } else if name == "Infinity" {
                    self.builder.const_f64(f64::INFINITY)
                } else if name == "NaN" {
                    self.builder.const_f64(f64::NAN)
                } else if name == "__filename" || name == "__dirname" {
                    // Placeholder empty strings — real values need module path
                    // info from the pipeline (v0.6+).
                    let idx = self.intern_string("");
                    self.builder.const_string(idx)
                } else if let Some(area) = globals::unimplemented_global(name) {
                    // A global JavaScript defines and we do not implement.
                    //
                    // Falling through to the ReferenceError below would be
                    // technically defensible and practically terrible: the program
                    // compiles at exit 0, then the artifact dies at exit 1 having
                    // printed ZERO bytes on both streams, because uncaught
                    // exceptions currently emit nothing (ESC-62). The user is told
                    // nothing at all.
                    //
                    // Refuse at compile time instead, naming the identifier. This
                    // is rung 1's rule applied: refuse loudly now, work correctly
                    // later — a half-implementation is the only forbidden state.
                    self.refusals.push(crate::Refusal {
                        code: "ESC-E300",
                        message: format!(
                            "`{name}` is a JavaScript global that this compiler does not \
                             implement yet ({area}). Refused at compile time rather than \
                             failing silently at run time."
                        ),
                    });
                    // Never used — compilation stops before codegen — but lowering
                    // must still yield a value.
                    self.builder.const_undefined()
                } else {
                    // Undeclared variable access — emit a runtime ReferenceError
                    // throw. This applies in both strict and sloppy mode (reads of
                    // undeclared vars always throw; only writes differ).
                    //
                    // Deliberately NOT a refusal: node throws here too, so refusing
                    // would diverge from the oracle. The silence is ESC-62's
                    // problem, not this one's.
                    let fn_idx = self.intern_string("__esc_rt_throw_reference_error");
                    let fn_id = self.builder.const_string(fn_idx);
                    let name_idx = self.intern_string(name);
                    let name_id = self.builder.const_string(name_idx);
                    self.builder.call_runtime(fn_id, vec![name_id])
                }
            }

            Expression::BinaryExpression(bin) => {
                let lhs = self.lower_expression(&bin.left);
                let rhs = self.lower_expression(&bin.right);
                match bin.operator {
                    BinaryOperator::Addition => self.builder.add_js(lhs, rhs),
                    BinaryOperator::Subtraction => self.builder.sub_js(lhs, rhs),
                    BinaryOperator::Multiplication => self.builder.mul_js(lhs, rhs),
                    BinaryOperator::Division => self.builder.div_js(lhs, rhs),
                    BinaryOperator::Remainder => self.builder.mod_js(lhs, rhs),
                    BinaryOperator::Exponential => self.builder.exp_js(lhs, rhs),
                    BinaryOperator::StrictEquality => self.builder.eq_strict(lhs, rhs),
                    BinaryOperator::StrictInequality => self.builder.ne_strict(lhs, rhs),
                    BinaryOperator::Equality => self.builder.eq_abstract(lhs, rhs),
                    BinaryOperator::Inequality => self.builder.ne_abstract(lhs, rhs),
                    BinaryOperator::LessThan => self.builder.lt_js(lhs, rhs),
                    BinaryOperator::LessEqualThan => self.builder.le_js(lhs, rhs),
                    BinaryOperator::GreaterThan => self.builder.gt_js(lhs, rhs),
                    BinaryOperator::GreaterEqualThan => self.builder.ge_js(lhs, rhs),
                    BinaryOperator::BitwiseAnd => {
                        let l = self.builder.to_int32(lhs);
                        let r = self.builder.to_int32(rhs);
                        self.builder.bitwise_and(l, r)
                    }
                    BinaryOperator::BitwiseOR => {
                        let l = self.builder.to_int32(lhs);
                        let r = self.builder.to_int32(rhs);
                        self.builder.bitwise_or(l, r)
                    }
                    BinaryOperator::BitwiseXOR => {
                        let l = self.builder.to_int32(lhs);
                        let r = self.builder.to_int32(rhs);
                        self.builder.bitwise_xor(l, r)
                    }
                    BinaryOperator::ShiftLeft => {
                        let l = self.builder.to_int32(lhs);
                        let r = self.builder.to_int32(rhs);
                        self.builder.shift_left(l, r)
                    }
                    BinaryOperator::ShiftRight => {
                        let l = self.builder.to_int32(lhs);
                        let r = self.builder.to_int32(rhs);
                        self.builder.shift_right(l, r)
                    }
                    BinaryOperator::ShiftRightZeroFill => {
                        let l = self.builder.to_int32(lhs);
                        let r = self.builder.to_uint32(rhs);
                        let result = self.builder.shift_right_unsigned(l, r);
                        // >>> always produces an unsigned result per ES spec
                        self.builder.box_unsigned_i32(result)
                    }
                    BinaryOperator::Instanceof => self.builder.instance_of(lhs, rhs),
                    BinaryOperator::In => self.builder.has_prop(rhs, lhs),
                }
            }

            Expression::UnaryExpression(unary) => {
                // Delete must inspect the argument before lowering it
                if matches!(unary.operator, UnaryOperator::Delete) {
                    return self.lower_delete_expression(&unary.argument);
                }
                // typeof on an undeclared identifier must NOT throw per spec
                // (it should return "undefined"). We special-case this here.
                // HOWEVER, typeof on a TDZ variable MUST throw ReferenceError.
                if matches!(unary.operator, UnaryOperator::Typeof)
                    && let Expression::Identifier(ident) = &unary.argument
                    && !self.is_declared_name(ident.name.as_str())
                    && !self.tdz_vars.contains(ident.name.as_str())
                {
                    // Inside a `with` scope, the name might exist on the with-object
                    // even though it's not statically declared. Use dynamic lookup
                    // (which returns `undefined` if not found) then apply typeof.
                    if let Some(env_var) = self.with_env_var {
                        let env = self.builder.read_variable(env_var, IrType::JSValue);
                        let name_idx = self.intern_string(ident.name.as_str());
                        let name_val = self.builder.const_string(name_idx);
                        let looked_up = self.builder.env_lookup(env, name_val);
                        return self.builder.typeof_boxed(looked_up);
                    }
                    let undef = self.builder.const_undefined();
                    return self.builder.typeof_boxed(undef);
                }
                let operand = self.lower_expression(&unary.argument);
                match unary.operator {
                    UnaryOperator::UnaryNegation => self.builder.neg_js(operand),
                    UnaryOperator::BitwiseNot => {
                        let coerced = self.builder.to_int32(operand);
                        self.builder.bitwise_not(coerced)
                    }
                    UnaryOperator::LogicalNot => {
                        let bool_val = self.builder.to_boolean(operand);
                        let false_val = self.builder.const_bool(false);
                        self.builder.eq_strict(bool_val, false_val)
                    }
                    UnaryOperator::Typeof => self.builder.typeof_boxed(operand),
                    UnaryOperator::UnaryPlus => self.builder.to_number(operand),
                    UnaryOperator::Void => self.builder.const_undefined(),
                    UnaryOperator::Delete => {
                        unreachable!("delete handled above")
                    }
                }
            }

            Expression::UpdateExpression(update) => {
                let one = self.builder.const_i32(1);
                match &update.argument {
                    SimpleAssignmentTarget::AssignmentTargetIdentifier(ident) => {
                        let name = ident.name.as_str();
                        // Strict mode: ++/-- on `eval` or `arguments` is SyntaxError
                        if self.is_strict && (name == "eval" || name == "arguments") {
                            self.errors.push(crate::LoweringError {
                                message: format!(
                                    "SyntaxError: Assignment to '{}' in strict mode",
                                    name
                                ),
                            });
                            return self.builder.const_undefined();
                        }
                        // Inside a `with` scope: route through dynamic env
                        if let Some(env_var) = self.with_env_var {
                            if let Some(var) = self.scopes.resolve_within_with(name) {
                                let current = self.read_boxed_or_var(name, var);
                                let new_val = match update.operator {
                                    UpdateOperator::Increment => self.builder.add_js(current, one),
                                    UpdateOperator::Decrement => self.builder.sub_js(current, one),
                                };
                                self.write_var_by_name(name, var, new_val);
                                return if update.prefix { new_val } else { current };
                            }
                            // Tier 0: direct property access for known object literals
                            if let Some((ref known, obj_val)) = self.with_known_props
                                && known.contains(name)
                            {
                                let key_idx = self.intern_string(name);
                                let key = self.builder.const_string(key_idx);
                                let ic_id = self.next_ic_id();
                                let ic_val = self.builder.const_i32(ic_id as i32);
                                let current = self.builder.ic_get_prop(obj_val, key, ic_val);
                                let new_val = match update.operator {
                                    UpdateOperator::Increment => self.builder.add_js(current, one),
                                    UpdateOperator::Decrement => self.builder.sub_js(current, one),
                                };
                                self.emit_set_prop(obj_val, key, new_val);
                                return if update.prefix { new_val } else { current };
                            }
                            // Not in with body — use dynamic lookup + store
                            let env = self.builder.read_variable(env_var, IrType::JSValue);
                            let name_idx = self.intern_string(name);
                            let name_val = self.builder.const_string(name_idx);
                            let current = self.builder.env_lookup(env, name_val);
                            let new_val = match update.operator {
                                UpdateOperator::Increment => self.builder.add_js(current, one),
                                UpdateOperator::Decrement => self.builder.sub_js(current, one),
                            };
                            // Re-emit name for the store call
                            let name_val2 = self.builder.const_string(name_idx);
                            let env2 = self.builder.read_variable(env_var, IrType::JSValue);
                            self.builder.env_lookup_store(env2, name_val2, new_val);
                            return if update.prefix { new_val } else { current };
                        }
                        if let Some(var) = self.resolve_for_assignment(name) {
                            let current = self.read_boxed_or_var(name, var);
                            let new_val = match update.operator {
                                UpdateOperator::Increment => self.builder.add_js(current, one),
                                UpdateOperator::Decrement => self.builder.sub_js(current, one),
                            };
                            self.write_var_by_name(name, var, new_val);
                            if update.prefix { new_val } else { current }
                        } else {
                            // Strict mode ReferenceError was emitted
                            self.builder.const_undefined()
                        }
                    }
                    SimpleAssignmentTarget::StaticMemberExpression(member) => {
                        let obj = self.lower_expression(&member.object);
                        let key_idx = self.intern_string(member.property.name.as_str());
                        let key = self.builder.const_string(key_idx);
                        let ic_id_get = self.next_ic_id();
                        let ic_val_get = self.builder.const_i32(ic_id_get as i32);
                        let current = self.builder.ic_get_prop(obj, key, ic_val_get);
                        let new_val = match update.operator {
                            UpdateOperator::Increment => self.builder.add_js(current, one),
                            UpdateOperator::Decrement => self.builder.sub_js(current, one),
                        };
                        self.emit_set_prop(obj, key, new_val);
                        if update.prefix { new_val } else { current }
                    }
                    SimpleAssignmentTarget::ComputedMemberExpression(member) => {
                        let obj = self.lower_expression(&member.object);
                        let key = self.lower_expression(&member.expression);
                        let current = self.builder.get_elem(obj, key);
                        let new_val = match update.operator {
                            UpdateOperator::Increment => self.builder.add_js(current, one),
                            UpdateOperator::Decrement => self.builder.sub_js(current, one),
                        };
                        self.builder.set_elem(obj, key, new_val);
                        if update.prefix { new_val } else { current }
                    }
                    _ => self.builder.const_undefined(),
                }
            }

            Expression::CallExpression(call) => {
                // Check for well-known global method calls (e.g., console.log)
                if let Expression::StaticMemberExpression(member) = &call.callee
                    && let Expression::Identifier(obj_ident) = &member.object
                {
                    let obj_name = obj_ident.name.as_str();
                    let method_name = member.property.name.as_str();
                    if globals::is_console_method(obj_name, method_name)
                        && let Some(rt_name) = globals::console_runtime_name(method_name)
                    {
                        let name_idx = self.intern_string(rt_name);
                        let func = self.builder.const_string(name_idx);
                        let args: Vec<ValueId> = call
                            .arguments
                            .iter()
                            .filter_map(|arg| arg.as_expression().map(|e| self.lower_expression(e)))
                            .collect();
                        return self.builder.call_runtime(func, args);
                    }
                }

                // super() call in derived constructor — call parent constructor
                if matches!(&call.callee, Expression::Super(_)) {
                    // Lower the parent constructor reference. In a derived class
                    // constructor, the parent constructor was lowered as part of
                    // the class extends clause. The runtime will handle the
                    // [[Construct]] protocol via __esc_rt_super_call.
                    //
                    // We pass undefined as the callee; the runtime's super_call
                    // delegates to call_new. The actual parent constructor is
                    // resolved at runtime from the prototype chain.
                    let this_val = self.builder.this_value();
                    let proto_key_idx = self.intern_string("__proto__");
                    let proto_key = self.builder.const_string(proto_key_idx);
                    let this_proto = self.builder.get_prop(this_val, proto_key);
                    let ctor_key_idx = self.intern_string("constructor");
                    let ctor_key = self.builder.const_string(ctor_key_idx);
                    let parent_ctor = self.builder.get_prop(this_proto, ctor_key);
                    let args: Vec<ValueId> = call
                        .arguments
                        .iter()
                        .filter_map(|arg| arg.as_expression().map(|e| self.lower_expression(e)))
                        .collect();
                    return self.builder.super_call(parent_ctor, args);
                }

                // super.method() call — call parent prototype method with current `this`
                if let Expression::StaticMemberExpression(member) = &call.callee
                    && matches!(&member.object, Expression::Super(_))
                {
                    let this_val = self.builder.this_value();
                    let key_idx = self.intern_string(member.property.name.as_str());
                    let key = self.builder.const_string(key_idx);
                    // Get the method from the parent prototype
                    let method_val = self.builder.get_super(this_val, key);
                    // Call the method with `this` as the receiver
                    let args: Vec<ValueId> = call
                        .arguments
                        .iter()
                        .filter_map(|arg| arg.as_expression().map(|e| self.lower_expression(e)))
                        .collect();
                    return self.builder.call(method_val, args);
                }

                // super[expr]() — computed super method call
                if let Expression::ComputedMemberExpression(member) = &call.callee
                    && matches!(&member.object, Expression::Super(_))
                {
                    let this_val = self.builder.this_value();
                    let key = self.lower_expression(&member.expression);
                    let method_val = self.builder.get_super(this_val, key);
                    let args: Vec<ValueId> = call
                        .arguments
                        .iter()
                        .filter_map(|arg| arg.as_expression().map(|e| self.lower_expression(e)))
                        .collect();
                    return self.builder.call(method_val, args);
                }

                // Method calls: obj.method(args)
                if let Expression::StaticMemberExpression(member) = &call.callee {
                    let obj = self.lower_expression(&member.object);
                    let key_idx = self.intern_string(member.property.name.as_str());
                    let key = self.builder.const_string(key_idx);

                    // Check if any argument is a spread element
                    let has_spread = call
                        .arguments
                        .iter()
                        .any(|arg| matches!(arg, oxc_ast::ast::Argument::SpreadElement(_)));

                    if has_spread {
                        // Build an args array at runtime, expanding spreads, then
                        // invoke __esc_rt_apply_method(obj, key, args_array) so the
                        // receiver is preserved as `this` (mirrors the plain-call
                        // __esc_rt_apply path).
                        let arr = self.lower_spread_args_array(&call.arguments);
                        let apply_name_idx = self.intern_string("__esc_rt_apply_method");
                        let apply_name = self.builder.const_string(apply_name_idx);
                        return self.builder.call_runtime(apply_name, vec![obj, key, arr]);
                    }

                    let args: Vec<ValueId> = call
                        .arguments
                        .iter()
                        .filter_map(|arg| arg.as_expression().map(|e| self.lower_expression(e)))
                        .collect();
                    return self.builder.call_method(obj, key, args);
                }

                // Computed member calls: obj[key](args)
                // Use CallMethod so the receiver (obj) is preserved as `this`,
                // enabling correct dispatch for Generator, Promise, etc.
                if let Expression::ComputedMemberExpression(member) = &call.callee {
                    let obj = self.lower_expression(&member.object);
                    let key = self.lower_expression(&member.expression);

                    // Check if any argument is a spread element
                    let has_spread = call
                        .arguments
                        .iter()
                        .any(|arg| matches!(arg, oxc_ast::ast::Argument::SpreadElement(_)));

                    if has_spread {
                        // Build an args array at runtime, expanding spreads, then
                        // invoke __esc_rt_apply_method(obj, key, args_array) so the
                        // receiver is preserved as `this`.
                        let arr = self.lower_spread_args_array(&call.arguments);
                        let apply_name_idx = self.intern_string("__esc_rt_apply_method");
                        let apply_name = self.builder.const_string(apply_name_idx);
                        return self.builder.call_runtime(apply_name, vec![obj, key, arr]);
                    }

                    let args: Vec<ValueId> = call
                        .arguments
                        .iter()
                        .filter_map(|arg| arg.as_expression().map(|e| self.lower_expression(e)))
                        .collect();
                    return self.builder.call_method(obj, key, args);
                }

                // Private method call: obj.#method(args)
                if let Expression::PrivateFieldExpression(field_expr) = &call.callee {
                    let obj = self.lower_expression(&field_expr.object);
                    let field_name = field_expr.field.name.as_str();
                    let callee_val = if let Some(&pid) = self.private_name_ids.get(field_name) {
                        let private_id = self.builder.const_i32(pid as i32);
                        self.builder.private_field_get(obj, private_id)
                    } else {
                        let key_idx = self.intern_string(field_name);
                        let key = self.builder.const_string(key_idx);
                        self.builder.get_private(obj, key)
                    };
                    let args: Vec<ValueId> = call
                        .arguments
                        .iter()
                        .filter_map(|arg| arg.as_expression().map(|e| self.lower_expression(e)))
                        .collect();
                    return self.builder.call(callee_val, args);
                }

                // Direct eval() call — attempt compile-time inlining (Tier 0).
                //
                // When the callee is the bare identifier `eval` and the first
                // argument is a string literal, we parse the string at compile
                // time and inline the resulting statements into the caller's IR.
                // Non-constant arguments (variables, template literals with
                // expressions) fall through to the runtime `CallEval` opcode.
                if let Expression::Identifier(ident) = &call.callee
                    && ident.name.as_str() == "eval"
                {
                    // Record that eval was encountered in the source.
                    self.has_eval = true;

                    if let Some(result) = self.try_inline_eval(call) {
                        return result;
                    }
                    // Fall through to runtime eval.
                    // If inside a poisoned function, emit CallEvalDirect with
                    // lex_env, var_env, and this_value so eval'd code can see
                    // the enclosing scope's bindings.
                    if let Some(env_var) = self.poisoned_env_var {
                        let code = if let Some(first_arg) = call.arguments.first() {
                            if let Some(e) = first_arg.as_expression() {
                                self.lower_expression(e)
                            } else {
                                self.builder.const_undefined()
                            }
                        } else {
                            self.builder.const_undefined()
                        };
                        let lex_env = self.builder.read_variable(env_var, IrType::JSValue);
                        // var_env is the same as lex_env for now (single env)
                        let var_env = self.builder.read_variable(env_var, IrType::JSValue);
                        let this_val = self.builder.this_value();
                        return self
                            .builder
                            .call_eval_direct(code, lex_env, var_env, this_val);
                    }
                    // Non-poisoned: lower args normally and emit plain CallEval
                    let args: Vec<ValueId> = call
                        .arguments
                        .iter()
                        .filter_map(|arg| arg.as_expression().map(|e| self.lower_expression(e)))
                        .collect();
                    return self.builder.call_eval(args);
                }

                // Detect `Function(...)` called without `new` — also dynamic code.
                if let Expression::Identifier(ident) = &call.callee
                    && ident.name.as_str() == "Function"
                {
                    self.has_function_constructor = true;
                }

                // Generic call (direct function call by name)
                let callee = self.lower_expression(&call.callee);

                // Check if any argument is a spread element
                let has_spread = call
                    .arguments
                    .iter()
                    .any(|arg| matches!(arg, oxc_ast::ast::Argument::SpreadElement(_)));

                if has_spread {
                    let arr = self.lower_spread_args_array(&call.arguments);
                    // Call via __esc_rt_apply(callee, args_array)
                    let apply_name_idx = self.intern_string("__esc_rt_apply");
                    let apply_name = self.builder.const_string(apply_name_idx);
                    self.builder.call_runtime(apply_name, vec![callee, arr])
                } else {
                    let args: Vec<ValueId> = call
                        .arguments
                        .iter()
                        .filter_map(|arg| arg.as_expression().map(|e| self.lower_expression(e)))
                        .collect();
                    self.builder.call(callee, args)
                }
            }

            Expression::StaticMemberExpression(member)
                if matches!(&member.object, Expression::Super(_)) =>
            {
                // super.prop — read from parent prototype
                let this_val = self.builder.this_value();
                let key_idx = self.intern_string(member.property.name.as_str());
                let key = self.builder.const_string(key_idx);
                self.builder.get_super(this_val, key)
            }

            Expression::ComputedMemberExpression(member)
                if matches!(&member.object, Expression::Super(_)) =>
            {
                // super[expr] — computed read from parent prototype
                let this_val = self.builder.this_value();
                let key = self.lower_expression(&member.expression);
                self.builder.get_super(this_val, key)
            }

            Expression::StaticMemberExpression(member) => {
                let obj = self.lower_expression(&member.object);
                let key_idx = self.intern_string(member.property.name.as_str());
                let key = self.builder.const_string(key_idx);
                let ic_id = self.next_ic_id();
                let ic_val = self.builder.const_i32(ic_id as i32);
                self.builder.ic_get_prop(obj, key, ic_val)
            }

            Expression::ComputedMemberExpression(member) => {
                let obj = self.lower_expression(&member.object);
                let key = self.lower_expression(&member.expression);
                self.builder.get_elem(obj, key)
            }

            Expression::AssignmentExpression(assign) => {
                if assign.operator == AssignmentOperator::Assign {
                    // Simple assignment: lhs = rhs
                    let rhs = self.lower_expression(&assign.right);
                    // §13.15.2 SetFunctionName for assignment expressions:
                    // If rhs is an anonymous function/class, infer its name
                    // from the assignment target identifier.
                    if let AssignmentTarget::AssignmentTargetIdentifier(ident) = &assign.left {
                        let is_anonymous = match &assign.right {
                            Expression::FunctionExpression(f) => f.id.is_none(),
                            Expression::ArrowFunctionExpression(_) => true,
                            Expression::ClassExpression(c) => c.id.is_none(),
                            _ => false,
                        };
                        if is_anonymous {
                            let name_key_idx = self.intern_string("name");
                            let name_key = self.builder.const_string(name_key_idx);
                            let name_val_idx = self.intern_string(ident.name.as_str());
                            let name_val = self.builder.const_string(name_val_idx);
                            self.builder.set_prop(rhs, name_key, name_val);
                        }
                    }
                    self.lower_assignment_target(&assign.left, rhs);
                    rhs
                } else if matches!(
                    assign.operator,
                    AssignmentOperator::LogicalAnd
                        | AssignmentOperator::LogicalOr
                        | AssignmentOperator::LogicalNullish
                ) {
                    // Logical assignment (&&=, ||=, ??=) with short-circuit semantics
                    self.lower_logical_assignment(assign)
                } else {
                    // Compound assignment: +=, -=, *=, etc.
                    self.lower_compound_assignment(assign)
                }
            }

            Expression::ConditionalExpression(cond) => {
                let test = self.lower_expression(&cond.test);
                let test_bool = self.builder.to_boolean(test);

                let then_bb = self.builder.create_block();
                let else_bb = self.builder.create_block();
                let merge_bb = self.builder.create_block();

                let branch_block = self.current_block.unwrap_or(ir::BlockId(0));
                self.builder.br_if(test_bool, then_bb, else_bb);

                let temp_var = self.alloc_temp_var();

                self.builder.switch_to_block(then_bb);
                self.builder.add_predecessor(then_bb, branch_block);
                self.current_block = Some(then_bb);
                let then_val = self.lower_expression(&cond.consequent);
                self.builder.write_variable(temp_var, then_val);
                self.builder.br(merge_bb);
                let then_exit = self.current_block_id();
                self.builder.seal_block(then_bb);

                self.builder.switch_to_block(else_bb);
                self.builder.add_predecessor(else_bb, branch_block);
                self.current_block = Some(else_bb);
                let else_val = self.lower_expression(&cond.alternate);
                self.builder.write_variable(temp_var, else_val);
                self.builder.br(merge_bb);
                let else_exit = self.current_block_id();
                self.builder.seal_block(else_bb);

                self.builder.switch_to_block(merge_bb);
                self.builder.add_predecessor(merge_bb, then_exit);
                self.builder.add_predecessor(merge_bb, else_exit);
                self.builder.seal_block(merge_bb);
                self.current_block = Some(merge_bb);

                self.builder.read_variable(temp_var, IrType::JSValue)
            }

            Expression::LogicalExpression(logical) => {
                let lhs = self.lower_expression(&logical.left);

                match logical.operator {
                    LogicalOperator::And => {
                        let lhs_bool = self.builder.to_boolean(lhs);
                        let then_bb = self.builder.create_block();
                        let merge_bb = self.builder.create_block();
                        let branch_block = self.current_block.unwrap_or(ir::BlockId(0));

                        let temp_var = self.alloc_temp_var();
                        self.builder.write_variable(temp_var, lhs);
                        self.builder.br_if(lhs_bool, then_bb, merge_bb);

                        self.builder.switch_to_block(then_bb);
                        self.builder.add_predecessor(then_bb, branch_block);
                        self.current_block = Some(then_bb);
                        let rhs = self.lower_expression(&logical.right);
                        self.builder.write_variable(temp_var, rhs);
                        self.builder.br(merge_bb);
                        let then_exit = self.current_block_id();
                        self.builder.seal_block(then_bb);

                        self.builder.switch_to_block(merge_bb);
                        self.builder.add_predecessor(merge_bb, branch_block);
                        self.builder.add_predecessor(merge_bb, then_exit);
                        self.builder.seal_block(merge_bb);
                        self.current_block = Some(merge_bb);

                        self.builder.read_variable(temp_var, IrType::JSValue)
                    }
                    LogicalOperator::Or => {
                        let lhs_bool = self.builder.to_boolean(lhs);
                        let else_bb = self.builder.create_block();
                        let merge_bb = self.builder.create_block();
                        let branch_block = self.current_block.unwrap_or(ir::BlockId(0));

                        let temp_var = self.alloc_temp_var();
                        self.builder.write_variable(temp_var, lhs);
                        self.builder.br_if(lhs_bool, merge_bb, else_bb);

                        self.builder.switch_to_block(else_bb);
                        self.builder.add_predecessor(else_bb, branch_block);
                        self.current_block = Some(else_bb);
                        let rhs = self.lower_expression(&logical.right);
                        self.builder.write_variable(temp_var, rhs);
                        self.builder.br(merge_bb);
                        let else_exit = self.current_block_id();
                        self.builder.seal_block(else_bb);

                        self.builder.switch_to_block(merge_bb);
                        self.builder.add_predecessor(merge_bb, branch_block);
                        self.builder.add_predecessor(merge_bb, else_exit);
                        self.builder.seal_block(merge_bb);
                        self.current_block = Some(merge_bb);

                        self.builder.read_variable(temp_var, IrType::JSValue)
                    }
                    LogicalOperator::Coalesce => {
                        let is_null = self.builder.is_nullish(lhs);
                        let else_bb = self.builder.create_block();
                        let merge_bb = self.builder.create_block();
                        let branch_block = self.current_block.unwrap_or(ir::BlockId(0));

                        let temp_var = self.alloc_temp_var();
                        self.builder.write_variable(temp_var, lhs);
                        self.builder.br_if(is_null, else_bb, merge_bb);

                        self.builder.switch_to_block(else_bb);
                        self.builder.add_predecessor(else_bb, branch_block);
                        self.current_block = Some(else_bb);
                        let rhs = self.lower_expression(&logical.right);
                        self.builder.write_variable(temp_var, rhs);
                        self.builder.br(merge_bb);
                        let else_exit = self.current_block_id();
                        self.builder.seal_block(else_bb);

                        self.builder.switch_to_block(merge_bb);
                        self.builder.add_predecessor(merge_bb, branch_block);
                        self.builder.add_predecessor(merge_bb, else_exit);
                        self.builder.seal_block(merge_bb);
                        self.current_block = Some(merge_bb);

                        self.builder.read_variable(temp_var, IrType::JSValue)
                    }
                }
            }

            Expression::ArrayExpression(arr) => {
                // Check if any element is a spread
                let has_spread = arr.elements.iter().any(|e| e.is_spread());

                if has_spread {
                    // Build array incrementally, expanding spread elements via runtime
                    let result_arr = self.builder.create_array(vec![]);
                    for elem in &arr.elements {
                        if let ArrayExpressionElement::SpreadElement(spread) = elem {
                            let spread_val = self.lower_expression(&spread.argument);
                            let rt_name_idx = self.intern_string("__esc_rt_spread_into_array");
                            let rt_name = self.builder.const_string(rt_name_idx);
                            self.builder
                                .call_runtime(rt_name, vec![result_arr, spread_val]);
                        } else if let ArrayExpressionElement::Elision(_) = elem {
                            let undef = self.builder.const_undefined();
                            let push_idx = self.intern_string("__esc_rt_array_push");
                            let push_name = self.builder.const_string(push_idx);
                            self.builder
                                .call_runtime(push_name, vec![result_arr, undef]);
                        } else if let Some(expr) = elem.as_expression() {
                            let val = self.lower_expression(expr);
                            let push_idx = self.intern_string("__esc_rt_array_push");
                            let push_name = self.builder.const_string(push_idx);
                            self.builder.call_runtime(push_name, vec![result_arr, val]);
                        }
                    }
                    result_arr
                } else {
                    let elements: Vec<ValueId> = arr
                        .elements
                        .iter()
                        .map(|elem| match elem {
                            ArrayExpressionElement::Elision(_) => self.builder.const_undefined(),
                            _ => {
                                if let Some(expr) = elem.as_expression() {
                                    self.lower_expression(expr)
                                } else {
                                    self.builder.const_undefined()
                                }
                            }
                        })
                        .collect();
                    self.builder.create_array(elements)
                }
            }

            Expression::ObjectExpression(obj) => {
                // Check if all properties are static data properties suitable
                // for the CreateObjectLiteral fast path: no getters/setters, no
                // spreads, no computed keys, no numeric keys — all plain
                // identifier or string-literal keys with Init kind.
                let can_use_literal = !obj.properties.is_empty()
                    && obj.properties.iter().all(|prop| match prop {
                        ObjectPropertyKind::ObjectProperty(p) => {
                            p.kind == oxc_ast::ast::PropertyKind::Init
                                && !p.computed
                                && matches!(
                                    &p.key,
                                    PropertyKey::StaticIdentifier(_)
                                        | PropertyKey::StringLiteral(_)
                                )
                        }
                        _ => false, // SpreadProperty falls back
                    });

                if can_use_literal {
                    // Fast path: emit CreateObjectLiteral with interleaved
                    // [key0, val0, key1, val1, ...] operands.
                    let mut kvpairs = Vec::with_capacity(obj.properties.len() * 2);
                    for prop in &obj.properties {
                        let ObjectPropertyKind::ObjectProperty(p) = prop else {
                            unreachable!("BUG: can_use_literal guard guarantees ObjectProperty");
                        };
                        let key_str = match &p.key {
                            PropertyKey::StaticIdentifier(ident) => ident.name.as_str().to_string(),
                            PropertyKey::StringLiteral(lit) => lit.value.to_string(),
                            _ => unreachable!(
                                "BUG: can_use_literal guard guarantees identifier/string key"
                            ),
                        };
                        let key_idx = self.intern_string(&key_str);
                        let key_val = self.builder.const_string(key_idx);
                        let val = self.lower_expression(&p.value);

                        // Set function.name for methods and anonymous functions
                        let is_anon_fn = matches!(
                            &p.value,
                            Expression::FunctionExpression(f) if f.id.is_none()
                        );
                        let is_arrow = matches!(&p.value, Expression::ArrowFunctionExpression(_));
                        if p.method || is_anon_fn || is_arrow {
                            let nk_idx = self.intern_string("name");
                            let nk = self.builder.const_string(nk_idx);
                            let nv_idx = self.intern_string(&key_str);
                            let nv = self.builder.const_string(nv_idx);
                            self.builder.set_prop(val, nk, nv);
                        }

                        kvpairs.push(key_val);
                        kvpairs.push(val);
                    }
                    self.builder.create_object_literal(kvpairs)
                } else {
                    // Fallback: CreateObject + SetProp for each property.
                    // For getter/setter properties, we collect them by key name
                    // and emit `define_accessor` calls to install them via the
                    // shape-based accessor model.
                    let object = self.builder.create_object();

                    // First pass: identify accessor pairs (get/set for the same key).
                    // We record the indices of getter and setter entries per key
                    // so we can emit a single define_accessor call for paired accessors.
                    let mut accessor_map: HashMap<String, (Option<usize>, Option<usize>)> =
                        HashMap::new();
                    for (i, prop) in obj.properties.iter().enumerate() {
                        if let ObjectPropertyKind::ObjectProperty(p) = prop {
                            let key_name = match &p.key {
                                PropertyKey::StaticIdentifier(ident) => {
                                    Some(ident.name.as_str().to_string())
                                }
                                PropertyKey::StringLiteral(lit) => Some(lit.value.to_string()),
                                _ => None,
                            };
                            if let Some(name) = key_name {
                                if p.kind == oxc_ast::ast::PropertyKind::Get {
                                    let entry = accessor_map.entry(name).or_default();
                                    entry.0 = Some(i);
                                } else if p.kind == oxc_ast::ast::PropertyKind::Set {
                                    let entry = accessor_map.entry(name).or_default();
                                    entry.1 = Some(i);
                                }
                            }
                        }
                    }

                    // Second pass: lower each property. For accessors, accumulate
                    // lowered getter/setter values so we can emit define_accessor
                    // once per key with both getter and setter when available.
                    let mut lowered_getters: HashMap<String, ValueId> = HashMap::new();
                    let mut lowered_setters: HashMap<String, ValueId> = HashMap::new();

                    for (i, prop) in obj.properties.iter().enumerate() {
                        match prop {
                            ObjectPropertyKind::ObjectProperty(p) => {
                                let key_name = match &p.key {
                                    PropertyKey::StaticIdentifier(ident) => {
                                        Some(ident.name.as_str().to_string())
                                    }
                                    PropertyKey::StringLiteral(lit) => Some(lit.value.to_string()),
                                    _ => None,
                                };
                                let key = self.lower_property_key(&p.key);

                                if p.kind == oxc_ast::ast::PropertyKind::Get {
                                    let getter = self.lower_expression(&p.value);
                                    if let Some(name) = &key_name {
                                        // Set function.name to "get <name>"
                                        let getter_name = format!("get {name}");
                                        let nk_idx = self.intern_string("name");
                                        let nk = self.builder.const_string(nk_idx);
                                        let nv_idx = self.intern_string(&getter_name);
                                        let nv = self.builder.const_string(nv_idx);
                                        self.builder.set_prop(getter, nk, nv);
                                        lowered_getters.insert(name.clone(), getter);
                                    }
                                    // Check if paired setter already lowered (setter appeared first)
                                    // or if this is the last entry for this accessor key.
                                    if let Some(name) = &key_name {
                                        let pair = accessor_map.get(name.as_str());
                                        let is_last_for_key = match pair {
                                            Some((_, Some(set_idx))) => i > *set_idx,
                                            Some((_, None)) => true,
                                            None => true,
                                        };
                                        if is_last_for_key {
                                            self.emit_define_accessor(
                                                object,
                                                key,
                                                lowered_getters.get(name).copied(),
                                                lowered_setters.get(name).copied(),
                                            );
                                        }
                                    } else {
                                        // Computed property key — emit define_accessor immediately
                                        // since we can't defer/pair computed keys.
                                        self.emit_define_accessor(object, key, Some(getter), None);
                                    }
                                } else if p.kind == oxc_ast::ast::PropertyKind::Set {
                                    let setter = self.lower_expression(&p.value);
                                    if let Some(name) = &key_name {
                                        // Set function.name to "set <name>"
                                        let setter_name = format!("set {name}");
                                        let nk_idx = self.intern_string("name");
                                        let nk = self.builder.const_string(nk_idx);
                                        let nv_idx = self.intern_string(&setter_name);
                                        let nv = self.builder.const_string(nv_idx);
                                        self.builder.set_prop(setter, nk, nv);
                                        lowered_setters.insert(name.clone(), setter);
                                    }
                                    // Check if paired getter already lowered (getter appeared first)
                                    // or if this is the last entry for this accessor key.
                                    if let Some(name) = &key_name {
                                        let pair = accessor_map.get(name.as_str());
                                        let is_last_for_key = match pair {
                                            Some((Some(get_idx), _)) => i > *get_idx,
                                            Some((None, _)) => true,
                                            None => true,
                                        };
                                        if is_last_for_key {
                                            self.emit_define_accessor(
                                                object,
                                                key,
                                                lowered_getters.get(name).copied(),
                                                lowered_setters.get(name).copied(),
                                            );
                                        }
                                    } else {
                                        // Computed property key — emit define_accessor immediately
                                        self.emit_define_accessor(object, key, None, Some(setter));
                                    }
                                } else {
                                    let val = self.lower_expression(&p.value);
                                    if let Some(name) = &key_name {
                                        let is_anon_fn = matches!(
                                            &p.value,
                                            Expression::FunctionExpression(f) if f.id.is_none()
                                        );
                                        let is_arrow = matches!(
                                            &p.value,
                                            Expression::ArrowFunctionExpression(_)
                                        );
                                        if p.method || is_anon_fn || is_arrow {
                                            let nk_idx = self.intern_string("name");
                                            let nk = self.builder.const_string(nk_idx);
                                            let nv_idx = self.intern_string(name);
                                            let nv = self.builder.const_string(nv_idx);
                                            self.builder.set_prop(val, nk, nv);
                                        }
                                    }
                                    self.builder.set_prop(object, key, val);
                                }
                            }
                            ObjectPropertyKind::SpreadProperty(spread) => {
                                let source = self.lower_expression(&spread.argument);
                                let rt_name_idx = self.intern_string("__esc_rt_spread_into_object");
                                let rt_name = self.builder.const_string(rt_name_idx);
                                self.builder.call_runtime(rt_name, vec![object, source]);
                            }
                        }
                    }
                    object
                }
            }

            Expression::ArrowFunctionExpression(arrow) => self.lower_arrow_function(arrow),

            Expression::FunctionExpression(func) => self.lower_function_expression(func),

            Expression::TemplateLiteral(template) => {
                let mut result = None;
                let quasis = &template.quasis;
                let expressions = &template.expressions;

                for (i, quasi) in quasis.iter().enumerate() {
                    // Use `cooked` value (escape sequences processed) for
                    // regular template literals. Fall back to `raw` only if
                    // cooked is None (invalid escape — shouldn't happen in
                    // non-tagged templates, but be defensive).
                    let text = quasi
                        .value
                        .cooked
                        .as_ref()
                        .map_or_else(|| quasi.value.raw.as_str(), |c| c.as_str());
                    if !text.is_empty() {
                        let idx = self.intern_string(text);
                        let s = self.builder.const_string(idx);
                        result = Some(match result {
                            Some(prev) => self.builder.string_concat(prev, s),
                            None => s,
                        });
                    }

                    if i < expressions.len() {
                        let expr_val = self.lower_expression(&expressions[i]);
                        let str_val = self.builder.to_js_string(expr_val);
                        result = Some(match result {
                            Some(prev) => self.builder.string_concat(prev, str_val),
                            None => str_val,
                        });
                    }
                }

                result.unwrap_or_else(|| {
                    let idx = self.intern_string("");
                    self.builder.const_string(idx)
                })
            }

            Expression::TaggedTemplateExpression(tagged) => {
                // tag`strings${expr}more` → tag(cookedArr, expr1, expr2, ...)
                // where cookedArr has a .raw property with raw strings, and is frozen.
                let tag_fn = self.lower_expression(&tagged.tag);

                // Build the cooked strings array (escape sequences processed).
                // If cooked is None (invalid escape in tagged template), use undefined.
                let cooked_elements: Vec<ValueId> = tagged
                    .quasi
                    .quasis
                    .iter()
                    .map(|quasi| {
                        if let Some(cooked) = &quasi.value.cooked {
                            let idx = self.intern_string(cooked.as_str());
                            self.builder.const_string(idx)
                        } else {
                            self.builder.const_undefined()
                        }
                    })
                    .collect();
                let cooked_arr = self.builder.create_array(cooked_elements);

                // Build the raw strings array (escape sequences preserved as-is).
                let raw_elements: Vec<ValueId> = tagged
                    .quasi
                    .quasis
                    .iter()
                    .map(|quasi| {
                        let raw = quasi.value.raw.as_str();
                        let idx = self.intern_string(raw);
                        self.builder.const_string(idx)
                    })
                    .collect();
                let raw_arr = self.builder.create_array(raw_elements);

                // Set .raw property on the cooked array: cookedArr.raw = rawArr
                let raw_key_idx = self.intern_string("raw");
                let raw_key = self.builder.const_string(raw_key_idx);
                self.builder.set_prop(cooked_arr, raw_key, raw_arr);

                // Freeze the template array: Object.freeze(cookedArr)
                let freeze_name_idx = self.intern_string("freeze");
                let freeze_name = self.builder.const_string(freeze_name_idx);
                let object_name_idx = self.intern_string("Object");
                let object_name = self.builder.const_string(object_name_idx);
                self.builder
                    .call_method(object_name, freeze_name, vec![cooked_arr]);

                // Build args: [cookedArr, ...interpolated_values]
                let mut args = vec![cooked_arr];
                for expr in &tagged.quasi.expressions {
                    args.push(self.lower_expression(expr));
                }
                self.builder.call(tag_fn, args)
            }

            Expression::NewExpression(new_expr) => {
                // Detect `new Function(...)` — record for permission checks.
                if let Expression::Identifier(ident) = &new_expr.callee
                    && ident.name.as_str() == "Function"
                {
                    self.has_function_constructor = true;
                }

                let callee = self.lower_expression(&new_expr.callee);

                // Check if any argument is a spread element
                let has_spread = new_expr
                    .arguments
                    .iter()
                    .any(|arg| matches!(arg, oxc_ast::ast::Argument::SpreadElement(_)));

                if has_spread {
                    // Build an args array at runtime, expanding spreads
                    let create_arr_idx = self.intern_string("__esc_rt_create_empty_array");
                    let create_arr_name = self.builder.const_string(create_arr_idx);
                    let dummy = self.builder.const_undefined();
                    let arr = self.builder.call_runtime(create_arr_name, vec![dummy]);
                    for arg in &new_expr.arguments {
                        match arg {
                            oxc_ast::ast::Argument::SpreadElement(spread) => {
                                let spread_val = self.lower_expression(&spread.argument);
                                let rt_name_idx = self.intern_string("__esc_rt_spread_into_array");
                                let rt_name = self.builder.const_string(rt_name_idx);
                                self.builder.call_runtime(rt_name, vec![arr, spread_val]);
                            }
                            _ => {
                                if let Some(expr) = arg.as_expression() {
                                    let val = self.lower_expression(expr);
                                    let push_idx = self.intern_string("__esc_rt_array_push");
                                    let push_name = self.builder.const_string(push_idx);
                                    self.builder.call_runtime(push_name, vec![arr, val]);
                                }
                            }
                        }
                    }
                    // Call via __esc_rt_apply_new(callee, args_array)
                    let apply_name_idx = self.intern_string("__esc_rt_apply_new");
                    let apply_name = self.builder.const_string(apply_name_idx);
                    self.builder.call_runtime(apply_name, vec![callee, arr])
                } else {
                    let args: Vec<ValueId> = new_expr
                        .arguments
                        .iter()
                        .filter_map(|arg| arg.as_expression().map(|e| self.lower_expression(e)))
                        .collect();
                    self.builder.call_new(callee, args)
                }
            }

            Expression::SequenceExpression(seq) => {
                let mut last = self.builder.const_undefined();
                for expr in &seq.expressions {
                    last = self.lower_expression(expr);
                }
                last
            }

            Expression::ThisExpression(_) => {
                // Inside static initializer blocks, `this` refers to the
                // class constructor rather than the enclosing function's `this`.
                if let Some(override_val) = self.this_override {
                    override_val
                } else {
                    self.builder.this_value()
                }
            }

            Expression::ParenthesizedExpression(paren) => self.lower_expression(&paren.expression),

            Expression::AwaitExpression(await_expr) => {
                let val = self.lower_expression(&await_expr.argument);
                self.builder.await_(val)
            }

            Expression::YieldExpression(yield_expr) => {
                let val = if let Some(arg) = &yield_expr.argument {
                    self.lower_expression(arg)
                } else {
                    self.builder.const_undefined()
                };
                self.builder.yield_(val)
            }

            Expression::ChainExpression(chain) => self.lower_optional_chain(&chain.expression),

            Expression::RegExpLiteral(re) => {
                // Lower /pattern/flags to new RegExp("pattern", "flags")
                let pattern_str = re.regex.pattern.text.to_string();
                let mut flags_str = String::new();
                let flags = re.regex.flags;
                if flags.contains(oxc_ast::ast::RegExpFlags::G) {
                    flags_str.push('g');
                }
                if flags.contains(oxc_ast::ast::RegExpFlags::I) {
                    flags_str.push('i');
                }
                if flags.contains(oxc_ast::ast::RegExpFlags::M) {
                    flags_str.push('m');
                }
                if flags.contains(oxc_ast::ast::RegExpFlags::S) {
                    flags_str.push('s');
                }
                if flags.contains(oxc_ast::ast::RegExpFlags::U) {
                    flags_str.push('u');
                }
                if flags.contains(oxc_ast::ast::RegExpFlags::Y) {
                    flags_str.push('y');
                }
                let pattern_idx = self.intern_string(&pattern_str);
                let pattern_val = self.builder.const_string(pattern_idx);
                let flags_idx = self.intern_string(&flags_str);
                let flags_val = self.builder.const_string(flags_idx);
                let callee_idx = self.intern_string("RegExp");
                let callee = self.builder.const_string(callee_idx);
                self.builder.call_new(callee, vec![pattern_val, flags_val])
            }

            Expression::MetaProperty(meta) => {
                // new.target metaproperty — emits NewTarget opcode
                if meta.meta.name == "new" && meta.property.name == "target" {
                    self.builder.new_target()
                } else if meta.meta.name == "import" && meta.property.name == "meta" {
                    // import.meta — emit ImportMeta opcode
                    self.builder.import_meta()
                } else {
                    self.builder.const_undefined()
                }
            }

            Expression::ClassExpression(class) => self.lower_class_expression(class),

            Expression::Super(_) => {
                // Bare `super` reference outside a call or member expression.
                // This is a syntax error in real JS but the parser may still
                // produce it. Return undefined as a fallback.
                self.builder.const_undefined()
            }

            Expression::PrivateFieldExpression(field_expr) => {
                // obj.#field → PrivateFieldGet(obj, private_id)
                let obj = self.lower_expression(&field_expr.object);
                let field_name = field_expr.field.name.as_str();
                if let Some(&pid) = self.private_name_ids.get(field_name) {
                    let private_id = self.builder.const_i32(pid as i32);
                    self.builder.private_field_get(obj, private_id)
                } else {
                    // Fallback: treat as dynamic private access
                    let key_idx = self.intern_string(field_name);
                    let key = self.builder.const_string(key_idx);
                    self.builder.get_private(obj, key)
                }
            }

            Expression::PrivateInExpression(priv_in) => {
                // #x in obj → PrivateFieldHas(obj, private_id)
                let obj = self.lower_expression(&priv_in.right);
                let field_name = priv_in.left.name.as_str();
                if let Some(&pid) = self.private_name_ids.get(field_name) {
                    let private_id = self.builder.const_i32(pid as i32);
                    self.builder.private_field_has(obj, private_id)
                } else {
                    // Fallback: unknown private name — always false
                    self.builder.const_bool(false)
                }
            }

            Expression::ImportExpression(import_expr) => {
                // import("./mod.js") — dynamic import expression.
                //
                // If the specifier is a string literal (or a template literal
                // with no interpolations), we record it as a compile-time module
                // dependency and emit a runtime call that wraps the module's
                // namespace object in a resolved Promise. Since we AOT-compile
                // all modules, the imported module is already initialized when
                // import() runs, so the Promise resolves synchronously.
                //
                // Non-literal specifiers (variables, interpolated templates)
                // are not supported in v0.6 and emit a TypeError at runtime.
                if let Some(specifier) = self.extract_constant_eval_string(&import_expr.source) {
                    // Record the module dependency so the module graph can
                    // discover it during the build.
                    if !self.dynamic_imports.contains(&specifier) {
                        self.dynamic_imports.push(specifier.clone());
                    }

                    // Emit: __esc_rt_dynamic_import(specifier_string)
                    // At link time the module is already compiled. The runtime
                    // function creates a resolved Promise wrapping the namespace
                    // object. The specifier string is passed so the runtime can
                    // look up the module's namespace in a registry populated
                    // during module initialization.
                    let specifier_idx = self.intern_string(&specifier);
                    let specifier_val = self.builder.const_string(specifier_idx);
                    let rt_name_idx = self.intern_string("__esc_rt_dynamic_import");
                    let rt_name = self.builder.const_string(rt_name_idx);
                    self.builder.call_runtime(rt_name, vec![specifier_val])
                } else {
                    // Non-literal specifier — not supported in AOT compilation.
                    // Emit a TypeError at runtime.
                    let msg = "Dynamic import with non-literal specifier is not supported";
                    let msg_idx = self.intern_string(msg);
                    let msg_val = self.builder.const_string(msg_idx);
                    let rt_name_idx = self.intern_string("__esc_rt_throw_type_error");
                    let rt_name = self.builder.const_string(rt_name_idx);
                    self.builder.call_runtime(rt_name, vec![msg_val])
                }
            }

            _ => self.builder.const_undefined(),
        }
    }

    /// Lower `delete expr`. Emits `DeleteProp`/`DeleteElem` for member
    /// expressions. For identifier operands in sloppy mode, `delete x`
    /// returns `false` for `var`/`let`/`const` bindings (they cannot be
    /// deleted) and calls `__esc_rt_delete_binding` for undeclared/global
    /// identifiers. In strict mode, `delete identifier` is a SyntaxError
    /// (caught by the parser). For all other operands, evaluates for side
    /// effects and returns `true`.
    /// Build the runtime arguments array for a call whose argument list
    /// contains a spread element.
    ///
    /// A spread's length is not known until run time, so the argument list
    /// cannot be lowered to a fixed vector of values. Instead the arguments are
    /// accumulated into a real array: ordinary arguments are pushed in source
    /// order and each `...x` is expanded in place by
    /// `__esc_rt_spread_into_array`, which preserves argument order across the
    /// mix.
    ///
    /// Returns the array value. The caller then invokes whichever
    /// array-accepting runtime entry point matches the call shape —
    /// `__esc_rt_apply` for a plain call, or `__esc_rt_apply_method` when a
    /// receiver has to survive as the `this` value.
    ///
    /// Shared by the plain-call, static-member-call and computed-member-call
    /// paths. It is one behaviour, and keeping it in one place is what stops
    /// the three from drifting — ESC-102 was exactly that drift: the plain-call
    /// path grew spread support and the two member-call paths silently did not,
    /// so `Math.max(...[1,5,3])` returned `-Infinity` at exit 0.
    fn lower_spread_args_array(&mut self, arguments: &[oxc_ast::ast::Argument<'_>]) -> ValueId {
        let create_arr_idx = self.intern_string("__esc_rt_create_empty_array");
        let create_arr_name = self.builder.const_string(create_arr_idx);
        let dummy = self.builder.const_undefined();
        let arr = self.builder.call_runtime(create_arr_name, vec![dummy]);
        for arg in arguments {
            match arg {
                oxc_ast::ast::Argument::SpreadElement(spread) => {
                    let spread_val = self.lower_expression(&spread.argument);
                    let rt_name_idx = self.intern_string("__esc_rt_spread_into_array");
                    let rt_name = self.builder.const_string(rt_name_idx);
                    self.builder.call_runtime(rt_name, vec![arr, spread_val]);
                }
                _ => {
                    if let Some(expr) = arg.as_expression() {
                        let val = self.lower_expression(expr);
                        let push_idx = self.intern_string("__esc_rt_array_push");
                        let push_name = self.builder.const_string(push_idx);
                        self.builder.call_runtime(push_name, vec![arr, val]);
                    }
                }
            }
        }
        arr
    }

    fn lower_delete_expression(&mut self, argument: &Expression<'_>) -> ValueId {
        match argument {
            Expression::StaticMemberExpression(member) => {
                let obj = self.lower_expression(&member.object);
                let key_idx = self.intern_string(member.property.name.as_str());
                let key = self.builder.const_string(key_idx);
                self.builder.delete_prop(obj, key)
            }
            Expression::ComputedMemberExpression(member) => {
                let obj = self.lower_expression(&member.object);
                let key = self.lower_expression(&member.expression);
                // Use delete_prop with a dynamic key — semantically correct
                self.builder.delete_prop(obj, key)
            }
            // Strict mode: `delete identifier` is a SyntaxError per ES spec
            // (12.5.3.1 — it is an early error in strict mode code).
            Expression::Identifier(ident) if self.is_strict => {
                self.errors.push(crate::LoweringError {
                    message: format!(
                        "SyntaxError: Delete of an unqualified identifier '{}' in strict mode",
                        ident.name.as_str()
                    ),
                });
                self.builder.const_undefined()
            }
            // Sloppy mode: `delete identifier` — var/let/const cannot be
            // deleted (returns false); undeclared/global identifiers attempt
            // deletion via runtime call.
            Expression::Identifier(ident) if !self.is_strict => {
                let name = ident.name.as_str();
                if self.is_declared_name(name) {
                    // var/let/const/builtin bindings cannot be deleted
                    self.builder.const_bool(false)
                } else {
                    // Undeclared identifier — try to delete from globalThis
                    let rt_idx = self.intern_string("__esc_rt_delete_binding");
                    let rt_name = self.builder.const_string(rt_idx);
                    let name_idx = self.intern_string(name);
                    let name_val = self.builder.const_string(name_idx);
                    self.builder.call_runtime(rt_name, vec![name_val])
                }
            }
            _ => {
                // `delete <non-member>` evaluates the operand for side effects
                // and returns true
                self.lower_expression(argument);
                self.builder.const_bool(true)
            }
        }
    }

    /// Lower compound assignment operators (`+=`, `-=`, `*=`, etc.).
    fn lower_compound_assignment(
        &mut self,
        assign: &oxc_ast::ast::AssignmentExpression<'_>,
    ) -> ValueId {
        let rhs = self.lower_expression(&assign.right);

        match &assign.left {
            AssignmentTarget::AssignmentTargetIdentifier(ident) => {
                let name = ident.name.as_str();
                // Inside a `with` scope: route through dynamic env
                if let Some(env_var) = self.with_env_var {
                    if let Some(var) = self.scopes.resolve_within_with(name) {
                        let current = self.read_boxed_or_var(name, var);
                        let new_val = self.apply_compound_op(assign.operator, current, rhs);
                        self.write_var_by_name(name, var, new_val);
                        return new_val;
                    }
                    // Tier 0: direct property access for known object literals
                    if let Some((ref known, obj_val)) = self.with_known_props
                        && known.contains(name)
                    {
                        let key_idx = self.intern_string(name);
                        let key = self.builder.const_string(key_idx);
                        let ic_id_get = self.next_ic_id();
                        let ic_val_get = self.builder.const_i32(ic_id_get as i32);
                        let current = self.builder.ic_get_prop(obj_val, key, ic_val_get);
                        let new_val = self.apply_compound_op(assign.operator, current, rhs);
                        self.emit_set_prop(obj_val, key, new_val);
                        return new_val;
                    }
                    // Not in with body — dynamic lookup + store
                    let env = self.builder.read_variable(env_var, IrType::JSValue);
                    let name_idx = self.intern_string(name);
                    let name_val = self.builder.const_string(name_idx);
                    let current = self.builder.env_lookup(env, name_val);
                    let new_val = self.apply_compound_op(assign.operator, current, rhs);
                    let name_val2 = self.builder.const_string(name_idx);
                    let env2 = self.builder.read_variable(env_var, IrType::JSValue);
                    self.builder.env_lookup_store(env2, name_val2, new_val);
                    return new_val;
                }
                if let Some(var) = self.resolve_for_assignment(name) {
                    let current = self.read_boxed_or_var(name, var);
                    let new_val = self.apply_compound_op(assign.operator, current, rhs);
                    self.write_var_by_name(name, var, new_val);
                    new_val
                } else {
                    self.builder.const_undefined()
                }
            }
            AssignmentTarget::StaticMemberExpression(member) => {
                let obj = self.lower_expression(&member.object);
                let key_idx = self.intern_string(member.property.name.as_str());
                let key = self.builder.const_string(key_idx);
                let ic_id_get = self.next_ic_id();
                let ic_val_get = self.builder.const_i32(ic_id_get as i32);
                let current = self.builder.ic_get_prop(obj, key, ic_val_get);
                let new_val = self.apply_compound_op(assign.operator, current, rhs);
                self.emit_set_prop(obj, key, new_val);
                new_val
            }
            AssignmentTarget::ComputedMemberExpression(member) => {
                let obj = self.lower_expression(&member.object);
                let key = self.lower_expression(&member.expression);
                let current = self.builder.get_elem(obj, key);
                let new_val = self.apply_compound_op(assign.operator, current, rhs);
                self.builder.set_elem(obj, key, new_val);
                new_val
            }
            _ => rhs,
        }
    }

    /// Apply the binary operation implied by a compound assignment operator.
    fn apply_compound_op(&mut self, op: AssignmentOperator, lhs: ValueId, rhs: ValueId) -> ValueId {
        match op {
            AssignmentOperator::Addition => self.builder.add_js(lhs, rhs),
            AssignmentOperator::Subtraction => self.builder.sub_js(lhs, rhs),
            AssignmentOperator::Multiplication => self.builder.mul_js(lhs, rhs),
            AssignmentOperator::Division => self.builder.div_js(lhs, rhs),
            AssignmentOperator::Remainder => self.builder.mod_js(lhs, rhs),
            AssignmentOperator::Exponential => self.builder.exp_js(lhs, rhs),
            AssignmentOperator::BitwiseAnd => {
                let l = self.builder.to_int32(lhs);
                let r = self.builder.to_int32(rhs);
                self.builder.bitwise_and(l, r)
            }
            AssignmentOperator::BitwiseOR => {
                let l = self.builder.to_int32(lhs);
                let r = self.builder.to_int32(rhs);
                self.builder.bitwise_or(l, r)
            }
            AssignmentOperator::BitwiseXOR => {
                let l = self.builder.to_int32(lhs);
                let r = self.builder.to_int32(rhs);
                self.builder.bitwise_xor(l, r)
            }
            AssignmentOperator::ShiftLeft => {
                let l = self.builder.to_int32(lhs);
                let r = self.builder.to_int32(rhs);
                self.builder.shift_left(l, r)
            }
            AssignmentOperator::ShiftRight => {
                let l = self.builder.to_int32(lhs);
                let r = self.builder.to_int32(rhs);
                self.builder.shift_right(l, r)
            }
            AssignmentOperator::ShiftRightZeroFill => {
                let l = self.builder.to_int32(lhs);
                let r = self.builder.to_uint32(rhs);
                let result = self.builder.shift_right_unsigned(l, r);
                // >>>= always produces an unsigned result per ES spec
                self.builder.box_unsigned_i32(result)
            }
            _ => self.builder.add_js(lhs, rhs),
        }
    }

    /// Lower logical assignment operators (`&&=`, `||=`, `??=`) with
    /// proper short-circuit semantics.
    fn lower_logical_assignment(
        &mut self,
        assign: &oxc_ast::ast::AssignmentExpression<'_>,
    ) -> ValueId {
        type WriteBack<'a> = Box<dyn FnOnce(&mut IrLowerer, ValueId) + 'a>;
        // Read the current value from the target
        let (current, write_back): (ValueId, WriteBack<'_>) = match &assign.left {
            AssignmentTarget::AssignmentTargetIdentifier(ident) => {
                let name = ident.name.as_str().to_string();
                // Inside a `with` scope: route through dynamic env
                if let Some(env_var) = self.with_env_var {
                    if let Some(var) = self.scopes.resolve_within_with(&name) {
                        let cur = self.read_boxed_or_var(&name, var);
                        (
                            cur,
                            Box::new(move |this: &mut Self, val: ValueId| {
                                this.write_var_by_name(&name, var, val);
                            }),
                        )
                    } else {
                        let env = self.builder.read_variable(env_var, IrType::JSValue);
                        let name_idx = self.intern_string(&name);
                        let name_val = self.builder.const_string(name_idx);
                        let cur = self.builder.env_lookup(env, name_val);
                        (
                            cur,
                            Box::new(move |this: &mut Self, val: ValueId| {
                                if let Some(ev) = this.with_env_var {
                                    let env = this.builder.read_variable(ev, IrType::JSValue);
                                    let nidx = this.intern_string(&name);
                                    let nval = this.builder.const_string(nidx);
                                    this.builder.env_lookup_store(env, nval, val);
                                }
                            }),
                        )
                    }
                } else if let Some(var) = self.resolve_for_assignment(&name) {
                    let cur = self.read_boxed_or_var(&name, var);
                    (
                        cur,
                        Box::new(move |this: &mut Self, val: ValueId| {
                            this.write_var_by_name(&name, var, val);
                        }),
                    )
                } else {
                    let undef = self.builder.const_undefined();
                    (
                        undef,
                        Box::new(|_: &mut Self, _: ValueId| {}) as WriteBack<'_>,
                    )
                }
            }
            AssignmentTarget::StaticMemberExpression(member) => {
                let obj = self.lower_expression(&member.object);
                let key_idx = self.intern_string(member.property.name.as_str());
                let key = self.builder.const_string(key_idx);
                let ic_id_get = self.next_ic_id();
                let ic_val_get = self.builder.const_i32(ic_id_get as i32);
                let cur = self.builder.ic_get_prop(obj, key, ic_val_get);
                (
                    cur,
                    Box::new(move |this: &mut Self, val: ValueId| {
                        this.emit_set_prop(obj, key, val);
                    }),
                )
            }
            AssignmentTarget::ComputedMemberExpression(member) => {
                let obj = self.lower_expression(&member.object);
                let key = self.lower_expression(&member.expression);
                let cur = self.builder.get_elem(obj, key);
                (
                    cur,
                    Box::new(move |this: &mut Self, val: ValueId| {
                        this.builder.set_elem(obj, key, val);
                    }),
                )
            }
            _ => {
                let rhs = self.lower_expression(&assign.right);
                return rhs;
            }
        };

        // Determine the branch condition
        let (condition, branch_on_true) = match assign.operator {
            AssignmentOperator::LogicalAnd => {
                // &&=: evaluate RHS only if LHS is truthy
                (self.builder.to_boolean(current), true)
            }
            AssignmentOperator::LogicalOr => {
                // ||=: evaluate RHS only if LHS is falsy
                (self.builder.to_boolean(current), false)
            }
            AssignmentOperator::LogicalNullish => {
                // ??=: evaluate RHS only if LHS is nullish
                (self.builder.is_nullish(current), true)
            }
            _ => unreachable!("only called for logical assignments"),
        };

        // Build short-circuit CFG
        let assign_bb = self.builder.create_block();
        let merge_bb = self.builder.create_block();
        let branch_block = self.current_block.unwrap_or(ir::BlockId(0));

        let temp_var = self.alloc_temp_var();
        self.builder.write_variable(temp_var, current);

        if branch_on_true {
            self.builder.br_if(condition, assign_bb, merge_bb);
        } else {
            self.builder.br_if(condition, merge_bb, assign_bb);
        }

        // assign_bb: evaluate RHS, write back, branch to merge
        self.builder.switch_to_block(assign_bb);
        self.builder.add_predecessor(assign_bb, branch_block);
        self.current_block = Some(assign_bb);
        let rhs = self.lower_expression(&assign.right);
        write_back(self, rhs);
        self.builder.write_variable(temp_var, rhs);
        self.builder.br(merge_bb);
        let assign_exit = self.current_block_id();
        self.builder.seal_block(assign_bb);

        // merge_bb
        self.builder.switch_to_block(merge_bb);
        self.builder.add_predecessor(merge_bb, branch_block);
        self.builder.add_predecessor(merge_bb, assign_exit);
        self.builder.seal_block(merge_bb);
        self.current_block = Some(merge_bb);

        self.builder.read_variable(temp_var, IrType::JSValue)
    }

    /// Lower an assignment target (destructuring or simple) by writing `val` into it.
    fn lower_assignment_target(&mut self, target: &AssignmentTarget<'_>, val: ValueId) {
        match target {
            AssignmentTarget::AssignmentTargetIdentifier(ident) => {
                let name = ident.name.as_str();
                // Strict mode: assignment to `eval` or `arguments` is SyntaxError
                // per ES spec 12.15.1 (LeftHandSideExpression early errors).
                if self.is_strict && (name == "eval" || name == "arguments") {
                    self.errors.push(crate::LoweringError {
                        message: format!("SyntaxError: Assignment to '{}' in strict mode", name),
                    });
                    return;
                }
                // Inside a `with` scope: check if the name is lexically
                // declared within the with body. If not, route through
                // dynamic EscEnvironment store.
                if let Some(env_var) = self.with_env_var {
                    if let Some(var) = self.scopes.resolve_within_with(name) {
                        self.write_var_by_name(name, var, val);
                    } else if let Some((ref known, obj_val)) = self.with_known_props {
                        if known.contains(name) {
                            // Tier 0: direct property set on the known object
                            let key_idx = self.intern_string(name);
                            let key = self.builder.const_string(key_idx);
                            self.emit_set_prop(obj_val, key, val);
                        } else {
                            let env = self.builder.read_variable(env_var, IrType::JSValue);
                            let name_idx = self.intern_string(name);
                            let name_val = self.builder.const_string(name_idx);
                            self.builder.env_lookup_store(env, name_val, val);
                        }
                    } else {
                        let env = self.builder.read_variable(env_var, IrType::JSValue);
                        let name_idx = self.intern_string(name);
                        let name_val = self.builder.const_string(name_idx);
                        self.builder.env_lookup_store(env, name_val, val);
                    }
                    return;
                }
                // Check if the variable is already declared before resolving,
                // so we can detect implicit globals in sloppy mode.
                let was_declared = self.is_declared_name(name);
                if let Some(var) = self.resolve_for_assignment(name) {
                    self.write_var_by_name(name, var, val);
                    // In sloppy mode, writing to an undeclared variable creates
                    // an implicit global — also write to globalThis.
                    if !self.is_strict && !was_declared {
                        let rt_name_idx = self.intern_string("__esc_rt_get_global_this");
                        let rt_name = self.builder.const_string(rt_name_idx);
                        let global = self.builder.call_runtime(rt_name, vec![]);
                        let key_idx = self.intern_string(name);
                        let key = self.builder.const_string(key_idx);
                        self.builder.set_prop(global, key, val);
                    }
                }
                // else: strict mode ReferenceError was already emitted
            }
            AssignmentTarget::StaticMemberExpression(member)
                if matches!(&member.object, Expression::Super(_)) =>
            {
                // super.prop = val
                let this_val = self.builder.this_value();
                let key_idx = self.intern_string(member.property.name.as_str());
                let key = self.builder.const_string(key_idx);
                self.builder.set_super(this_val, key, val);
            }
            AssignmentTarget::StaticMemberExpression(member) => {
                let obj = self.lower_expression(&member.object);
                let key_idx = self.intern_string(member.property.name.as_str());
                let key = self.builder.const_string(key_idx);
                self.emit_set_prop(obj, key, val);
            }
            AssignmentTarget::ComputedMemberExpression(member)
                if matches!(&member.object, Expression::Super(_)) =>
            {
                // super[expr] = val
                let this_val = self.builder.this_value();
                let key = self.lower_expression(&member.expression);
                self.builder.set_super(this_val, key, val);
            }
            AssignmentTarget::ComputedMemberExpression(member) => {
                let obj = self.lower_expression(&member.object);
                let key = self.lower_expression(&member.expression);
                self.builder.set_elem(obj, key, val);
            }
            AssignmentTarget::ArrayAssignmentTarget(arr) => {
                self.lower_array_assignment_target(arr, val);
            }
            AssignmentTarget::ObjectAssignmentTarget(obj) => {
                self.lower_object_assignment_target(obj, val);
            }
            AssignmentTarget::PrivateFieldExpression(field_expr) => {
                // obj.#field = val → PrivateFieldSet(obj, private_id, val)
                let obj = self.lower_expression(&field_expr.object);
                let field_name = field_expr.field.name.as_str();
                if let Some(&pid) = self.private_name_ids.get(field_name) {
                    let private_id = self.builder.const_i32(pid as i32);
                    self.builder.private_field_set(obj, private_id, val);
                } else {
                    // Fallback: treat as dynamic private access
                    let key_idx = self.intern_string(field_name);
                    let key = self.builder.const_string(key_idx);
                    self.builder.set_private(obj, key, val);
                }
            }
            _ => {
                // TSAsExpression, etc. — not supported, ignore
            }
        }
    }

    /// Lower an `ArrayAssignmentTarget` destructuring pattern.
    ///
    /// Used for both regular assignment (`[a, b] = arr`) and for-of LHS
    /// (`for ([a, b] of pairs)`).
    pub(crate) fn lower_array_assignment_target(
        &mut self,
        arr: &oxc_ast::ast::ArrayAssignmentTarget<'_>,
        val: ValueId,
    ) {
        let elem_count = arr.elements.len();
        for (i, elem) in arr.elements.iter().enumerate() {
            if let Some(elem_target) = elem {
                let idx = self.builder.const_i32(i as i32);
                let elem_val = self.builder.get_elem(val, idx);
                self.lower_assignment_target_maybe_default(elem_target, elem_val);
            }
        }
        // Rest element: [a, ...rest] = arr
        if let Some(rest) = &arr.rest {
            self.lower_array_rest_assignment(&rest.target, val, elem_count);
        }
    }

    /// Lower an `ObjectAssignmentTarget` destructuring pattern.
    ///
    /// Used for both regular assignment (`{a, b} = obj`) and for-of LHS
    /// (`for ({a, b} of objs)`).
    pub(crate) fn lower_object_assignment_target(
        &mut self,
        obj: &oxc_ast::ast::ObjectAssignmentTarget<'_>,
        val: ValueId,
    ) {
        let mut extracted_keys: Vec<String> = Vec::new();
        for prop in &obj.properties {
            match prop {
                oxc_ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(p) => {
                    let name = p.binding.name.as_str();
                    extracted_keys.push(name.to_string());
                    let key_idx = self.intern_string(name);
                    let key = self.builder.const_string(key_idx);
                    let prop_val = self.builder.get_prop(val, key);
                    // Handle default value
                    let final_val = if let Some(default_expr) = &p.init {
                        self.lower_default_value_expr(prop_val, default_expr)
                    } else {
                        prop_val
                    };
                    if let Some(var) = self.resolve_for_assignment(name) {
                        self.write_var_by_name(name, var, final_val);
                    }
                }
                oxc_ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyProperty(p) => {
                    // Extract property value using the appropriate access method
                    let (key_name, prop_val) = match &p.name {
                        oxc_ast::ast::PropertyKey::StaticIdentifier(ident) => {
                            let name = ident.name.as_str().to_string();
                            let key_idx = self.intern_string(&name);
                            let key = self.builder.const_string(key_idx);
                            (name, self.builder.get_prop(val, key))
                        }
                        oxc_ast::ast::PropertyKey::StringLiteral(lit) => {
                            let name = lit.value.to_string();
                            let key_idx = self.intern_string(&name);
                            let key = self.builder.const_string(key_idx);
                            (name, self.builder.get_prop(val, key))
                        }
                        oxc_ast::ast::PropertyKey::NumericLiteral(lit) => {
                            let name = if lit.value.fract() == 0.0 && lit.value.abs() < 1e15 {
                                format!("{}", lit.value as i64)
                            } else {
                                format!("{}", lit.value)
                            };
                            let idx = self.builder.const_f64(lit.value);
                            (name, self.builder.get_elem(val, idx))
                        }
                        _ => {
                            // Computed key: evaluate expression and use get_elem
                            let name = "__computed__".to_string();
                            if let Some(expr) = p.name.as_expression() {
                                let key = self.lower_expression(expr);
                                (name, self.builder.get_elem(val, key))
                            } else {
                                let key = self.builder.const_undefined();
                                (name, self.builder.get_elem(val, key))
                            }
                        }
                    };
                    extracted_keys.push(key_name);
                    self.lower_assignment_target_maybe_default(&p.binding, prop_val);
                }
            }
        }
        // Rest element: { a, ...rest } = obj
        if let Some(rest) = &obj.rest {
            self.lower_object_rest_assignment(&rest.target, val, &extracted_keys);
        }
    }

    /// Lower an `AssignmentTargetMaybeDefault` — handle default values then delegate.
    fn lower_assignment_target_maybe_default(
        &mut self,
        target: &oxc_ast::ast::AssignmentTargetMaybeDefault<'_>,
        val: ValueId,
    ) {
        match target {
            oxc_ast::ast::AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(
                with_default,
            ) => {
                let final_val = self.lower_default_value_expr(val, &with_default.init);
                self.lower_assignment_target(&with_default.binding, final_val);
            }
            // All other variants inherit from AssignmentTarget
            _ => {
                // Use the macro-expanded AssignmentTarget conversion
                // AssignmentTargetMaybeDefault inherits all AssignmentTarget variants
                self.lower_assignment_target_from_maybe_default(target, val);
            }
        }
    }

    /// Convert `AssignmentTargetMaybeDefault` (non-default variants) to assignment target lowering.
    fn lower_assignment_target_from_maybe_default(
        &mut self,
        target: &oxc_ast::ast::AssignmentTargetMaybeDefault<'_>,
        val: ValueId,
    ) {
        use oxc_ast::ast::AssignmentTargetMaybeDefault;
        match target {
            AssignmentTargetMaybeDefault::AssignmentTargetIdentifier(ident) => {
                let name = ident.name.as_str();
                // Inside a `with` scope: route through dynamic env
                if let Some(env_var) = self.with_env_var {
                    if let Some(var) = self.scopes.resolve_within_with(name) {
                        self.write_var_by_name(name, var, val);
                    } else {
                        let env = self.builder.read_variable(env_var, IrType::JSValue);
                        let name_idx = self.intern_string(name);
                        let name_val = self.builder.const_string(name_idx);
                        self.builder.env_lookup_store(env, name_val, val);
                    }
                    return;
                }
                let was_declared = self.is_declared_name(name);
                if let Some(var) = self.resolve_for_assignment(name) {
                    self.write_var_by_name(name, var, val);
                    // In sloppy mode, writing to an undeclared variable creates
                    // an implicit global — also write to globalThis.
                    if !self.is_strict && !was_declared {
                        let rt_name_idx = self.intern_string("__esc_rt_get_global_this");
                        let rt_name = self.builder.const_string(rt_name_idx);
                        let global = self.builder.call_runtime(rt_name, vec![]);
                        let key_idx = self.intern_string(name);
                        let key = self.builder.const_string(key_idx);
                        self.builder.set_prop(global, key, val);
                    }
                }
            }
            AssignmentTargetMaybeDefault::StaticMemberExpression(member) => {
                let obj = self.lower_expression(&member.object);
                let key_idx = self.intern_string(member.property.name.as_str());
                let key = self.builder.const_string(key_idx);
                self.emit_set_prop(obj, key, val);
            }
            AssignmentTargetMaybeDefault::ComputedMemberExpression(member) => {
                let obj = self.lower_expression(&member.object);
                let key = self.lower_expression(&member.expression);
                self.builder.set_elem(obj, key, val);
            }
            AssignmentTargetMaybeDefault::ArrayAssignmentTarget(arr) => {
                self.lower_array_assignment_target(arr, val);
            }
            AssignmentTargetMaybeDefault::ObjectAssignmentTarget(obj) => {
                self.lower_object_assignment_target(obj, val);
            }
            _ => {
                // TSAsExpression, TSNonNullExpression, etc. — ignore
            }
        }
    }

    /// Emit default value: if val is undefined, evaluate default_expr instead.
    ///
    /// Per the ES spec, destructuring defaults trigger on `undefined` only,
    /// not on `null`. Uses strict equality (`===`) with `undefined`.
    fn lower_default_value_expr(
        &mut self,
        val: ValueId,
        default_expr: &oxc_ast::ast::Expression<'_>,
    ) -> ValueId {
        let undef = self.builder.const_undefined();
        let is_undef = self.builder.eq_strict(val, undef);

        let then_bb = self.builder.create_block();
        let merge_bb = self.builder.create_block();
        let branch_block = self.current_block.unwrap_or(ir::BlockId(0));

        let temp_var = self.alloc_temp_var();
        self.builder.write_variable(temp_var, val);
        self.builder.br_if(is_undef, then_bb, merge_bb);

        self.builder.switch_to_block(then_bb);
        self.builder.add_predecessor(then_bb, branch_block);
        self.current_block = Some(then_bb);
        let default_val = self.lower_expression(default_expr);
        self.builder.write_variable(temp_var, default_val);
        self.builder.br(merge_bb);
        let then_exit = self.current_block_id();
        self.builder.seal_block(then_bb);

        self.builder.switch_to_block(merge_bb);
        self.builder.add_predecessor(merge_bb, branch_block);
        self.builder.add_predecessor(merge_bb, then_exit);
        self.builder.seal_block(merge_bb);
        self.current_block = Some(merge_bb);

        self.builder.read_variable(temp_var, IrType::JSValue)
    }

    /// Lower optional chaining expression (`a?.b`, `a?.b()`, `a?.[k]`).
    ///
    /// Handles deeply nested chains like `a?.b?.c?.d` by emitting a nullish
    /// check at each `optional: true` level. All checks short-circuit to a
    /// shared `none_block` that produces `undefined`.
    fn lower_optional_chain(&mut self, chain: &oxc_ast::ast::ChainElement<'_>) -> ValueId {
        // Set up the shared none/merge blocks for the entire chain.
        let none_bb = self.builder.create_block();
        let merge_bb = self.builder.create_block();
        let temp_var = self.alloc_temp_var();
        let undef = self.builder.const_undefined();
        self.builder.write_variable(temp_var, undef);

        // Lower the chain element, emitting nullish checks along the way.
        let result = match chain {
            oxc_ast::ast::ChainElement::CallExpression(call) => {
                // Lower the callee with chain-aware nullish checks.
                let callee = self.lower_chain_expr(&call.callee, none_bb, temp_var);

                // If this call itself is optional, check the callee.
                let callee = if call.optional {
                    self.emit_chain_nullish_check(callee, none_bb, temp_var)
                } else {
                    callee
                };

                let args: Vec<ValueId> = call
                    .arguments
                    .iter()
                    .filter_map(|arg| arg.as_expression().map(|e| self.lower_expression(e)))
                    .collect();
                self.builder.call(callee, args)
            }
            oxc_ast::ast::ChainElement::StaticMemberExpression(member) => {
                self.lower_chain_static_member(member, none_bb, temp_var)
            }
            oxc_ast::ast::ChainElement::ComputedMemberExpression(member) => {
                self.lower_chain_computed_member(member, none_bb, temp_var)
            }
            oxc_ast::ast::ChainElement::PrivateFieldExpression(_) => {
                todo!("Phase D: optional chaining on private fields")
            }
            _ => self.builder.const_undefined(),
        };

        // Write the successful result and branch to merge.
        self.builder.write_variable(temp_var, result);
        self.builder.br(merge_bb);
        let success_exit = self.current_block_id();

        // none_bb: any nullish check that failed jumps here; result is undefined.
        self.builder.switch_to_block(none_bb);
        self.current_block = Some(none_bb);
        self.builder.br(merge_bb);
        let none_exit = self.current_block_id();
        self.builder.seal_block(none_bb);

        // merge_bb: join success and none paths.
        self.builder.switch_to_block(merge_bb);
        self.builder.add_predecessor(merge_bb, success_exit);
        self.builder.add_predecessor(merge_bb, none_exit);
        self.builder.seal_block(merge_bb);
        self.current_block = Some(merge_bb);

        self.builder.read_variable(temp_var, IrType::JSValue)
    }

    /// Lower an expression that appears as the object of an optional chain.
    ///
    /// If the expression is itself an optional member expression (e.g., the
    /// `a?.b` in `a?.b?.c`), this emits the nullish check inline rather than
    /// treating it as a plain property access.
    fn lower_chain_expr(
        &mut self,
        expr: &Expression<'_>,
        none_bb: ir::BlockId,
        temp_var: u32,
    ) -> ValueId {
        match expr {
            Expression::StaticMemberExpression(member) if member.optional => {
                self.lower_chain_static_member(member, none_bb, temp_var)
            }
            Expression::ComputedMemberExpression(member) if member.optional => {
                self.lower_chain_computed_member(member, none_bb, temp_var)
            }
            _ => self.lower_expression(expr),
        }
    }

    /// Lower a static member expression (`obj.prop` or `obj?.prop`) within an
    /// optional chain context, emitting a nullish check if `optional` is true.
    fn lower_chain_static_member(
        &mut self,
        member: &oxc_ast::ast::StaticMemberExpression<'_>,
        none_bb: ir::BlockId,
        temp_var: u32,
    ) -> ValueId {
        // Recursively lower the object with chain-awareness.
        let obj = self.lower_chain_expr(&member.object, none_bb, temp_var);

        // If this level is optional, emit a nullish check.
        let obj = if member.optional {
            self.emit_chain_nullish_check(obj, none_bb, temp_var)
        } else {
            obj
        };

        let key_idx = self.intern_string(member.property.name.as_str());
        let key = self.builder.const_string(key_idx);
        let ic_id = self.next_ic_id();
        let ic_val = self.builder.const_i32(ic_id as i32);
        self.builder.ic_get_prop(obj, key, ic_val)
    }

    /// Lower a computed member expression (`obj[key]` or `obj?.[key]`) within
    /// an optional chain context, emitting a nullish check if `optional` is true.
    fn lower_chain_computed_member(
        &mut self,
        member: &oxc_ast::ast::ComputedMemberExpression<'_>,
        none_bb: ir::BlockId,
        temp_var: u32,
    ) -> ValueId {
        // Recursively lower the object with chain-awareness.
        let obj = self.lower_chain_expr(&member.object, none_bb, temp_var);

        // If this level is optional, emit a nullish check.
        let obj = if member.optional {
            self.emit_chain_nullish_check(obj, none_bb, temp_var)
        } else {
            obj
        };

        let key = self.lower_expression(&member.expression);
        self.builder.get_elem(obj, key)
    }

    /// Emit an inline nullish check within an optional chain.
    ///
    /// If `value` is nullish, branches to `none_bb` (short-circuiting the
    /// entire chain to `undefined`). Otherwise, continues in a new block
    /// with the non-nullish value.
    fn emit_chain_nullish_check(
        &mut self,
        value: ValueId,
        none_bb: ir::BlockId,
        temp_var: u32,
    ) -> ValueId {
        let is_null = self.builder.is_nullish(value);
        let continue_bb = self.builder.create_block();
        let branch_block = self.current_block.unwrap_or(ir::BlockId(0));

        // Write undefined to temp_var before branching so the none path
        // has the correct value if taken.
        let undef = self.builder.const_undefined();
        self.builder.write_variable(temp_var, undef);
        self.builder.br_if(is_null, none_bb, continue_bb);

        // Add predecessor to none_bb from this check point.
        self.builder.add_predecessor(none_bb, branch_block);

        // Continue in the non-nullish path.
        self.builder.switch_to_block(continue_bb);
        self.builder.add_predecessor(continue_bb, branch_block);
        self.builder.seal_block(continue_bb);
        self.current_block = Some(continue_bb);

        value
    }

    /// Lower array rest in an assignment target: `[a, ...rest] = arr`.
    ///
    /// Calls `__esc_rt_array_slice(arr, start_index)` and assigns the
    /// result to the rest target.
    fn lower_array_rest_assignment(
        &mut self,
        target: &oxc_ast::ast::AssignmentTarget<'_>,
        source: ValueId,
        start_index: usize,
    ) {
        let rt_name_idx = self.intern_string("__esc_rt_array_slice");
        let rt_name = self.builder.const_string(rt_name_idx);
        let raw_idx = self.builder.const_i32(start_index as i32);
        let idx = self.builder.box_i32(raw_idx);
        let sliced = self.builder.call_runtime(rt_name, vec![source, idx]);
        self.lower_assignment_target(target, sliced);
    }

    /// Lower object rest in an assignment target: `{ a, ...rest } = obj`.
    ///
    /// Builds an excluded keys array, calls `__esc_rt_object_rest(obj, excluded)`,
    /// and assigns the result to the rest target.
    fn lower_object_rest_assignment(
        &mut self,
        target: &oxc_ast::ast::AssignmentTarget<'_>,
        source: ValueId,
        excluded_keys: &[String],
    ) {
        // Build an array of excluded key strings
        let create_arr_idx = self.intern_string("__esc_rt_create_empty_array");
        let create_arr_name = self.builder.const_string(create_arr_idx);
        let dummy = self.builder.const_undefined();
        let excl_arr = self.builder.call_runtime(create_arr_name, vec![dummy]);

        let push_idx = self.intern_string("__esc_rt_array_push");
        let push_name = self.builder.const_string(push_idx);
        for key in excluded_keys {
            let key_str_idx = self.intern_string(key);
            let key_val = self.builder.const_string(key_str_idx);
            let to_str_idx = self.intern_string("__esc_rt_to_string");
            let to_str_name = self.builder.const_string(to_str_idx);
            let key_str = self.builder.call_runtime(to_str_name, vec![key_val]);
            self.builder
                .call_runtime(push_name, vec![excl_arr, key_str]);
        }

        let rt_name_idx = self.intern_string("__esc_rt_object_rest");
        let rt_name = self.builder.const_string(rt_name_idx);
        let rest_obj = self.builder.call_runtime(rt_name, vec![source, excl_arr]);
        self.lower_assignment_target(target, rest_obj);
    }

    /// Lower a `PropertyKey` AST node to an IR `ValueId`.
    ///
    /// Handles static identifiers, string literals, numeric literals, and
    /// computed keys by delegating to expression lowering.
    fn lower_property_key(&mut self, key: &PropertyKey<'_>) -> ValueId {
        match key {
            PropertyKey::StaticIdentifier(ident) => {
                let idx = self.intern_string(ident.name.as_str());
                self.builder.const_string(idx)
            }
            PropertyKey::StringLiteral(lit) => {
                let idx = self.intern_string(&lit.value);
                self.builder.const_string(idx)
            }
            PropertyKey::NumericLiteral(lit) => self.builder.const_f64(lit.value),
            _ => {
                if let Some(expr) = key.as_expression() {
                    self.lower_expression(expr)
                } else {
                    self.builder.const_undefined()
                }
            }
        }
    }

    /// Emit a `CallRuntime("__esc_rt_define_accessor")` to install a
    /// getter/setter pair on an object via the shape-based accessor model.
    ///
    /// Either `getter` or `setter` (or both) may be `Some`. Missing slots
    /// are passed as `undefined` to the runtime function.
    pub(crate) fn emit_define_accessor(
        &mut self,
        object: ValueId,
        key: ValueId,
        getter: Option<ValueId>,
        setter: Option<ValueId>,
    ) {
        let getter_val = getter.unwrap_or_else(|| self.builder.const_undefined());
        let setter_val = setter.unwrap_or_else(|| self.builder.const_undefined());
        let rt_name_idx = self.intern_string("__esc_rt_define_accessor");
        let rt_name = self.builder.const_string(rt_name_idx);
        self.builder
            .call_runtime(rt_name, vec![object, key, getter_val, setter_val]);
    }

    /// Attempt to inline a direct `eval(...)` call at compile time (Tier 0).
    ///
    /// Returns `Some(ValueId)` if the eval argument is a compile-time constant
    /// string that parses successfully and can be inlined. Returns `None` if
    /// the argument is not a constant string or if parsing fails, signaling
    /// that the caller should fall through to the runtime `CallEval` opcode.
    fn try_inline_eval(&mut self, call: &oxc_ast::ast::CallExpression<'_>) -> Option<ValueId> {
        // eval() with no arguments returns undefined per spec
        if call.arguments.is_empty() {
            return Some(self.builder.const_undefined());
        }

        // Only handle single-argument eval with a constant string literal
        let arg_expr = call.arguments[0].as_expression()?;
        let literal = self.extract_constant_eval_string(arg_expr)?;

        // Empty string eval returns undefined
        if literal.is_empty() {
            return Some(self.builder.const_undefined());
        }

        // Try to parse the eval string as JavaScript script source.
        // Script mode because direct eval inherits the caller's context;
        // strict mode is detected separately.
        let parse_result = parser::parse_with(&literal, oxc_span::SourceType::cjs(), |program| {
            self.lower_eval_body(program)
        });

        // Parse error — fall through to runtime eval
        parse_result.ok()
    }

    /// Extract a compile-time constant string from an expression.
    ///
    /// Returns `Some(String)` for `StringLiteral` nodes and `TemplateLiteral`
    /// nodes with no expressions (pure constant template strings). Returns
    /// `None` for all other expression types.
    fn extract_constant_eval_string(&self, expr: &Expression<'_>) -> Option<String> {
        match expr {
            Expression::StringLiteral(lit) => Some(lit.value.to_string()),
            Expression::TemplateLiteral(tmpl) if tmpl.expressions.is_empty() => {
                // Template literal with only constant quasis (no interpolations)
                let mut result = String::new();
                for quasi in &tmpl.quasis {
                    result.push_str(&quasi.value.raw);
                }
                Some(result)
            }
            _ => None,
        }
    }

    /// Lower the body of a compile-time eval'd program into the caller's IR.
    ///
    /// The eval'd code shares the caller's scope context in sloppy direct eval.
    /// `var` declarations hoist to the caller's variable environment. `let`/`const`
    /// get their own block scope (even in sloppy mode, per spec). In strict mode
    /// eval, even `var` is confined to its own scope.
    fn lower_eval_body(&mut self, program: &oxc_ast::ast::Program<'_>) -> ValueId {
        // Check for "use strict" directive inside the eval code
        let eval_is_strict = self.is_strict
            || program
                .directives
                .iter()
                .any(|d| d.directive.as_str() == "use strict");

        // In strict mode eval, all declarations are confined to a new scope.
        // In sloppy mode eval, `var` goes to the caller's scope but `let`/`const`
        // get a new block scope.
        if eval_is_strict {
            self.scopes.push_scope(crate::scope::ScopeKind::Block);
        }

        // Pre-scan for let/const TDZ names in the eval body
        let (tdz_names, _const_names) = Self::collect_block_lexical_names(&program.body);
        for name in &tdz_names {
            self.tdz_vars.insert(name.clone());
        }

        // Save and set strict mode for the duration of eval body lowering
        let prev_strict = self.is_strict;
        if eval_is_strict {
            self.is_strict = true;
        }

        // Lower each statement. Track the last expression value to return
        // as the completion value of eval (per spec, eval returns the result
        // of the last evaluated expression statement).
        let mut last_value = self.builder.const_undefined();
        for stmt in &program.body {
            if self.terminated {
                break;
            }
            // Capture expression statement values as the eval completion value
            if let oxc_ast::ast::Statement::ExpressionStatement(expr_stmt) = stmt {
                last_value = self.lower_expression(&expr_stmt.expression);
            } else {
                self.lower_statement(stmt);
            }
        }

        // Clean up TDZ vars from the eval body
        for name in &tdz_names {
            self.tdz_vars.remove(name);
        }

        // Restore strict mode
        self.is_strict = prev_strict;

        if eval_is_strict {
            self.scopes.pop_scope();
        }

        last_value
    }
}

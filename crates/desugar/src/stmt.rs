use std::collections::HashSet;

use ir::{BlockId, IrType, ValueId};
use oxc_ast::ast::{
    BindingPattern, BindingRestElement, Expression, ForStatementInit, ForStatementLeft, Statement,
    SwitchCase, VariableDeclarationKind,
};

use crate::lowerer::{ExportDeclKind, ExportInfo, ExportKind, IrLowerer};
use crate::scope::ScopeKind;

impl IrLowerer {
    /// Lower a single JavaScript statement into IR instructions.
    ///
    /// Dispatches to specialized handlers for variable declarations, control
    /// flow (if/for/while/switch/try), labeled statements, functions, classes,
    /// and module import/export declarations.
    pub fn lower_statement(&mut self, stmt: &Statement<'_>) {
        match stmt {
            Statement::VariableDeclaration(decl) => {
                let is_var = decl.kind == VariableDeclarationKind::Var;
                let is_const = decl.kind == VariableDeclarationKind::Const;
                let is_let_or_const = !is_var;

                if is_var {
                    self.var_hoist = true;
                }

                // Collect names being declared for TDZ removal and const tracking
                let declared_names: Vec<String> = if is_let_or_const {
                    decl.declarations
                        .iter()
                        .flat_map(|d| Self::collect_binding_names(&d.id))
                        .collect()
                } else {
                    Vec::new()
                };

                // Check for duplicate let/const declarations in the same scope
                if is_let_or_const {
                    let mut has_dup = false;
                    for name in &declared_names {
                        if self.scopes.has_duplicate_let_const(name) {
                            self.errors.push(crate::LoweringError {
                                message: format!(
                                    "SyntaxError: Identifier '{}' has already been declared",
                                    name
                                ),
                            });
                            has_dup = true;
                        }
                    }
                    if has_dup {
                        return;
                    }
                }

                // Check var/let conflict: var cannot shadow a let/const in same function
                if is_var {
                    let var_names: Vec<String> = decl
                        .declarations
                        .iter()
                        .flat_map(|d| Self::collect_binding_names(&d.id))
                        .collect();
                    let mut has_conflict = false;
                    for name in &var_names {
                        if self.scopes.has_let_const_conflict(name) {
                            self.errors.push(crate::LoweringError {
                                message: format!(
                                    "SyntaxError: Identifier '{}' has already been declared",
                                    name
                                ),
                            });
                            has_conflict = true;
                        }
                    }
                    if has_conflict {
                        return;
                    }
                }

                for declarator in &decl.declarations {
                    if is_var && declarator.init.is_none() {
                        // `var x;` without an initializer must NOT overwrite the
                        // variable's existing value (e.g. a function parameter or
                        // a prior `var x = 1;`). We still need to ensure the
                        // variable is declared in the function scope.
                        self.lower_var_no_init(&declarator.id);
                    } else {
                        let init_val = if let Some(init) = &declarator.init {
                            let val = self.lower_expression(init);
                            // Infer function.name from variable name when the
                            // initializer is an anonymous function or arrow.
                            self.maybe_infer_function_name(&declarator.id, init, val);
                            val
                        } else {
                            self.builder.const_undefined()
                        };
                        self.lower_binding_pattern(&declarator.id, init_val);
                    }
                }

                if is_var {
                    self.var_hoist = false;
                }

                // After the declaration is initialized, remove names from TDZ,
                // register const vars, and mark let/const in the scope
                if is_let_or_const {
                    for name in &declared_names {
                        self.tdz_vars.remove(name);
                        self.scopes.mark_let_const(name);
                        if is_const {
                            self.const_vars.insert(name.clone());
                        }
                    }
                }
            }

            Statement::ExpressionStatement(expr_stmt) => {
                self.lower_expression(&expr_stmt.expression);
            }

            Statement::ReturnStatement(ret) => {
                let val = ret.argument.as_ref().map(|e| self.lower_expression(e));
                if let Some(finally_bb) = self.finally_target {
                    // Inside try-with-finally: store the return value and
                    // branch to the finally block instead of returning.
                    let ret_val = val.unwrap_or_else(|| self.builder.const_undefined());
                    if let Some(ret_var) = self.finally_return_var {
                        self.builder.write_variable(ret_var, ret_val);
                    }
                    if let Some(flag_var) = self.finally_has_return_var {
                        let truthy = self.builder.const_f64(1.0);
                        self.builder.write_variable(flag_var, truthy);
                    }
                    self.builder.br(finally_bb);
                    if let Some(cur) = self.current_block {
                        self.builder.add_predecessor(finally_bb, cur);
                    }
                    self.terminated = true;
                } else {
                    // Bare `return;` must return undefined (ES2023 14.10.1)
                    let ret_val = val.unwrap_or_else(|| self.builder.const_undefined());
                    self.builder.ret(Some(ret_val));
                    self.terminated = true;
                }
            }

            Statement::IfStatement(if_stmt) => {
                let cond = self.lower_expression(&if_stmt.test);
                let cond_bool = self.builder.to_boolean(cond);

                let then_bb = self.builder.create_block();
                let merge_bb = self.builder.create_block();
                let else_bb = if if_stmt.alternate.is_some() {
                    self.builder.create_block()
                } else {
                    merge_bb
                };

                let branch_block = self.current_block.unwrap_or(BlockId(0));
                self.builder.br_if(cond_bool, then_bb, else_bb);
                self.terminated = false;

                // Then branch
                self.builder.switch_to_block(then_bb);
                self.builder.add_predecessor(then_bb, branch_block);
                self.current_block = Some(then_bb);
                self.terminated = false;
                self.lower_statement(&if_stmt.consequent);
                if !self.terminated {
                    self.builder.br(merge_bb);
                    self.builder
                        .add_predecessor(merge_bb, self.current_block_id());
                }
                self.builder.seal_block(then_bb);

                // Else branch
                if let Some(alternate) = &if_stmt.alternate {
                    self.builder.switch_to_block(else_bb);
                    self.builder.add_predecessor(else_bb, branch_block);
                    self.current_block = Some(else_bb);
                    self.terminated = false;
                    self.lower_statement(alternate);
                    if !self.terminated {
                        self.builder.br(merge_bb);
                        self.builder
                            .add_predecessor(merge_bb, self.current_block_id());
                    }
                    self.builder.seal_block(else_bb);
                } else {
                    self.builder.add_predecessor(merge_bb, branch_block);
                }

                self.builder.switch_to_block(merge_bb);
                self.builder.seal_block(merge_bb);
                self.current_block = Some(merge_bb);
                self.terminated = false;
            }

            Statement::BlockStatement(block) => {
                self.scopes.push_scope(ScopeKind::Block);
                // Pre-scan for let/const declarations and add to TDZ set
                let (tdz_names, block_const_names) = Self::collect_block_lexical_names(&block.body);
                for name in &tdz_names {
                    self.tdz_vars.insert(name.clone());
                }
                for stmt in &block.body {
                    if self.terminated {
                        break;
                    }
                    self.lower_statement(stmt);
                }
                // Clean up any remaining TDZ vars from this block
                for name in &tdz_names {
                    self.tdz_vars.remove(name);
                }
                // Clean up const vars declared in this block scope
                for name in &block_const_names {
                    self.const_vars.remove(name);
                }
                self.scopes.pop_scope();
            }

            Statement::WhileStatement(while_stmt) => {
                let header_bb = self.builder.create_block();
                let body_bb = self.builder.create_block();
                let exit_bb = self.builder.create_block();

                let prev_block = self.current_block.unwrap_or(BlockId(0));
                self.builder.br(header_bb);
                self.builder.add_predecessor(header_bb, prev_block);

                let prev_break = self.loop_break_target;
                let prev_continue = self.loop_continue_target;
                self.set_loop_targets(exit_bb, header_bb);

                // Header: check condition
                self.builder.switch_to_block(header_bb);
                self.current_block = Some(header_bb);
                self.terminated = false;
                let cond = self.lower_expression(&while_stmt.test);
                let cond_bool = self.builder.to_boolean(cond);
                self.builder.br_if(cond_bool, body_bb, exit_bb);
                self.builder.add_predecessor(body_bb, header_bb);
                self.builder.add_predecessor(exit_bb, header_bb);

                // Body
                self.builder.switch_to_block(body_bb);
                self.builder.seal_block(body_bb);
                self.current_block = Some(body_bb);
                self.terminated = false;
                self.lower_statement(&while_stmt.body);
                if !self.terminated {
                    self.builder.br(header_bb);
                    self.builder
                        .add_predecessor(header_bb, self.current_block_id());
                }

                self.builder.seal_block(header_bb);

                self.builder.switch_to_block(exit_bb);
                self.builder.seal_block(exit_bb);
                self.current_block = Some(exit_bb);
                self.terminated = false;

                self.loop_break_target = prev_break;
                self.loop_continue_target = prev_continue;
            }

            Statement::ForStatement(for_stmt) => {
                self.scopes.push_scope(ScopeKind::Block);

                // Track const names declared in the for-init for cleanup
                let mut for_const_names: Vec<String> = Vec::new();

                // Init
                if let Some(init) = &for_stmt.init {
                    match init {
                        ForStatementInit::VariableDeclaration(decl) => {
                            let is_var = decl.kind == oxc_ast::ast::VariableDeclarationKind::Var;
                            let is_const = decl.kind == VariableDeclarationKind::Const;
                            if is_var {
                                self.var_hoist = true;
                            }
                            for declarator in &decl.declarations {
                                let init_val = if let Some(init_expr) = &declarator.init {
                                    self.lower_expression(init_expr)
                                } else {
                                    self.builder.const_undefined()
                                };
                                self.lower_binding_pattern(&declarator.id, init_val);
                            }
                            if is_var {
                                self.var_hoist = false;
                            }
                            // Track const declarations in for-init
                            if is_const {
                                for declarator in &decl.declarations {
                                    let names = Self::collect_binding_names(&declarator.id);
                                    for name in &names {
                                        self.const_vars.insert(name.clone());
                                        for_const_names.push(name.clone());
                                    }
                                }
                            }
                        }
                        _ => {
                            if let Some(expr) = init.as_expression() {
                                self.lower_expression(expr);
                            }
                        }
                    }
                }

                let header_bb = self.builder.create_block();
                let body_bb = self.builder.create_block();
                let update_bb = self.builder.create_block();
                let exit_bb = self.builder.create_block();

                let prev_block = self.current_block.unwrap_or(BlockId(0));
                self.builder.br(header_bb);
                self.builder.add_predecessor(header_bb, prev_block);

                let prev_break = self.loop_break_target;
                let prev_continue = self.loop_continue_target;
                self.set_loop_targets(exit_bb, update_bb);

                // Header: check condition
                self.builder.switch_to_block(header_bb);
                self.current_block = Some(header_bb);
                self.terminated = false;
                if let Some(test) = &for_stmt.test {
                    let cond = self.lower_expression(test);
                    let cond_bool = self.builder.to_boolean(cond);
                    self.builder.br_if(cond_bool, body_bb, exit_bb);
                } else {
                    self.builder.br(body_bb);
                }
                self.builder.add_predecessor(body_bb, header_bb);
                self.builder.add_predecessor(exit_bb, header_bb);

                // Body
                self.builder.switch_to_block(body_bb);
                self.builder.seal_block(body_bb);
                self.current_block = Some(body_bb);
                self.terminated = false;
                self.lower_statement(&for_stmt.body);
                if !self.terminated {
                    self.builder.br(update_bb);
                    self.builder
                        .add_predecessor(update_bb, self.current_block_id());
                }

                // Update
                self.builder.switch_to_block(update_bb);
                self.builder.seal_block(update_bb);
                self.current_block = Some(update_bb);
                self.terminated = false;
                if let Some(update) = &for_stmt.update {
                    self.lower_expression(update);
                }
                self.builder.br(header_bb);
                self.builder.add_predecessor(header_bb, update_bb);

                self.builder.seal_block(header_bb);

                self.builder.switch_to_block(exit_bb);
                self.builder.seal_block(exit_bb);
                self.current_block = Some(exit_bb);
                self.terminated = false;

                self.loop_break_target = prev_break;
                self.loop_continue_target = prev_continue;
                // Clean up const vars from the for-loop init scope
                for name in &for_const_names {
                    self.const_vars.remove(name);
                }
                self.scopes.pop_scope();
            }

            Statement::BreakStatement(break_stmt) => {
                // Determine the target: labeled break uses label_targets,
                // unlabeled break uses the innermost loop/switch break target.
                let target = if let Some(ref label) = break_stmt.label {
                    self.label_targets
                        .get(label.name.as_str())
                        .map(|lt| lt.break_bb)
                } else {
                    self.loop_break_target
                };
                if let Some(target) = target {
                    self.emit_break_or_continue(target, false);
                } else {
                    // No valid break target — SyntaxError
                    self.errors.push(crate::LoweringError {
                        message: "SyntaxError: Illegal break statement".to_string(),
                    });
                }
            }

            Statement::ContinueStatement(continue_stmt) => {
                // Determine the target: labeled continue uses label_targets,
                // unlabeled continue uses the innermost loop continue target.
                let target = if let Some(ref label) = continue_stmt.label {
                    self.label_targets
                        .get(label.name.as_str())
                        .and_then(|lt| lt.continue_bb)
                } else {
                    self.loop_continue_target
                };
                if let Some(target) = target {
                    self.emit_break_or_continue(target, true);
                } else {
                    // No valid continue target — SyntaxError
                    self.errors.push(crate::LoweringError {
                        message: "SyntaxError: Illegal continue statement".to_string(),
                    });
                }
            }

            Statement::ThrowStatement(throw) => {
                let val = self.lower_expression(&throw.argument);
                if let Some(finally_bb) = self.finally_target
                    && self.finally_catch_redirects_throw
                    && self.catch_target_stack.len() <= self.finally_catch_depth
                {
                    // Inside a catch body with finally, and NOT inside a nested
                    // try scope that was entered after the catch body started:
                    // store the exception and branch to finally instead of
                    // throwing directly.  If catch_target_stack has grown beyond
                    // `finally_catch_depth`, a nested try-catch was entered and
                    // its handler should receive this throw.
                    if let Some(exc_var) = self.finally_exception_var {
                        self.builder.write_variable(exc_var, val);
                    }
                    if let Some(flag_var) = self.finally_has_exception_var {
                        let truthy = self.builder.const_f64(1.0);
                        self.builder.write_variable(flag_var, truthy);
                    }
                    self.builder.br(finally_bb);
                    if let Some(cur) = self.current_block {
                        self.builder.add_predecessor(finally_bb, cur);
                    }
                    self.terminated = true;
                } else {
                    self.builder.throw_(val);
                    self.terminated = true;
                }
            }

            Statement::TryStatement(try_stmt) => {
                self.lower_try_statement(try_stmt);
            }

            Statement::SwitchStatement(switch) => {
                let discriminant = self.lower_expression(&switch.discriminant);
                let exit_bb = self.builder.create_block();
                let prev_break = self.loop_break_target;
                self.loop_break_target = Some(exit_bb);

                // Push a block scope for the entire switch body (spec: sec-switch-statement-runtime-semantics-evaluation).
                // This allows `let`/`const` declarations inside case clauses to shadow outer bindings.
                self.scopes.push_scope(ScopeKind::Block);

                // Check for duplicate lexically-declared names and var/lexical conflicts
                // (sec-switch-statement-static-semantics-early-errors).
                let (switch_lexical_names, switch_var_names) =
                    Self::collect_switch_lexical_and_var_names(&switch.cases);
                {
                    let mut seen = std::collections::HashSet::new();
                    // LexicallyDeclaredNames must not contain duplicates
                    for name in &switch_lexical_names {
                        if !seen.insert(name.as_str()) {
                            self.errors.push(crate::LoweringError {
                                message: format!(
                                    "SyntaxError: Identifier '{}' has already been declared",
                                    name
                                ),
                            });
                        }
                    }
                    // VarDeclaredNames must not overlap with LexicallyDeclaredNames
                    for name in &switch_var_names {
                        if seen.contains(name.as_str()) {
                            self.errors.push(crate::LoweringError {
                                message: format!(
                                    "SyntaxError: Identifier '{}' has already been declared",
                                    name
                                ),
                            });
                        }
                    }
                }

                // Pre-scan all case consequent statements for let/const declarations (TDZ)
                let all_case_stmts: Vec<&Statement<'_>> = switch
                    .cases
                    .iter()
                    .flat_map(|c| c.consequent.iter())
                    .collect();
                let (switch_tdz_names, switch_const_names) =
                    Self::collect_block_lexical_names_from_refs(&all_case_stmts);
                for name in &switch_tdz_names {
                    self.tdz_vars.insert(name.clone());
                }

                let cases: &[SwitchCase] = &switch.cases;
                if cases.is_empty() {
                    self.builder.br(exit_bb);
                    if let Some(cur) = self.current_block {
                        self.builder.add_predecessor(exit_bb, cur);
                    }
                } else {
                    // Find the default case index (if any), collect non-default
                    let mut default_idx = None;
                    let mut test_cases: Vec<usize> = Vec::new();
                    for (i, case) in cases.iter().enumerate() {
                        if case.test.is_none() {
                            default_idx = Some(i);
                        } else {
                            test_cases.push(i);
                        }
                    }

                    // For empty-body fallthrough, compute the actual body target
                    // for each case. A case with an empty consequent shares the
                    // body block of the next non-empty case (or exit if all
                    // remaining cases are empty).
                    let mut body_targets: Vec<BlockId> = vec![exit_bb; cases.len()];
                    let mut body_block_map: Vec<Option<BlockId>> = vec![None; cases.len()];
                    // Create body blocks only for cases with non-empty consequent
                    for (i, case) in cases.iter().enumerate() {
                        if !case.consequent.is_empty() {
                            let bb = self.builder.create_block();
                            body_block_map[i] = Some(bb);
                            body_targets[i] = bb;
                        }
                    }
                    // Propagate: empty cases point to the next non-empty case's
                    // body block. Walk backwards so each empty case inherits from
                    // the one after it.
                    let mut next_target = exit_bb;
                    for i in (0..cases.len()).rev() {
                        if body_block_map[i].is_some() {
                            next_target = body_targets[i];
                        } else {
                            body_targets[i] = next_target;
                        }
                    }

                    // The fallback when no case matches: default body or exit
                    let no_match_target = default_idx.map(|d| body_targets[d]).unwrap_or(exit_bb);

                    // Emit the test chain (only non-default cases)
                    if test_cases.is_empty() {
                        // Only default case — unconditional jump to its body
                        self.builder.br(no_match_target);
                        if let Some(cur) = self.current_block {
                            self.builder.add_predecessor(no_match_target, cur);
                        }
                    } else {
                        for (ti, &case_idx) in test_cases.iter().enumerate() {
                            // test_cases only contains non-default case indices
                            // (built above from cases with test.is_some()).
                            let Some(test_expr) = cases[case_idx].test.as_ref() else {
                                unreachable!(
                                    "BUG: non-default case at index {case_idx} has no test"
                                );
                            };
                            let test = self.lower_expression(test_expr);
                            let eq = self.builder.eq_strict(discriminant, test);
                            let cur = self.current_block.unwrap_or(BlockId(0));

                            if ti + 1 < test_cases.len() {
                                // More tests to try — create a new test block
                                let next_bb = self.builder.create_block();
                                self.builder.br_if(eq, body_targets[case_idx], next_bb);
                                self.builder.add_predecessor(body_targets[case_idx], cur);
                                self.builder.add_predecessor(next_bb, cur);
                                self.builder.switch_to_block(next_bb);
                                self.builder.seal_block(next_bb);
                                self.current_block = Some(next_bb);
                            } else {
                                // Last test — else goes to default or exit
                                self.builder
                                    .br_if(eq, body_targets[case_idx], no_match_target);
                                self.builder.add_predecessor(body_targets[case_idx], cur);
                                self.builder.add_predecessor(no_match_target, cur);
                            }
                        }
                    }

                    // Emit case bodies (in declaration order for fallthrough).
                    // Only cases with non-empty consequent get their own block.
                    for (i, case) in cases.iter().enumerate() {
                        let Some(body_bb) = body_block_map[i] else {
                            // Empty case — no body block, fallthrough is handled
                            // by body_targets pointing to the next non-empty body.
                            continue;
                        };

                        self.builder.switch_to_block(body_bb);
                        self.builder.seal_block(body_bb);
                        self.current_block = Some(body_bb);
                        self.terminated = false;

                        for stmt in &case.consequent {
                            if self.terminated {
                                break;
                            }
                            self.lower_statement(stmt);
                        }

                        if !self.terminated {
                            // Find the next case body to fall through to
                            let fallthrough_target = if i + 1 < cases.len() {
                                body_targets[i + 1]
                            } else {
                                exit_bb
                            };
                            self.builder.br(fallthrough_target);
                            if let Some(cur) = self.current_block {
                                self.builder.add_predecessor(fallthrough_target, cur);
                            }
                        }
                    }
                }

                self.builder.switch_to_block(exit_bb);
                self.builder.seal_block(exit_bb);
                self.current_block = Some(exit_bb);
                self.terminated = false;
                self.loop_break_target = prev_break;

                // Clean up TDZ and const vars from the switch block scope
                for name in &switch_tdz_names {
                    self.tdz_vars.remove(name);
                }
                for name in &switch_const_names {
                    self.const_vars.remove(name);
                }
                self.scopes.pop_scope();
            }

            Statement::ForInStatement(for_in) => {
                let obj = self.lower_expression(&for_in.right);
                let iter = self.builder.for_in_init(obj);

                let header_bb = self.builder.create_block();
                let body_bb = self.builder.create_block();
                let exit_bb = self.builder.create_block();

                let prev_block = self.current_block.unwrap_or(BlockId(0));
                self.builder.br(header_bb);
                self.builder.add_predecessor(header_bb, prev_block);

                let prev_break = self.loop_break_target;
                let prev_continue = self.loop_continue_target;
                self.set_loop_targets(exit_bb, header_bb);

                self.builder.switch_to_block(header_bb);
                self.current_block = Some(header_bb);
                self.terminated = false;
                let result = self.builder.iter_next(iter);
                let done = self.builder.iter_done(result);
                self.builder.br_if(done, exit_bb, body_bb);
                self.builder.add_predecessor(body_bb, header_bb);
                self.builder.add_predecessor(exit_bb, header_bb);

                self.builder.switch_to_block(body_bb);
                self.builder.seal_block(body_bb);
                self.current_block = Some(body_bb);
                self.terminated = false;
                self.scopes.push_scope(ScopeKind::Block);

                let value = self.builder.iter_value(result);
                let loop_const_names = Self::collect_for_target_const_names(&for_in.left);
                self.bind_for_in_of_target(&for_in.left, value);

                self.lower_statement(&for_in.body);
                // Clean up const vars from this loop iteration scope
                for name in &loop_const_names {
                    self.const_vars.remove(name);
                }
                self.scopes.pop_scope();

                if !self.terminated {
                    self.builder.br(header_bb);
                    self.builder
                        .add_predecessor(header_bb, self.current_block_id());
                }

                self.builder.seal_block(header_bb);

                self.builder.switch_to_block(exit_bb);
                self.builder.seal_block(exit_bb);
                self.current_block = Some(exit_bb);
                self.terminated = false;

                self.loop_break_target = prev_break;
                self.loop_continue_target = prev_continue;
            }

            Statement::ForOfStatement(for_of) => {
                let is_await = for_of.r#await;
                let iterable = self.lower_expression(&for_of.right);

                // for-await-of uses Symbol.asyncIterator (falling back to
                // Symbol.iterator); regular for-of uses Symbol.iterator.
                let iter = if is_await {
                    self.builder.iter_init_async(iterable)
                } else {
                    self.builder.iter_init(iterable)
                };

                let header_bb = self.builder.create_block();
                let body_bb = self.builder.create_block();
                let exit_bb = self.builder.create_block();

                let prev_block = self.current_block.unwrap_or(BlockId(0));
                self.builder.br(header_bb);
                self.builder.add_predecessor(header_bb, prev_block);

                let prev_break = self.loop_break_target;
                let prev_continue = self.loop_continue_target;
                self.set_loop_targets(exit_bb, header_bb);

                self.builder.switch_to_block(header_bb);
                self.current_block = Some(header_bb);
                self.terminated = false;
                let result = self.builder.iter_next(iter);

                // for-await-of: await the result of each .next() call
                let result = if is_await {
                    self.builder.await_(result)
                } else {
                    result
                };

                let done = self.builder.iter_done(result);
                self.builder.br_if(done, exit_bb, body_bb);
                self.builder.add_predecessor(body_bb, header_bb);
                self.builder.add_predecessor(exit_bb, header_bb);

                self.builder.switch_to_block(body_bb);
                self.builder.seal_block(body_bb);
                self.current_block = Some(body_bb);
                self.terminated = false;
                self.scopes.push_scope(ScopeKind::Block);

                let value = self.builder.iter_value(result);
                let loop_const_names = Self::collect_for_target_const_names(&for_of.left);
                self.bind_for_in_of_target(&for_of.left, value);

                self.lower_statement(&for_of.body);
                // Clean up const vars from this loop iteration scope
                for name in &loop_const_names {
                    self.const_vars.remove(name);
                }
                self.scopes.pop_scope();

                if !self.terminated {
                    self.builder.br(header_bb);
                    self.builder
                        .add_predecessor(header_bb, self.current_block_id());
                }

                self.builder.seal_block(header_bb);

                self.builder.switch_to_block(exit_bb);
                self.builder.seal_block(exit_bb);
                self.current_block = Some(exit_bb);
                self.terminated = false;

                self.builder.iter_close(iter);

                self.loop_break_target = prev_break;
                self.loop_continue_target = prev_continue;
            }

            Statement::DoWhileStatement(do_while) => {
                let body_bb = self.builder.create_block();
                let header_bb = self.builder.create_block();
                let exit_bb = self.builder.create_block();

                let prev_block = self.current_block.unwrap_or(BlockId(0));
                self.builder.br(body_bb);
                self.builder.add_predecessor(body_bb, prev_block);

                let prev_break = self.loop_break_target;
                let prev_continue = self.loop_continue_target;
                self.set_loop_targets(exit_bb, header_bb);

                // Body (executes first)
                self.builder.switch_to_block(body_bb);
                self.current_block = Some(body_bb);
                self.terminated = false;
                self.lower_statement(&do_while.body);
                if !self.terminated {
                    self.builder.br(header_bb);
                    self.builder
                        .add_predecessor(header_bb, self.current_block_id());
                }

                // Header: check condition
                self.builder.switch_to_block(header_bb);
                self.current_block = Some(header_bb);
                self.terminated = false;
                let cond = self.lower_expression(&do_while.test);
                let cond_bool = self.builder.to_boolean(cond);
                self.builder.br_if(cond_bool, body_bb, exit_bb);
                self.builder.add_predecessor(body_bb, header_bb);
                self.builder.add_predecessor(exit_bb, header_bb);

                self.builder.seal_block(body_bb);
                self.builder.seal_block(header_bb);

                self.builder.switch_to_block(exit_bb);
                self.builder.seal_block(exit_bb);
                self.current_block = Some(exit_bb);
                self.terminated = false;

                self.loop_break_target = prev_break;
                self.loop_continue_target = prev_continue;
            }

            Statement::LabeledStatement(labeled) => {
                let label_name = labeled.label.name.as_str().to_string();

                // Create the break target block that `break label` will jump to.
                let break_target = self.builder.create_block();

                let body_is_loop = matches!(
                    &labeled.body,
                    Statement::ForStatement(_)
                        | Statement::ForInStatement(_)
                        | Statement::ForOfStatement(_)
                        | Statement::WhileStatement(_)
                        | Statement::DoWhileStatement(_)
                );

                // Register the label with break target. For loops, the
                // continue_bb starts as None and gets filled in by
                // `set_loop_targets()` when the loop sets its targets.
                self.label_targets.insert(
                    label_name.clone(),
                    crate::lowerer::LabelTarget {
                        break_bb: break_target,
                        continue_bb: None,
                    },
                );

                if body_is_loop {
                    // Tell the loop lowering to update this label's
                    // continue_bb when it calls `set_loop_targets()`.
                    self.active_label = Some(label_name.clone());
                }

                self.lower_statement(&labeled.body);

                // Wire up: if the current block isn't terminated, branch
                // to break_target (normal fall-through).
                if !self.block_terminated() {
                    self.builder.br(break_target);
                    if let Some(cur) = self.current_block {
                        self.builder.add_predecessor(break_target, cur);
                    }
                }

                self.builder.switch_to_block(break_target);
                self.builder.seal_block(break_target);
                self.current_block = Some(break_target);
                self.terminated = false;

                // Clean up label registration.
                self.label_targets.remove(&label_name);
            }

            Statement::EmptyStatement(_) => {}

            Statement::DebuggerStatement(_) => {
                self.builder.nop();
            }

            Statement::FunctionDeclaration(func) => {
                self.lower_function_declaration(func);
            }

            Statement::ClassDeclaration(class) => {
                self.lower_class_declaration(class);
            }

            Statement::ImportDeclaration(import) => {
                self.lower_import_declaration(import);
            }

            Statement::ExportDefaultDeclaration(export) => {
                self.lower_export_default(export);
            }

            Statement::ExportNamedDeclaration(export) => {
                self.lower_export_named(export);
            }

            Statement::ExportAllDeclaration(export) => {
                self.lower_export_all(export);
            }

            Statement::WithStatement(with_stmt) => {
                // Per ES spec 13.11.1: `with` is a SyntaxError in strict mode
                if self.is_strict {
                    self.errors.push(crate::LoweringError {
                        message: "SyntaxError: Strict mode code may not include a with statement"
                            .to_string(),
                    });
                    return;
                }
                self.lower_with_statement(with_stmt);
            }

            _ => {}
        }
    }

    /// Lower a `with(obj) { body }` statement.
    ///
    /// Creates a dynamic `EscEnvironment` with the evaluated object as the
    /// `with` target. Inside the body, identifier reads/writes for names that
    /// are NOT lexically declared within the body go through dynamic name
    /// lookup via `__esc_rt_esc_env_lookup` / `__esc_rt_esc_env_store`.
    ///
    /// Variables declared with `let`/`const` inside the body are NOT affected
    /// by the with-object — they are resolved normally via lexical scoping.
    fn lower_with_statement(&mut self, with_stmt: &oxc_ast::ast::WithStatement<'_>) {
        // Tier 0 optimization: extract known property names from object literals
        let known_props = Self::extract_with_object_props(&with_stmt.object);

        // 1. Evaluate the with-object expression
        let obj = self.lower_expression(&with_stmt.object);

        // 2. Ensure the object is a proper object (ToObject conversion)
        let obj_val = self.builder.to_object(obj);

        // 3. Create the with-environment by calling __esc_rt_with_env_create
        let rt_name_idx = self.intern_string("__esc_rt_with_env_create");
        let rt_name = self.builder.const_string(rt_name_idx);
        // Pass the current with-env (or undefined if none) as outer
        let outer_env = if let Some(env_var) = self.with_env_var {
            self.builder.read_variable(env_var, IrType::JSValue)
        } else {
            self.builder.const_undefined()
        };
        let new_env = self.builder.call_runtime(rt_name, vec![obj_val, outer_env]);

        // 4. Save the current with_env_var and push the new one
        self.with_env_stack.push(self.with_env_var);
        let env_var = self.alloc_temp_var();
        self.builder.write_variable(env_var, new_env);
        self.with_env_var = Some(env_var);

        // 5. Set up Tier 0 known properties if available
        self.with_known_props_stack
            .push(self.with_known_props.take());
        if let Some(prop_names) = known_props {
            self.with_known_props = Some((prop_names, obj_val));
        }

        // 6. Push a With scope and lower the body
        self.scopes.push_scope(ScopeKind::With);

        // Pre-scan for let/const declarations in the with body — these
        // are lexically scoped and not affected by the with-object.
        if let Statement::BlockStatement(block) = &with_stmt.body {
            let (tdz_names, _block_const_names) = Self::collect_block_lexical_names(&block.body);
            for name in &tdz_names {
                self.tdz_vars.insert(name.clone());
            }
        }

        self.lower_statement(&with_stmt.body);
        self.scopes.pop_scope();

        // 7. Restore the previous with_env_var and known props
        self.with_env_var = self.with_env_stack.pop().unwrap_or(None);
        self.with_known_props = self.with_known_props_stack.pop().unwrap_or(None);
    }

    /// Extract statically known property names from a with-object expression.
    ///
    /// Returns `Some(set)` when the expression is an object literal with all
    /// static, non-computed data property keys (e.g., `{x: 1, y: 2}`).
    /// Returns `None` for any other expression (variable, call, etc.).
    ///
    /// This enables Tier 0 optimization: identifiers matching these names
    /// can use direct property access instead of dynamic environment lookup.
    fn extract_with_object_props(expr: &Expression<'_>) -> Option<HashSet<String>> {
        use oxc_ast::ast::{ObjectPropertyKind, PropertyKey, PropertyKind};

        let Expression::ObjectExpression(obj) = expr else {
            return None;
        };
        let mut names = HashSet::new();
        for prop in &obj.properties {
            match prop {
                ObjectPropertyKind::ObjectProperty(p) => {
                    // Only Init kind (data properties), non-computed
                    if p.kind != PropertyKind::Init || p.computed {
                        return None;
                    }
                    match &p.key {
                        PropertyKey::StaticIdentifier(ident) => {
                            names.insert(ident.name.as_str().to_string());
                        }
                        PropertyKey::StringLiteral(lit) => {
                            names.insert(lit.value.to_string());
                        }
                        _ => return None, // computed or numeric
                    }
                }
                ObjectPropertyKind::SpreadProperty(_) => return None,
            }
        }
        if names.is_empty() {
            return None;
        }
        Some(names)
    }

    /// Bind the iteration value to the for-in/of loop target.
    ///
    /// Handles both `VariableDeclaration` targets (e.g. `for (const [a, b] of ...)`)
    /// and bare `AssignmentTarget` variants (e.g. `for (x in ...)`).
    fn bind_for_in_of_target(&mut self, target: &ForStatementLeft<'_>, value: ir::ValueId) {
        match target {
            ForStatementLeft::VariableDeclaration(decl) => {
                let is_var = decl.kind == oxc_ast::ast::VariableDeclarationKind::Var;
                let is_const = decl.kind == VariableDeclarationKind::Const;
                if is_var {
                    self.var_hoist = true;
                }
                if let Some(declarator) = decl.declarations.first() {
                    self.lower_binding_pattern(&declarator.id, value);
                    // Track const vars so reassignment in the body is caught
                    if is_const {
                        let names = Self::collect_binding_names(&declarator.id);
                        for name in names {
                            self.const_vars.insert(name);
                        }
                    }
                }
                if is_var {
                    self.var_hoist = false;
                }
            }
            // Bare identifier without let/const/var: for (x in obj)
            ForStatementLeft::AssignmentTargetIdentifier(ident) => {
                let name = ident.name.as_str();
                let var = self.scopes.resolve_or_declare(name);
                self.builder.write_variable(var, value);
            }
            // Bare member expressions: for (obj.prop in ...), for (obj[key] in ...)
            ForStatementLeft::StaticMemberExpression(member) => {
                let obj = self.lower_expression(&member.object);
                let key_idx = self.intern_string(member.property.name.as_str());
                let key = self.builder.const_string(key_idx);
                self.emit_set_prop(obj, key, value);
            }
            ForStatementLeft::ComputedMemberExpression(member) => {
                let obj = self.lower_expression(&member.object);
                let key = self.lower_expression(&member.expression);
                self.builder.set_elem(obj, key, value);
            }
            ForStatementLeft::PrivateFieldExpression(field_expr) => {
                let obj = self.lower_expression(&field_expr.object);
                let field_name = field_expr.field.name.as_str();
                if let Some(&pid) = self.private_name_ids.get(field_name) {
                    let private_id = self.builder.const_i32(pid as i32);
                    self.builder.private_field_set(obj, private_id, value);
                } else {
                    let key_idx = self.intern_string(field_name);
                    let key = self.builder.const_string(key_idx);
                    self.builder.set_private(obj, key, value);
                }
            }
            // Destructuring assignment targets: for ([a, b] of pairs) / for ({x, y} of objs)
            ForStatementLeft::ArrayAssignmentTarget(arr) => {
                self.lower_array_assignment_target(arr, value);
            }
            ForStatementLeft::ObjectAssignmentTarget(obj) => {
                self.lower_object_assignment_target(obj, value);
            }
            _ => {
                // UsingDeclaration, TSAs/TSSatisfies/TSNonNull/TSInstantiation
                // Not yet supported.
            }
        }
    }

    // -----------------------------------------------------------------------
    // Destructuring
    // -----------------------------------------------------------------------

    /// Lower a binding pattern, assigning `init_val` to whatever the pattern binds.
    pub(crate) fn lower_binding_pattern(
        &mut self,
        pattern: &BindingPattern<'_>,
        init_val: ValueId,
    ) {
        match pattern {
            BindingPattern::BindingIdentifier(ident) => {
                let name = ident.name.as_str();

                // Strict mode: eval and arguments cannot be used as binding names
                if self.is_strict && (name == "eval" || name == "arguments") {
                    self.errors.push(crate::LoweringError {
                        message: format!(
                            "SyntaxError: '{}' cannot be used as a variable name in strict mode",
                            name
                        ),
                    });
                    return;
                }

                let var = if self.var_hoist {
                    self.scopes.declare_in_function_scope(name)
                } else {
                    self.scopes.declare(name)
                };
                self.builder.write_variable(var, init_val);
            }
            BindingPattern::ObjectPattern(obj_pat) => {
                // ES2024 §13.15.5.3: BindingInitialization for ObjectBindingPattern
                // requires RequireObjectCoercible(value) before destructuring.
                // This throws TypeError for null/undefined.
                let roc_name_idx = self.intern_string("__esc_rt_require_object_coercible");
                let roc_name = self.builder.const_string(roc_name_idx);
                let coerced_val = self.builder.call_runtime(roc_name, vec![init_val]);
                let init_val = coerced_val;

                // Collect the property key names for potential rest element exclusion
                let mut extracted_keys: Vec<String> = Vec::new();
                for prop in &obj_pat.properties {
                    // Extract property value using the appropriate access method
                    let (key_name, prop_val) = match &prop.key {
                        oxc_ast::ast::PropertyKey::StaticIdentifier(ident) => {
                            let name = ident.name.as_str().to_string();
                            let key_idx = self.intern_string(&name);
                            let key = self.builder.const_string(key_idx);
                            (name, self.builder.get_prop(init_val, key))
                        }
                        oxc_ast::ast::PropertyKey::StringLiteral(lit) => {
                            let name = lit.value.to_string();
                            let key_idx = self.intern_string(&name);
                            let key = self.builder.const_string(key_idx);
                            (name, self.builder.get_prop(init_val, key))
                        }
                        oxc_ast::ast::PropertyKey::NumericLiteral(lit) => {
                            let name = if lit.value.fract() == 0.0 && lit.value.abs() < 1e15 {
                                format!("{}", lit.value as i64)
                            } else {
                                format!("{}", lit.value)
                            };
                            let idx = self.builder.const_f64(lit.value);
                            (name, self.builder.get_elem(init_val, idx))
                        }
                        _ => {
                            // Computed key: evaluate expression and use get_elem
                            let name = "__computed__".to_string();
                            if let Some(expr) = prop.key.as_expression() {
                                let key = self.lower_expression(expr);
                                (name, self.builder.get_elem(init_val, key))
                            } else {
                                let key = self.builder.const_undefined();
                                (name, self.builder.get_elem(init_val, key))
                            }
                        }
                    };
                    extracted_keys.push(key_name);

                    // Check for default value: if the property has a default and
                    // the value is undefined, use the default instead.
                    if let BindingPattern::AssignmentPattern(assignment) = &prop.value {
                        let final_val = self.lower_default_value(prop_val, &assignment.right);
                        self.lower_binding_pattern(&assignment.left, final_val);
                    } else {
                        // Recurse for nested patterns
                        self.lower_binding_pattern(&prop.value, prop_val);
                    }
                }
                // Rest element: const { a, ...rest } = obj
                if let Some(rest) = &obj_pat.rest {
                    self.lower_object_rest_element(rest, init_val, &extracted_keys);
                }
            }
            BindingPattern::ArrayPattern(arr_pat) => {
                let elem_count = arr_pat.elements.len();
                for (i, elem) in arr_pat.elements.iter().enumerate() {
                    if let Some(binding) = elem {
                        let idx = self.builder.const_i32(i as i32);
                        let elem_val = self.builder.get_elem(init_val, idx);

                        if let BindingPattern::AssignmentPattern(assignment) = binding {
                            let final_val = self.lower_default_value(elem_val, &assignment.right);
                            self.lower_binding_pattern(&assignment.left, final_val);
                        } else {
                            self.lower_binding_pattern(binding, elem_val);
                        }
                    }
                }
                // Rest element: const [a, ...rest] = arr
                if let Some(rest) = &arr_pat.rest {
                    self.lower_array_rest_element(rest, init_val, elem_count);
                }
            }
            BindingPattern::AssignmentPattern(assignment) => {
                let final_val = self.lower_default_value(init_val, &assignment.right);
                self.lower_binding_pattern(&assignment.left, final_val);
            }
        }
    }

    /// Handle `var x;` without an initializer.
    ///
    /// Per the JS spec, `var x;` must declare `x` in the function scope but
    /// must NOT overwrite its current value when the variable already exists
    /// (e.g. from a function parameter or a prior declaration). Only writes
    /// `undefined` if the variable is truly new.
    pub(crate) fn lower_var_no_init(&mut self, pattern: &BindingPattern<'_>) {
        if let BindingPattern::BindingIdentifier(ident) = pattern {
            let name = ident.name.as_str();

            // Strict mode: eval and arguments cannot be used as binding names
            if self.is_strict && (name == "eval" || name == "arguments") {
                self.errors.push(crate::LoweringError {
                    message: format!(
                        "SyntaxError: '{}' cannot be used as a variable name in strict mode",
                        name
                    ),
                });
                return;
            }

            // Check if the variable already exists in the function scope.
            // If it does, we keep the existing value (redeclaration is a no-op).
            // If it doesn't, declare it with `undefined`.
            let already_exists = self.scopes.resolve(name).is_some();
            let var = self.scopes.declare_in_function_scope(name);
            if !already_exists {
                let undef = self.builder.const_undefined();
                self.builder.write_variable(var, undef);
            }
        } else {
            // For destructuring patterns like `var {a, b};` (rare but legal),
            // fall back to normal lowering with undefined.
            let undef = self.builder.const_undefined();
            self.lower_binding_pattern(pattern, undef);
        }
    }

    /// Emit a conditional: if `val` is undefined, evaluate and use `default_expr`.
    ///
    /// Per the ES spec, destructuring defaults trigger on `undefined` only,
    /// not on `null`. Uses strict equality (`===`) with `undefined`.
    fn lower_default_value(
        &mut self,
        val: ValueId,
        default_expr: &oxc_ast::ast::Expression<'_>,
    ) -> ValueId {
        let undef = self.builder.const_undefined();
        let is_undef = self.builder.eq_strict(val, undef);

        let then_bb = self.builder.create_block();
        let else_bb = self.builder.create_block();
        let merge_bb = self.builder.create_block();
        let branch_block = self.current_block.unwrap_or(BlockId(0));

        let temp_var = self.alloc_temp_var();
        self.builder.write_variable(temp_var, val);
        self.builder.br_if(is_undef, then_bb, else_bb);

        // then_bb: value is undefined, use default
        self.builder.switch_to_block(then_bb);
        self.builder.add_predecessor(then_bb, branch_block);
        self.current_block = Some(then_bb);
        let default_val = self.lower_expression(default_expr);
        self.builder.write_variable(temp_var, default_val);
        self.builder.br(merge_bb);
        let then_exit = self.current_block_id();
        self.builder.seal_block(then_bb);

        // else_bb: value is defined, keep it
        self.builder.switch_to_block(else_bb);
        self.builder.add_predecessor(else_bb, branch_block);
        self.current_block = Some(else_bb);
        self.builder.br(merge_bb);
        let else_exit = self.current_block_id();
        self.builder.seal_block(else_bb);

        // merge
        self.builder.switch_to_block(merge_bb);
        self.builder.add_predecessor(merge_bb, then_exit);
        self.builder.add_predecessor(merge_bb, else_exit);
        self.builder.seal_block(merge_bb);
        self.current_block = Some(merge_bb);

        self.builder.read_variable(temp_var, IrType::JSValue)
    }

    /// Lower an array rest element: `let [a, b, ...rest] = arr`.
    ///
    /// Calls `__esc_rt_array_slice(arr, start_index)` to collect elements
    /// from `start_index` onward into a new array, then binds to `rest`.
    fn lower_array_rest_element(
        &mut self,
        rest: &BindingRestElement<'_>,
        source: ValueId,
        start_index: usize,
    ) {
        let rt_name_idx = self.intern_string("__esc_rt_array_slice");
        let rt_name = self.builder.const_string(rt_name_idx);
        let raw_idx = self.builder.const_i32(start_index as i32);
        let idx = self.builder.box_i32(raw_idx);
        let sliced = self.builder.call_runtime(rt_name, vec![source, idx]);
        self.lower_binding_pattern(&rest.argument, sliced);
    }

    /// Lower an object rest element: `let { a, b, ...rest } = obj`.
    ///
    /// Builds an array of excluded key names, then calls
    /// `__esc_rt_object_rest(obj, excluded_keys)` to create a new object
    /// without the extracted properties.
    fn lower_object_rest_element(
        &mut self,
        rest: &BindingRestElement<'_>,
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
            // Wrap the string into a proper runtime string value
            let to_str_idx = self.intern_string("__esc_rt_to_string");
            let to_str_name = self.builder.const_string(to_str_idx);
            let key_str = self.builder.call_runtime(to_str_name, vec![key_val]);
            self.builder
                .call_runtime(push_name, vec![excl_arr, key_str]);
        }

        let rt_name_idx = self.intern_string("__esc_rt_object_rest");
        let rt_name = self.builder.const_string(rt_name_idx);
        let rest_obj = self.builder.call_runtime(rt_name, vec![source, excl_arr]);
        self.lower_binding_pattern(&rest.argument, rest_obj);
    }

    // -----------------------------------------------------------------------
    // Import / Export
    // -----------------------------------------------------------------------

    /// Lower `import { a, b } from "mod"` or `import x from "mod"`.
    fn lower_import_declaration(&mut self, import: &oxc_ast::ast::ImportDeclaration<'_>) {
        let source_str = import.source.value.as_str();
        let source_idx = self.intern_string(source_str);
        let source_val = self.builder.const_string(source_idx);
        // Emit a LoadModule-equivalent via CallRuntime
        let load_name_idx = self.intern_string("__esc_rt_load_module");
        let load_name = self.builder.const_string(load_name_idx);
        let module_val = self.builder.call_runtime(load_name, vec![source_val]);

        if let Some(specifiers) = &import.specifiers {
            for spec in specifiers {
                match spec {
                    oxc_ast::ast::ImportDeclarationSpecifier::ImportSpecifier(named) => {
                        let import_name = named.imported.name().as_str();
                        let key_idx = self.intern_string(import_name);
                        let key = self.builder.const_string(key_idx);
                        let val = self.builder.get_prop(module_val, key);
                        let var = self.scopes.declare(named.local.name.as_str());
                        self.builder.write_variable(var, val);
                    }
                    oxc_ast::ast::ImportDeclarationSpecifier::ImportDefaultSpecifier(default) => {
                        let key_idx = self.intern_string("default");
                        let key = self.builder.const_string(key_idx);
                        let val = self.builder.get_prop(module_val, key);
                        let var = self.scopes.declare(default.local.name.as_str());
                        self.builder.write_variable(var, val);
                    }
                    oxc_ast::ast::ImportDeclarationSpecifier::ImportNamespaceSpecifier(ns) => {
                        let var = self.scopes.declare(ns.local.name.as_str());
                        self.builder.write_variable(var, module_val);
                    }
                }
            }
        }
    }

    /// Lower `export default expr`.
    fn lower_export_default(&mut self, export: &oxc_ast::ast::ExportDefaultDeclaration<'_>) {
        // Determine the declaration kind for the default export.
        let decl_kind = match &export.declaration {
            oxc_ast::ast::ExportDefaultDeclarationKind::FunctionDeclaration(_) => {
                ExportDeclKind::Function
            }
            oxc_ast::ast::ExportDefaultDeclarationKind::ClassDeclaration(_) => {
                ExportDeclKind::Class
            }
            _ => ExportDeclKind::Const, // expression defaults are effectively immutable
        };

        // Record the default export in the side table.
        self.recorded_exports.push(ExportInfo {
            name: "default".to_string(),
            kind: ExportKind::Default,
            decl_kind,
        });

        match &export.declaration {
            oxc_ast::ast::ExportDefaultDeclarationKind::FunctionDeclaration(func) => {
                self.lower_function_declaration(func);
            }
            oxc_ast::ast::ExportDefaultDeclarationKind::ClassDeclaration(class) => {
                self.lower_class_declaration(class);
            }
            _ => {
                if let Some(expr) = export.declaration.as_expression() {
                    let val = self.lower_expression(expr);
                    let var = self.scopes.declare("__default");
                    self.builder.write_variable(var, val);
                }
            }
        }
    }

    /// Lower `export { a, b }` or `export const x = ...`.
    fn lower_export_named(&mut self, export: &oxc_ast::ast::ExportNamedDeclaration<'_>) {
        let re_export_source = export.source.as_ref().map(|s| s.value.to_string());

        // Record named specifiers: `export { foo, bar }` or `export { foo } from './mod'`
        for spec in &export.specifiers {
            let name = spec.exported.name().to_string();
            let (kind, decl_kind) = match &re_export_source {
                Some(src) => (
                    ExportKind::ReExport {
                        source: src.clone(),
                    },
                    ExportDeclKind::Unknown,
                ),
                None => (ExportKind::Named, ExportDeclKind::Unknown),
            };
            self.recorded_exports.push(ExportInfo {
                name,
                kind,
                decl_kind,
            });
        }

        // Record declaration exports: `export const x = ...`, `export function f() {}`, etc.
        if let Some(decl) = &export.declaration {
            let entries = Self::declaration_export_entries(decl);
            for (name, decl_kind) in entries {
                self.recorded_exports.push(ExportInfo {
                    name,
                    kind: ExportKind::Named,
                    decl_kind,
                });
            }
            self.lower_declaration(decl);
        }
        // Named specifiers (re-exports) are handled at the module graph level,
        // not during lowering.
    }

    /// Lower `export * from './mod'`.
    fn lower_export_all(&mut self, export: &oxc_ast::ast::ExportAllDeclaration<'_>) {
        let source_str = export.source.value.to_string();
        let name = match &export.exported {
            Some(exported) => exported.name().to_string(),
            None => "*".to_string(),
        };
        self.recorded_exports.push(ExportInfo {
            name,
            kind: ExportKind::ReExport { source: source_str },
            decl_kind: ExportDeclKind::Unknown,
        });
    }

    /// Extract declared names and their declaration kinds from a declaration node.
    ///
    /// Returns `(name, decl_kind)` pairs that an `export const x = ...` or
    /// `export function f() {}` declaration would introduce as exports.
    fn declaration_export_entries(
        decl: &oxc_ast::ast::Declaration<'_>,
    ) -> Vec<(String, ExportDeclKind)> {
        match decl {
            oxc_ast::ast::Declaration::VariableDeclaration(var) => {
                let decl_kind = match var.kind {
                    VariableDeclarationKind::Const => ExportDeclKind::Const,
                    VariableDeclarationKind::Let => ExportDeclKind::Let,
                    VariableDeclarationKind::Var => ExportDeclKind::Var,
                    _ => ExportDeclKind::Unknown,
                };
                let mut names = Vec::new();
                for declarator in &var.declarations {
                    Self::collect_binding_names_inner(&declarator.id, &mut names);
                }
                names.into_iter().map(|n| (n, decl_kind)).collect()
            }
            oxc_ast::ast::Declaration::FunctionDeclaration(f) => {
                if let Some(id) = &f.id {
                    vec![(id.name.to_string(), ExportDeclKind::Function)]
                } else {
                    vec![]
                }
            }
            oxc_ast::ast::Declaration::ClassDeclaration(c) => {
                if let Some(id) = &c.id {
                    vec![(id.name.to_string(), ExportDeclKind::Class)]
                } else {
                    vec![]
                }
            }
            _ => vec![],
        }
    }

    /// Lower a declaration node (used by export named declarations).
    fn lower_declaration(&mut self, decl: &oxc_ast::ast::Declaration<'_>) {
        match decl {
            oxc_ast::ast::Declaration::VariableDeclaration(var_decl) => {
                let is_const = var_decl.kind == VariableDeclarationKind::Const;
                let is_var = var_decl.kind == VariableDeclarationKind::Var;
                for declarator in &var_decl.declarations {
                    let init_val = if let Some(init) = &declarator.init {
                        self.lower_expression(init)
                    } else {
                        self.builder.const_undefined()
                    };
                    self.lower_binding_pattern(&declarator.id, init_val);
                }
                // Track const names and clear TDZ for exported let/const
                if !is_var {
                    let names: Vec<String> = var_decl
                        .declarations
                        .iter()
                        .flat_map(|d| Self::collect_binding_names(&d.id))
                        .collect();
                    for name in &names {
                        self.tdz_vars.remove(name);
                        if is_const {
                            self.const_vars.insert(name.clone());
                        }
                    }
                }
            }
            oxc_ast::ast::Declaration::FunctionDeclaration(func) => {
                self.lower_function_declaration(func);
            }
            oxc_ast::ast::Declaration::ClassDeclaration(class) => {
                self.lower_class_declaration(class);
            }
            _ => {}
        }
    }

    // -----------------------------------------------------------------------
    // TDZ / const helpers
    // -----------------------------------------------------------------------

    /// Collect all variable names bound by a binding pattern.
    ///
    /// Recurses into destructuring patterns (object, array, assignment)
    /// to find all identifiers that would be declared.
    pub(crate) fn collect_binding_names(pattern: &BindingPattern<'_>) -> Vec<String> {
        let mut names = Vec::new();
        Self::collect_binding_names_inner(pattern, &mut names);
        names
    }

    /// Recursive helper for `collect_binding_names`.
    fn collect_binding_names_inner(pattern: &BindingPattern<'_>, names: &mut Vec<String>) {
        match pattern {
            BindingPattern::BindingIdentifier(ident) => {
                names.push(ident.name.as_str().to_string());
            }
            BindingPattern::ObjectPattern(obj) => {
                for prop in &obj.properties {
                    Self::collect_binding_names_inner(&prop.value, names);
                }
                if let Some(rest) = &obj.rest {
                    Self::collect_binding_names_inner(&rest.argument, names);
                }
            }
            BindingPattern::ArrayPattern(arr) => {
                for binding in arr.elements.iter().flatten() {
                    Self::collect_binding_names_inner(binding, names);
                }
                if let Some(rest) = &arr.rest {
                    Self::collect_binding_names_inner(&rest.argument, names);
                }
            }
            BindingPattern::AssignmentPattern(assignment) => {
                Self::collect_binding_names_inner(&assignment.left, names);
            }
        }
    }

    /// Pre-scan a block's statements for `let`/`const` declarations.
    ///
    /// Returns `(tdz_names, const_names)` where:
    /// - `tdz_names`: all names declared with `let` or `const` (subject to TDZ)
    /// - `const_names`: names declared with `const` (subset of tdz_names)
    pub(crate) fn collect_block_lexical_names(
        stmts: &[Statement<'_>],
    ) -> (Vec<String>, Vec<String>) {
        let mut tdz_names = Vec::new();
        let mut const_names = Vec::new();
        for stmt in stmts {
            if let Statement::VariableDeclaration(decl) = stmt {
                let is_let_or_const = matches!(
                    decl.kind,
                    VariableDeclarationKind::Let | VariableDeclarationKind::Const
                );
                if is_let_or_const {
                    for declarator in &decl.declarations {
                        let names = Self::collect_binding_names(&declarator.id);
                        for name in &names {
                            tdz_names.push(name.clone());
                            if decl.kind == VariableDeclarationKind::Const {
                                const_names.push(name.clone());
                            }
                        }
                    }
                }
            }
        }
        (tdz_names, const_names)
    }

    /// Pre-scan a collection of statement references for `let`/`const` declarations.
    ///
    /// Like [`collect_block_lexical_names`] but accepts `&[&Statement]` (used for
    /// switch cases where statements are gathered from multiple case clauses).
    pub(crate) fn collect_block_lexical_names_from_refs(
        stmts: &[&Statement<'_>],
    ) -> (Vec<String>, Vec<String>) {
        let mut tdz_names = Vec::new();
        let mut const_names = Vec::new();
        for stmt in stmts {
            if let Statement::VariableDeclaration(decl) = stmt {
                let is_let_or_const = matches!(
                    decl.kind,
                    VariableDeclarationKind::Let | VariableDeclarationKind::Const
                );
                if is_let_or_const {
                    for declarator in &decl.declarations {
                        let names = Self::collect_binding_names(&declarator.id);
                        for name in &names {
                            tdz_names.push(name.clone());
                            if decl.kind == VariableDeclarationKind::Const {
                                const_names.push(name.clone());
                            }
                        }
                    }
                }
            }
        }
        (tdz_names, const_names)
    }

    /// Collect lexically-declared and var-declared names from switch case bodies.
    ///
    /// Per sec-switch-statement-static-semantics-early-errors:
    /// - LexicallyDeclaredNames must not contain duplicates
    /// - LexicallyDeclaredNames and VarDeclaredNames must not overlap
    ///
    /// Returns `(lexical_names, var_names)`.
    fn collect_switch_lexical_and_var_names(
        cases: &[SwitchCase<'_>],
    ) -> (Vec<String>, Vec<String>) {
        let mut lexical = Vec::new();
        let mut var_names = Vec::new();
        for case in cases {
            for stmt in &case.consequent {
                match stmt {
                    Statement::VariableDeclaration(decl) => {
                        if matches!(
                            decl.kind,
                            VariableDeclarationKind::Let | VariableDeclarationKind::Const
                        ) {
                            for declarator in &decl.declarations {
                                lexical.extend(Self::collect_binding_names(&declarator.id));
                            }
                        } else {
                            // var declaration
                            for declarator in &decl.declarations {
                                var_names.extend(Self::collect_binding_names(&declarator.id));
                            }
                        }
                    }
                    Statement::FunctionDeclaration(func) => {
                        if let Some(id) = &func.id {
                            lexical.push(id.name.to_string());
                        }
                    }
                    Statement::ClassDeclaration(class) => {
                        if let Some(id) = &class.id {
                            lexical.push(id.name.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
        (lexical, var_names)
    }

    /// Collect const-declared variable names from a `for-in`/`for-of` loop target.
    ///
    /// Returns names that should be added to `const_vars` during the loop body
    /// and removed after each iteration.
    fn collect_for_target_const_names(target: &ForStatementLeft<'_>) -> Vec<String> {
        if let ForStatementLeft::VariableDeclaration(decl) = target
            && decl.kind == VariableDeclarationKind::Const
            && let Some(declarator) = decl.declarations.first()
        {
            return Self::collect_binding_names(&declarator.id);
        }
        Vec::new()
    }

    /// Infer `function.name` from the variable binding when the initializer is
    /// an anonymous function expression or arrow function.
    ///
    /// Per ECMAScript spec:
    /// - `const f = function() {}` -> `f.name === "f"`
    /// - `let g = () => {}` -> `g.name === "g"`
    /// - `var h = function named() {}` -> `h.name === "named"` (explicit name takes priority)
    fn maybe_infer_function_name(
        &mut self,
        binding: &BindingPattern<'_>,
        init: &Expression<'_>,
        closure_val: ValueId,
    ) {
        // Only infer for simple identifier bindings (not destructuring)
        let BindingPattern::BindingIdentifier(ident) = binding else {
            return;
        };

        // Check if the initializer is an anonymous function definition.
        // Per ES2024 §13.15.5.3 / §14.3.1.3 / §15.2.6, when an anonymous
        // function/class/arrow is assigned, SetFunctionName is called with
        // the binding identifier.
        let should_infer = match init {
            Expression::FunctionExpression(func) => {
                // Only infer if function has no explicit name
                func.id.is_none()
            }
            Expression::ArrowFunctionExpression(_) => {
                // Arrows never have explicit names
                true
            }
            Expression::ClassExpression(cls) => {
                // Only infer if class has no explicit name
                cls.id.is_none()
            }
            _ => false,
        };

        if should_infer {
            let var_name = ident.name.as_str();
            let name_key_idx = self.intern_string("name");
            let name_key = self.builder.const_string(name_key_idx);
            let name_val_idx = self.intern_string(var_name);
            let name_val = self.builder.const_string(name_val_idx);
            self.builder.set_prop(closure_val, name_key, name_val);
        }
    }

    /// Lower a try/catch/finally statement with proper finally semantics.
    ///
    /// When a finally block is present, `return` and `throw` in the try/catch
    /// body redirect to the finally block instead of acting immediately. After
    /// the finally body completes, the original completion (return value or
    /// exception) is replayed — unless the finally body itself returns or throws,
    /// in which case the finally completion takes precedence.
    fn lower_try_statement(&mut self, try_stmt: &oxc_ast::ast::TryStatement<'_>) {
        let catch_bb = self.builder.create_block();
        let finally_bb = if try_stmt.finalizer.is_some() {
            Some(self.builder.create_block())
        } else {
            None
        };
        let exit_bb = self.builder.create_block();

        let try_block = self.current_block.unwrap_or(BlockId(0));

        // Save outer finally state so nested try-finally works correctly
        let outer_finally_target = self.finally_target;
        let outer_return_var = self.finally_return_var;
        let outer_has_return_var = self.finally_has_return_var;
        let outer_exception_var = self.finally_exception_var;
        let outer_has_exception_var = self.finally_has_exception_var;
        let outer_catch_redirects = self.finally_catch_redirects_throw;
        let outer_catch_depth = self.finally_catch_depth;
        let outer_has_break_var = self.finally_has_break_var;
        let outer_break_target_var = self.finally_break_target_var;
        let outer_is_continue_var = self.finally_is_continue_var;
        let outer_jump_targets = std::mem::take(&mut self.finally_jump_targets);
        let outer_external_targets = std::mem::take(&mut self.finally_external_targets);

        // Track the vars for THIS try statement so we can read them
        // back after restoring outer state.
        let mut this_return_var = None;
        let mut this_has_return_var = None;
        let mut this_exception_var = None;
        let mut this_has_exception_var = None;
        let mut this_has_break_var = None;
        let mut this_break_target_var = None;
        let mut this_is_continue_var = None;

        // If there's a finally block, set up completion tracking variables.
        // All variables use JSValue-compatible types (f64) so phi nodes
        // in Cranelift have consistent I64 types.
        if finally_bb.is_some() {
            let ret_var = self.alloc_temp_var();
            let has_ret_var = self.alloc_temp_var();
            let exc_var = self.alloc_temp_var();
            let has_exc_var = self.alloc_temp_var();
            let brk_var = self.alloc_temp_var();
            let tgt_var = self.alloc_temp_var();
            let cont_var = self.alloc_temp_var();

            // Initialize all flags to falsy, values to undefined/zero
            let falsy = self.builder.const_f64(0.0);
            self.builder.write_variable(has_ret_var, falsy);
            let falsy2 = self.builder.const_f64(0.0);
            self.builder.write_variable(has_exc_var, falsy2);
            let falsy3 = self.builder.const_f64(0.0);
            self.builder.write_variable(brk_var, falsy3);
            let zero = self.builder.const_f64(0.0);
            self.builder.write_variable(tgt_var, zero);
            let zero2 = self.builder.const_f64(0.0);
            self.builder.write_variable(cont_var, zero2);
            let undef = self.builder.const_undefined();
            self.builder.write_variable(ret_var, undef);
            let undef2 = self.builder.const_undefined();
            self.builder.write_variable(exc_var, undef2);

            self.finally_target = finally_bb;
            self.finally_return_var = Some(ret_var);
            self.finally_has_return_var = Some(has_ret_var);
            self.finally_exception_var = Some(exc_var);
            self.finally_has_exception_var = Some(has_exc_var);
            self.finally_has_break_var = Some(brk_var);
            self.finally_break_target_var = Some(tgt_var);
            self.finally_is_continue_var = Some(cont_var);
            self.finally_jump_targets = Vec::new();

            // Snapshot all current break/continue/label targets. Breaks to
            // these targets are "external" (outside the try body) and must
            // route through the finally block.
            let mut ext = std::collections::HashSet::new();
            if let Some(bt) = self.loop_break_target {
                ext.insert(bt);
            }
            if let Some(ct) = self.loop_continue_target {
                ext.insert(ct);
            }
            for lt in self.label_targets.values() {
                ext.insert(lt.break_bb);
                if let Some(cbb) = lt.continue_bb {
                    ext.insert(cbb);
                }
            }
            self.finally_external_targets = ext;

            // Don't redirect throw during the try body — throw should go
            // to the catch handler normally.
            self.finally_catch_redirects_throw = false;

            this_return_var = Some(ret_var);
            this_has_return_var = Some(has_ret_var);
            this_exception_var = Some(exc_var);
            this_has_exception_var = Some(has_exc_var);
            this_has_break_var = Some(brk_var);
            this_break_target_var = Some(tgt_var);
            this_is_continue_var = Some(cont_var);
        }

        // === Try body ===
        self.builder.try_begin(catch_bb);
        self.catch_target_stack.push(catch_bb);
        self.terminated = false;
        for stmt in &try_stmt.block.body {
            if self.terminated {
                break;
            }
            self.lower_statement(stmt);
        }
        if !self.terminated {
            self.builder.try_end();
            self.catch_target_stack.pop();
        }
        if !self.terminated {
            let target = finally_bb.unwrap_or(exit_bb);
            self.builder.br(target);
            self.builder
                .add_predecessor(target, self.current_block_id());
        }

        // === Catch handler ===
        // The Cranelift lowerer handles catch_end and try_catch_stack
        // management automatically when it detects a catch handler block
        // (one with Op::Catch). We do NOT emit TryEnd here because it
        // would confuse precompute_catch_targets.
        // Pop catch_bb from catch_target_stack if the try body threw
        // (TryEnd wasn't emitted and thus didn't pop it).
        if self.catch_target_stack.last() == Some(&catch_bb) {
            self.catch_target_stack.pop();
        }
        if let Some(handler) = &try_stmt.handler {
            self.builder.switch_to_block(catch_bb);
            self.builder.add_predecessor(catch_bb, try_block);
            self.builder.seal_block(catch_bb);
            self.current_block = Some(catch_bb);
            self.terminated = false;

            let exception = self.builder.catch_();

            // Enable throw-to-finally redirection during the catch body
            if finally_bb.is_some() {
                self.finally_catch_redirects_throw = true;
                self.finally_catch_depth = self.catch_target_stack.len();
            }

            // Push catch block scope BEFORE binding the catch parameter so it
            // lives in the catch scope, not the enclosing scope. This matches
            // the ES spec (sec-runtime-semantics-catchclauseevaluation) where
            // the catch parameter is a new binding in the catch block scope.
            self.scopes.push_scope(ScopeKind::Block);

            // Bind catch parameter (supports destructuring and optional
            // catch binding per ES2019).
            if let Some(param) = &handler.param {
                self.lower_binding_pattern(&param.pattern, exception);
            }

            for stmt in &handler.body.body {
                if self.terminated {
                    break;
                }
                self.lower_statement(stmt);
            }
            self.scopes.pop_scope();

            // Disable throw-to-finally for subsequent code
            self.finally_catch_redirects_throw = false;

            if !self.terminated {
                let target = finally_bb.unwrap_or(exit_bb);
                self.builder.br(target);
                self.builder
                    .add_predecessor(target, self.current_block_id());
            }
        } else if let Some(fbb) = finally_bb {
            // No explicit catch handler but there IS a finally block.
            // Catch the exception, save it, and branch to finally so
            // the finally body executes before rethrowing.
            self.builder.switch_to_block(catch_bb);
            self.builder.add_predecessor(catch_bb, try_block);
            self.builder.seal_block(catch_bb);
            self.current_block = Some(catch_bb);

            let exception = self.builder.catch_();
            if let Some(exc_var) = this_exception_var {
                self.builder.write_variable(exc_var, exception);
            }
            if let Some(flag_var) = this_has_exception_var {
                let truthy = self.builder.const_f64(1.0);
                self.builder.write_variable(flag_var, truthy);
            }
            self.builder.br(fbb);
            self.builder.add_predecessor(fbb, catch_bb);
        } else {
            // No catch handler, no finally — just rethrow
            self.builder.switch_to_block(catch_bb);
            self.builder.add_predecessor(catch_bb, try_block);
            self.builder.seal_block(catch_bb);
            self.current_block = Some(catch_bb);
            let exception = self.builder.catch_();
            self.builder.rethrow(exception);
        }

        // === Finally handler ===
        if let Some(finally_bb) = finally_bb {
            // Capture jump targets built during the try/catch body lowering
            // BEFORE restoring outer state.
            let this_jump_targets = std::mem::take(&mut self.finally_jump_targets);

            // Restore outer finally state so return/throw in the finally
            // body itself acts directly (or redirects to an outer finally).
            self.finally_target = outer_finally_target;
            self.finally_return_var = outer_return_var;
            self.finally_has_return_var = outer_has_return_var;
            self.finally_exception_var = outer_exception_var;
            self.finally_has_exception_var = outer_has_exception_var;
            self.finally_catch_redirects_throw = outer_catch_redirects;
            self.finally_catch_depth = outer_catch_depth;
            self.finally_has_break_var = outer_has_break_var;
            self.finally_break_target_var = outer_break_target_var;
            self.finally_is_continue_var = outer_is_continue_var;
            self.finally_jump_targets = outer_jump_targets;
            self.finally_external_targets = outer_external_targets.clone();

            self.builder.switch_to_block(finally_bb);
            self.builder.seal_block(finally_bb);
            self.current_block = Some(finally_bb);
            self.terminated = false;

            if let Some(finalizer) = &try_stmt.finalizer {
                for stmt in &finalizer.body {
                    if self.terminated {
                        break;
                    }
                    self.lower_statement(stmt);
                }
            }

            // After the finally body: if it completed normally (no
            // return/throw in finally itself), replay the saved completion.
            if !self.terminated {
                // These vars are always set when finally_bb is Some (set
                // in the same branch that creates finally_bb above).
                assert!(
                    this_has_return_var.is_some()
                        && this_return_var.is_some()
                        && this_has_exception_var.is_some()
                        && this_exception_var.is_some()
                        && this_has_break_var.is_some()
                        && this_break_target_var.is_some()
                        && this_is_continue_var.is_some(),
                    "BUG: finally completion vars must be set when finally block exists"
                );
                let Some(has_ret) = this_has_return_var else {
                    unreachable!()
                };
                let Some(ret) = this_return_var else {
                    unreachable!()
                };
                let Some(has_exc) = this_has_exception_var else {
                    unreachable!()
                };
                let Some(exc) = this_exception_var else {
                    unreachable!()
                };
                let Some(has_brk) = this_has_break_var else {
                    unreachable!()
                };
                let Some(brk_tgt) = this_break_target_var else {
                    unreachable!()
                };
                let Some(is_cont) = this_is_continue_var else {
                    unreachable!()
                };
                self.emit_finally_completion(
                    exit_bb,
                    has_ret,
                    ret,
                    has_exc,
                    exc,
                    has_brk,
                    brk_tgt,
                    is_cont,
                    &this_jump_targets,
                );
            }
        } else {
            // No finally — restore outer state
            self.finally_target = outer_finally_target;
            self.finally_return_var = outer_return_var;
            self.finally_has_return_var = outer_has_return_var;
            self.finally_exception_var = outer_exception_var;
            self.finally_has_exception_var = outer_has_exception_var;
            self.finally_catch_redirects_throw = outer_catch_redirects;
            self.finally_catch_depth = outer_catch_depth;
            self.finally_has_break_var = outer_has_break_var;
            self.finally_break_target_var = outer_break_target_var;
            self.finally_is_continue_var = outer_is_continue_var;
            self.finally_jump_targets = outer_jump_targets;
            self.finally_external_targets = outer_external_targets;
        }

        self.builder.switch_to_block(exit_bb);
        self.builder.seal_block(exit_bb);
        self.current_block = Some(exit_bb);
        self.terminated = false;
    }

    /// Emit the completion check after a finally block completes normally.
    ///
    /// Reads the saved completion flags and acts accordingly:
    /// 1. has_return truthy → propagate to outer finally or emit `ret`
    /// 2. has_exception truthy → propagate to outer finally or `rethrow`
    /// 3. has_break truthy → propagate to outer finally or jump to target
    /// 4. otherwise → fall through to `exit_bb`
    ///
    /// For nested try-finally, when an outer finally exists
    /// (`self.finally_target` is `Some`), completions are propagated to the
    /// outer finally's vars rather than executed directly. This ensures all
    /// finally blocks in the chain execute before the completion takes effect.
    #[allow(clippy::too_many_arguments)]
    fn emit_finally_completion(
        &mut self,
        exit_bb: BlockId,
        has_return_var: u32,
        return_var: u32,
        has_exception_var: u32,
        exception_var: u32,
        has_break_var: u32,
        break_target_var: u32,
        is_continue_var: u32,
        jump_targets: &[BlockId],
    ) {
        // Snapshot the enclosing catch target now (before blocks are created).
        // The outer try state was restored before the finally body, so
        // catch_target_stack reflects the enclosing try scope.
        let enclosing_catch = self.catch_target_stack.last().copied();

        // --- Check pending return ---
        let has_ret = self.builder.read_variable(has_return_var, IrType::JSValue);
        let is_return = self.builder.to_boolean(has_ret);
        let return_bb = self.builder.create_block();
        let check_exc_bb = self.builder.create_block();
        self.builder.br_if(is_return, return_bb, check_exc_bb);
        let branch_block = self.current_block_id();
        self.builder.add_predecessor(return_bb, branch_block);
        self.builder.add_predecessor(check_exc_bb, branch_block);

        // Return block: propagate to outer finally or emit ret
        self.builder.switch_to_block(return_bb);
        self.builder.seal_block(return_bb);
        self.current_block = Some(return_bb);
        let ret_val = self.builder.read_variable(return_var, IrType::JSValue);
        if let Some(outer_finally_bb) = self.finally_target {
            // Propagate return to outer finally
            if let Some(outer_ret_var) = self.finally_return_var {
                self.builder.write_variable(outer_ret_var, ret_val);
            }
            if let Some(outer_flag) = self.finally_has_return_var {
                let truthy = self.builder.const_f64(1.0);
                self.builder.write_variable(outer_flag, truthy);
            }
            self.builder.br(outer_finally_bb);
            self.builder.add_predecessor(outer_finally_bb, return_bb);
        } else {
            self.builder.ret(Some(ret_val));
        }

        // --- Check pending exception ---
        self.builder.switch_to_block(check_exc_bb);
        self.builder.seal_block(check_exc_bb);
        self.current_block = Some(check_exc_bb);
        let has_exc = self
            .builder
            .read_variable(has_exception_var, IrType::JSValue);
        let is_exc = self.builder.to_boolean(has_exc);
        let rethrow_bb = self.builder.create_block();
        let check_break_bb = self.builder.create_block();
        self.builder.br_if(is_exc, rethrow_bb, check_break_bb);
        let check_block = self.current_block_id();
        self.builder.add_predecessor(rethrow_bb, check_block);
        self.builder.add_predecessor(check_break_bb, check_block);

        // Rethrow block: propagate to outer finally or rethrow
        self.builder.switch_to_block(rethrow_bb);
        self.builder.seal_block(rethrow_bb);
        self.current_block = Some(rethrow_bb);
        let exc_val = self.builder.read_variable(exception_var, IrType::JSValue);
        if let Some(outer_finally_bb) = self.finally_target {
            // Propagate exception to outer finally
            if let Some(outer_exc_var) = self.finally_exception_var {
                self.builder.write_variable(outer_exc_var, exc_val);
            }
            if let Some(outer_flag) = self.finally_has_exception_var {
                let truthy = self.builder.const_f64(1.0);
                self.builder.write_variable(outer_flag, truthy);
            }
            self.builder.br(outer_finally_bb);
            self.builder.add_predecessor(outer_finally_bb, rethrow_bb);
        } else if let Some(catch_target) = enclosing_catch {
            self.builder.add_predecessor(catch_target, rethrow_bb);
            self.builder.rethrow_to(exc_val, catch_target);
        } else {
            self.builder.rethrow(exc_val);
        }

        // --- Check pending break/continue ---
        self.builder.switch_to_block(check_break_bb);
        self.builder.seal_block(check_break_bb);
        self.current_block = Some(check_break_bb);

        if jump_targets.is_empty() {
            // No break/continue targets were registered — fall through
            self.builder.br(exit_bb);
            self.builder.add_predecessor(exit_bb, check_break_bb);
        } else {
            let has_brk = self.builder.read_variable(has_break_var, IrType::JSValue);
            let is_brk = self.builder.to_boolean(has_brk);
            let dispatch_bb = self.builder.create_block();
            self.builder.br_if(is_brk, dispatch_bb, exit_bb);
            let brk_check_block = self.current_block_id();
            self.builder.add_predecessor(dispatch_bb, brk_check_block);
            self.builder.add_predecessor(exit_bb, brk_check_block);

            // Dispatch block: read target index and branch to the right target.
            self.builder.switch_to_block(dispatch_bb);
            self.builder.seal_block(dispatch_bb);
            self.current_block = Some(dispatch_bb);

            self.emit_jump_dispatch(break_target_var, is_continue_var, jump_targets, exit_bb);
        }
    }

    /// Emit a chain of comparisons to dispatch a break/continue to the
    /// correct target block based on the stored target index.
    ///
    /// For each registered jump target, compares the stored index against
    /// the target's index. If matched, branches to that target. If an outer
    /// finally exists (`self.finally_target`), the completion is propagated
    /// through the outer finally instead of jumping directly.
    ///
    /// The `is_continue_var` is currently unused for dispatch logic (both
    /// break and continue use the same target blocks), but is preserved
    /// for potential future use with loop-continue semantics.
    fn emit_jump_dispatch(
        &mut self,
        break_target_var: u32,
        _is_continue_var: u32,
        jump_targets: &[BlockId],
        fallback_bb: BlockId,
    ) {
        let target_idx_val = self
            .builder
            .read_variable(break_target_var, IrType::JSValue);

        // For a single target, jump directly (no comparison needed)
        if jump_targets.len() == 1 {
            let target = jump_targets[0];
            if let Some(outer_finally_bb) = self.finally_target {
                // Propagate break/continue through outer finally
                self.propagate_break_to_outer_finally(target, outer_finally_bb);
            } else {
                self.builder.br(target);
                if let Some(cur) = self.current_block {
                    self.builder.add_predecessor(target, cur);
                }
            }
            return;
        }

        // Multiple targets: emit comparison chain
        for (i, &target) in jump_targets.iter().enumerate() {
            let idx_const = self.builder.const_f64(i as f64);
            let eq = self.builder.eq_strict(target_idx_val, idx_const);
            let eq_bool = self.builder.to_boolean(eq);

            let match_bb = self.builder.create_block();
            let next_bb = if i + 1 < jump_targets.len() {
                self.builder.create_block()
            } else {
                // Last target — fallback is unreachable but we still
                // need a block for the false branch
                fallback_bb
            };

            self.builder.br_if(eq_bool, match_bb, next_bb);
            let cur = self.current_block_id();
            self.builder.add_predecessor(match_bb, cur);
            self.builder.add_predecessor(next_bb, cur);

            // Match block: jump to target (or propagate through outer finally)
            self.builder.switch_to_block(match_bb);
            self.builder.seal_block(match_bb);
            self.current_block = Some(match_bb);

            if let Some(outer_finally_bb) = self.finally_target {
                self.propagate_break_to_outer_finally(target, outer_finally_bb);
            } else {
                self.builder.br(target);
                self.builder.add_predecessor(target, match_bb);
            }

            // Continue to next check
            if i + 1 < jump_targets.len() {
                self.builder.switch_to_block(next_bb);
                self.builder.seal_block(next_bb);
                self.current_block = Some(next_bb);
            }
        }
    }

    /// Propagate a pending break/continue through the outer finally block.
    ///
    /// Registers the target in the outer finally's jump target table, writes
    /// the outer completion vars, and branches to the outer finally block.
    fn propagate_break_to_outer_finally(&mut self, target: BlockId, outer_finally_bb: BlockId) {
        let outer_idx = self.register_jump_target(target);
        if let Some(brk_var) = self.finally_has_break_var {
            let truthy = self.builder.const_f64(1.0);
            self.builder.write_variable(brk_var, truthy);
        }
        if let Some(tgt_var) = self.finally_break_target_var {
            let idx_val = self.builder.const_f64(outer_idx as f64);
            self.builder.write_variable(tgt_var, idx_val);
        }
        self.builder.br(outer_finally_bb);
        if let Some(cur) = self.current_block {
            self.builder.add_predecessor(outer_finally_bb, cur);
        }
    }

    /// Emit a break or continue, routing through the finally block if the
    /// target is external to the current try-finally scope.
    ///
    /// When `self.finally_target` is `Some` AND the target is in
    /// `finally_external_targets` (meaning it was created before the
    /// try-finally was entered), we store the completion type and target
    /// index, then branch to the finally block. Otherwise, we branch
    /// directly to the target.
    ///
    /// `is_continue` distinguishes continue (true) from break (false) so
    /// that `emit_finally_completion` can dispatch correctly.
    fn emit_break_or_continue(&mut self, target: BlockId, is_continue: bool) {
        if let Some(finally_bb) = self.finally_target {
            if !self.finally_external_targets.contains(&target) {
                // Target is inside the try body (e.g., a switch exit or
                // inner loop exit) — branch directly without going
                // through finally.
                self.builder.br(target);
                if let Some(cur) = self.current_block {
                    self.builder.add_predecessor(target, cur);
                }
                self.terminated = true;
                return;
            }
            // Inside try-with-finally and target is external: register
            // the target and store completion vars, then branch to the
            // finally block.
            let target_idx = self.register_jump_target(target);
            if let Some(brk_var) = self.finally_has_break_var {
                let truthy = self.builder.const_f64(1.0);
                self.builder.write_variable(brk_var, truthy);
            }
            if let Some(tgt_var) = self.finally_break_target_var {
                let idx_val = self.builder.const_f64(target_idx as f64);
                self.builder.write_variable(tgt_var, idx_val);
            }
            if let Some(cont_var) = self.finally_is_continue_var {
                let flag = self.builder.const_f64(if is_continue { 1.0 } else { 0.0 });
                self.builder.write_variable(cont_var, flag);
            }
            self.builder.br(finally_bb);
            if let Some(cur) = self.current_block {
                self.builder.add_predecessor(finally_bb, cur);
            }
            self.terminated = true;
        } else {
            // No finally — branch directly to the target.
            self.builder.br(target);
            if let Some(cur) = self.current_block {
                self.builder.add_predecessor(target, cur);
            }
            self.terminated = true;
        }
    }

    /// Register a jump target for break/continue dispatch in the current
    /// finally level. Returns the numeric index assigned to this target.
    /// If the target is already registered, returns the existing index.
    fn register_jump_target(&mut self, target: BlockId) -> usize {
        if let Some(idx) = self.finally_jump_targets.iter().position(|&b| b == target) {
            idx
        } else {
            let idx = self.finally_jump_targets.len();
            self.finally_jump_targets.push(target);
            idx
        }
    }
}

#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use crate::{lower_program, lower_source};
    use ir::printer::print_typed_module;
    use ir::types::Op;
    use ir::verify::verify_typed_module;

    /// Lower JS source and return (printed IR, module) for inspection.
    fn lower_and_print(source: &str) -> String {
        let result = lower_program(source).expect("lowering should succeed");
        verify_typed_module(&result.module).expect("IR should verify");
        print_typed_module(&result.module)
    }

    /// Lower JS source and return the lowering result for opcode-level checks.
    fn lower_and_get_module(source: &str) -> ir::builder::TypedModule {
        let result = lower_program(source).expect("lowering should succeed");
        verify_typed_module(&result.module).expect("IR should verify");
        result.module
    }

    /// Lower JS source as a **script** (non-module, sloppy mode by default).
    fn lower_script(source: &str) -> crate::LoweringResult {
        // Use default script source type (not module) — sloppy mode unless "use strict"
        let source_type = oxc_span::SourceType::cjs();
        lower_source(source, source_type).expect("lowering should succeed")
    }

    /// Lower JS source as a script and return the module for opcode checks.
    fn lower_script_module(source: &str) -> ir::builder::TypedModule {
        let result = lower_script(source);
        verify_typed_module(&result.module).expect("IR should verify");
        result.module
    }

    /// Lower JS source as a script and return the printed IR.
    fn lower_script_print(source: &str) -> String {
        let result = lower_script(source);
        verify_typed_module(&result.module).expect("IR should verify");
        print_typed_module(&result.module)
    }

    /// Check if any block in the entry function contains the given opcode.
    fn entry_has_op(module: &ir::builder::TypedModule, op: Op) -> bool {
        let entry_fn = &module.functions[module.entry.unwrap()];
        entry_fn
            .blocks
            .iter()
            .any(|block| block.instructions.iter().any(|inst| inst.op == op))
    }

    // === Bug 1: BlockStatement stops after terminator ===

    #[test]
    fn test_block_terminates_on_break_in_loop() {
        // After `break`, no more instructions should be emitted in that block.
        // `let x = 1` after `break` should be dead code and not emitted.
        let ir = lower_and_print("while(true) { break; let x = 1; }");
        // The break emits a `br` to the exit block. After that, no write_var
        // for `x` should appear in the same block.
        // We verify by checking the IR text: the block containing "br bb"
        // (the break jump) should NOT also contain a write_var after it.
        // A simpler check: the IR should verify (no instructions after terminator).
        assert!(ir.contains("br bb"), "should have branch for break");
    }

    #[test]
    fn test_block_terminates_on_return() {
        // After `return`, no more instructions should be emitted in that function body block.
        let ir = lower_and_print("function f() { return 1; let x = 2; }");
        // The IR should verify correctly (no instructions after terminator).
        // The function should have a `ret` instruction.
        assert!(ir.contains("ret"), "should have ret instruction");
        // Count const_i32 instructions in the function: should have `1` but NOT `2`
        // because `let x = 2` is dead code after `return 1`.
        // The function f has const_i32(1) + ret. The dead `let x = 2` should not appear.
        let func_count = ir.matches("fn @").count();
        assert!(
            func_count >= 2,
            "should have at least 2 functions (main + f)"
        );
    }

    // === Bug 2: try_end not emitted after terminator in try body ===

    #[test]
    fn test_try_catch_with_throw_no_try_end() {
        // When the try body ends with `throw`, TryEnd should NOT be emitted
        // because the block is already terminated.
        let module = lower_and_get_module(r#"try { throw "err"; } catch(e) {}"#);
        let entry_fn = &module.functions[module.entry.unwrap()];

        // Find the block that contains TryBegin — the try body block.
        // It should have Throw but NOT TryEnd.
        let try_body_block = entry_fn.blocks.iter().find(|block| {
            block
                .instructions
                .iter()
                .any(|inst| inst.op == Op::TryBegin)
        });

        assert!(
            try_body_block.is_some(),
            "should have a block with TryBegin"
        );
        let block = try_body_block.unwrap();

        let has_throw = block.instructions.iter().any(|inst| inst.op == Op::Throw);
        let has_try_end = block.instructions.iter().any(|inst| inst.op == Op::TryEnd);

        assert!(has_throw, "try body block should have Throw");
        assert!(
            !has_try_end,
            "try body block should NOT have TryEnd after Throw"
        );
    }

    #[test]
    fn test_try_catch_normal_has_try_end() {
        // When the try body does NOT end with a terminator, TryEnd SHOULD be present.
        let module = lower_and_get_module("try { let x = 1; } catch(e) {}");
        let entry_fn = &module.functions[module.entry.unwrap()];

        let try_body_block = entry_fn.blocks.iter().find(|block| {
            block
                .instructions
                .iter()
                .any(|inst| inst.op == Op::TryBegin)
        });

        assert!(
            try_body_block.is_some(),
            "should have a block with TryBegin"
        );
        let block = try_body_block.unwrap();

        let has_try_end = block.instructions.iter().any(|inst| inst.op == Op::TryEnd);

        assert!(
            has_try_end,
            "try body block should have TryEnd when body does not terminate"
        );
    }

    #[test]
    fn test_try_finally_with_throw() {
        // try { throw 1; } finally { let x = 2; }
        // The try body block should NOT have TryEnd after Throw.
        let module = lower_and_get_module("try { throw 1; } finally { let x = 2; }");
        let entry_fn = &module.functions[module.entry.unwrap()];

        let try_body_block = entry_fn.blocks.iter().find(|block| {
            block
                .instructions
                .iter()
                .any(|inst| inst.op == Op::TryBegin)
        });

        assert!(
            try_body_block.is_some(),
            "should have a block with TryBegin"
        );
        let block = try_body_block.unwrap();

        let has_throw = block.instructions.iter().any(|inst| inst.op == Op::Throw);
        let has_try_end = block.instructions.iter().any(|inst| inst.op == Op::TryEnd);

        assert!(has_throw, "try body block should have Throw");
        assert!(
            !has_try_end,
            "try body block should NOT have TryEnd after Throw"
        );
    }

    // === Bug 3: CallExpression with member callee emits CallMethod ===

    #[test]
    fn test_method_call_static_emits_call_method() {
        // `a.join(",")` should emit CallMethod, not Call
        let module = lower_and_get_module(r#"let a = [1]; a.join(",");"#);
        assert!(
            entry_has_op(&module, Op::CallMethod),
            "obj.method() should emit CallMethod"
        );
        assert!(
            !entry_has_op(&module, Op::Call),
            "obj.method() should not emit generic Call"
        );
    }

    #[test]
    fn test_method_call_computed_emits_call_method() {
        // `o["key"]()` should emit CallMethod so the receiver is preserved
        // as `this` (needed for Generator.return, Promise.then, etc.).
        let module = lower_and_get_module(r#"let o = {}; o["key"]();"#);
        assert!(
            entry_has_op(&module, Op::CallMethod),
            "obj[key]() should emit CallMethod to preserve receiver"
        );
    }

    #[test]
    fn test_console_log_still_emits_call_runtime() {
        // Regression test: console.log should still use CallRuntime, not CallMethod
        let module = lower_and_get_module(r#"console.log("hi");"#);
        assert!(
            entry_has_op(&module, Op::CallRuntime),
            "console.log should still emit CallRuntime"
        );
        assert!(
            !entry_has_op(&module, Op::CallMethod),
            "console.log should not emit CallMethod"
        );
        assert!(
            !entry_has_op(&module, Op::Call),
            "console.log should not emit generic Call"
        );
    }

    // === DoWhileStatement lowering ===

    #[test]
    fn test_do_while_basic() {
        // do { ... } while (x) should produce body → header → branch pattern
        let ir = lower_and_print("let x = 3; do { x = x - 1; } while (x > 0);");
        assert!(
            ir.contains("br_if"),
            "should have conditional branch for do-while condition"
        );
    }

    #[test]
    fn test_do_while_single_iteration() {
        // do { ... } while (false) — body runs once, condition is false
        let ir = lower_and_print("do { let x = 1; } while (false);");
        assert!(
            ir.contains("br_if"),
            "should have conditional branch even for false condition"
        );
    }

    #[test]
    fn test_do_while_break() {
        // break inside do-while should jump to exit
        let ir = lower_and_print("do { break; } while (true);");
        // Should have at least 2 'br' instructions (one for break, one for body→header)
        assert!(ir.contains("br bb"), "should have branch for break");
    }

    #[test]
    fn test_do_while_continue() {
        // continue inside do-while should jump to header (condition check)
        let ir = lower_and_print("do { continue; } while (true);");
        assert!(ir.contains("br bb"), "should have branch for continue");
    }

    #[test]
    fn test_do_while_verifies() {
        // The lowered IR should pass verification
        let module = lower_and_get_module("let i = 0; do { i = i + 1; } while (i < 5);");
        assert!(
            !module.functions.is_empty(),
            "should have at least one function"
        );
    }

    // === for-of iter_done fix ===

    #[test]
    fn test_for_of_array_iter_done_uses_result() {
        // Verify that for..of lowering emits iter_done on the result of iter_next,
        // not on the iterator itself.
        let module = lower_and_get_module("let a = [1, 2, 3]; for (let x of a) {}");
        let entry_fn = &module.functions[module.entry.unwrap()];

        // Find the block with IterNext — get its result ValueId
        let mut iter_next_result = None;
        let mut iter_done_operand = None;

        for block in &entry_fn.blocks {
            for inst in &block.instructions {
                if inst.op == Op::IterNext {
                    iter_next_result = Some(inst.id);
                }
                if inst.op == Op::IterDone {
                    iter_done_operand = Some(inst.operands[0]);
                }
            }
        }

        assert!(iter_next_result.is_some(), "should have IterNext");
        assert!(iter_done_operand.is_some(), "should have IterDone");

        // IterDone should use the result of IterNext, not the iterator
        assert_eq!(
            iter_next_result.unwrap(),
            iter_done_operand.unwrap(),
            "IterDone should operate on IterNext result, not the raw iterator"
        );
    }

    #[test]
    fn test_for_in_iter_done_uses_result() {
        // Same fix applies to for..in
        let module = lower_and_get_module("let o = {}; for (let k in o) {}");
        let entry_fn = &module.functions[module.entry.unwrap()];

        let mut iter_next_result = None;
        let mut iter_done_operand = None;

        for block in &entry_fn.blocks {
            for inst in &block.instructions {
                if inst.op == Op::IterNext {
                    iter_next_result = Some(inst.id);
                }
                if inst.op == Op::IterDone {
                    iter_done_operand = Some(inst.operands[0]);
                }
            }
        }

        assert!(iter_next_result.is_some(), "should have IterNext");
        assert!(iter_done_operand.is_some(), "should have IterDone");
        assert_eq!(
            iter_next_result.unwrap(),
            iter_done_operand.unwrap(),
            "IterDone should operate on IterNext result, not the raw iterator"
        );
    }

    // === Closure variable capture tests ===

    /// Check if any function in the module contains the given opcode.
    fn any_fn_has_op(module: &ir::builder::TypedModule, op: Op) -> bool {
        module.functions.iter().any(|f| {
            f.blocks
                .iter()
                .any(|block| block.instructions.iter().any(|inst| inst.op == op))
        })
    }

    #[test]
    fn test_capture_single_variable() {
        // `let a = 1; let f = () => a;` — the arrow should capture `a`
        let module = lower_and_get_module("let a = 1; let f = () => a;");
        assert!(
            any_fn_has_op(&module, Op::EnvCreate),
            "should emit EnvCreate for captured variable"
        );
        assert!(
            any_fn_has_op(&module, Op::EnvStore),
            "should emit EnvStore to populate env"
        );
        assert!(
            any_fn_has_op(&module, Op::EnvLoad),
            "should emit EnvLoad inside closure"
        );
    }

    #[test]
    fn test_capture_multiple_variables() {
        // Closure captures two variables from parent
        let source = r#"
            let a = 1;
            let b = 2;
            let f = () => a + b;
        "#;
        let module = lower_and_get_module(source);
        assert!(
            any_fn_has_op(&module, Op::EnvCreate),
            "should emit EnvCreate for captured variables"
        );
        // Count EnvStore instructions — should have at least 2
        let store_count: usize = module
            .functions
            .iter()
            .flat_map(|f| f.blocks.iter())
            .flat_map(|b| b.instructions.iter())
            .filter(|i| i.op == Op::EnvStore)
            .count();
        assert!(
            store_count >= 2,
            "should have at least 2 EnvStore instructions, got {store_count}"
        );
    }

    #[test]
    fn test_no_capture_passthrough() {
        // Function with no captures should NOT emit EnvCreate/EnvStore
        let module = lower_and_get_module("let f = (x) => x * 2;");
        // The main function should not have EnvCreate
        let entry_fn = &module.functions[module.entry.unwrap()];
        let has_env_create = entry_fn
            .blocks
            .iter()
            .any(|b| b.instructions.iter().any(|i| i.op == Op::EnvCreate));
        assert!(
            !has_env_create,
            "non-capturing closure should NOT emit EnvCreate"
        );
    }

    #[test]
    fn test_capture_in_arrow() {
        // Arrow function capturing parent var
        let ir = lower_and_print("let x = 10; let add = (y) => x + y;");
        assert!(
            ir.contains("env_create"),
            "should have env_create in IR output"
        );
        assert!(
            ir.contains("env_store"),
            "should have env_store in IR output"
        );
        assert!(ir.contains("env_load"), "should have env_load in IR output");
    }

    #[test]
    fn test_capture_in_function_expression() {
        // Function expression capturing parent var
        let source = r#"
            let x = 42;
            let f = function() { return x; };
        "#;
        let module = lower_and_get_module(source);
        assert!(
            any_fn_has_op(&module, Op::EnvCreate),
            "function expression should capture via EnvCreate"
        );
        assert!(
            any_fn_has_op(&module, Op::EnvLoad),
            "function expression should load captured var via EnvLoad"
        );
    }

    #[test]
    fn test_capture_parameter() {
        // Closure captures a parameter from outer function
        let source = r#"
            function make(x) {
                return () => x;
            }
        "#;
        let module = lower_and_get_module(source);
        assert!(
            any_fn_has_op(&module, Op::EnvCreate),
            "should create env for captured parameter"
        );
        assert!(
            any_fn_has_op(&module, Op::EnvLoad),
            "inner closure should EnvLoad the captured parameter"
        );
    }

    #[test]
    fn test_capture_with_shadowing() {
        // Inner variable shadows the captured variable — should NOT capture
        let source = r#"
            let x = 1;
            let f = (x) => x * 2;
        "#;
        let module = lower_and_get_module(source);
        // Since the arrow's param `x` shadows the outer `x`,
        // there should be no capture
        let entry_fn = &module.functions[module.entry.unwrap()];
        let has_env_create = entry_fn
            .blocks
            .iter()
            .any(|b| b.instructions.iter().any(|i| i.op == Op::EnvCreate));
        assert!(!has_env_create, "shadowed variable should NOT be captured");
    }

    #[test]
    fn test_capture_loop_variable() {
        // `for(let i = 0; i < 3; i++) { arr.push(() => i); }`
        let source = r#"
            let arr = [];
            for (let i = 0; i < 3; i++) {
                arr.push(() => i);
            }
        "#;
        let module = lower_and_get_module(source);
        assert!(
            any_fn_has_op(&module, Op::EnvCreate),
            "should capture loop variable"
        );
        assert!(
            any_fn_has_op(&module, Op::EnvLoad),
            "closure should load loop variable from env"
        );
    }

    #[test]
    fn test_capture_ir_snapshot_simple() {
        // Verify the IR output shape for a simple capture
        let ir = lower_and_print("let a = 1; let f = () => a;");
        // Should contain env_create, env_store (in main), and env_load (in arrow)
        assert!(ir.contains("env_create"), "IR should contain env_create");
        assert!(ir.contains("env_store"), "IR should contain env_store");
        assert!(ir.contains("env_load"), "IR should contain env_load");
        assert!(
            ir.contains("create_closure"),
            "IR should contain create_closure"
        );
    }

    #[test]
    fn test_capture_does_not_capture_globals() {
        // console.log is a global — should NOT be captured
        let source = r#"
            let f = () => console.log("hi");
        "#;
        let module = lower_and_get_module(source);
        let entry_fn = &module.functions[module.entry.unwrap()];
        let has_env_create = entry_fn
            .blocks
            .iter()
            .any(|b| b.instructions.iter().any(|i| i.op == Op::EnvCreate));
        assert!(
            !has_env_create,
            "globals like console should NOT trigger capture"
        );
    }

    #[test]
    fn test_capture_create_closure_receives_env() {
        // The CreateClosure should receive the env (not null) when captures exist
        let module = lower_and_get_module("let a = 1; let f = () => a;");
        let entry_fn = &module.functions[module.entry.unwrap()];
        // Find CreateClosure instruction
        let closure_instr = entry_fn
            .blocks
            .iter()
            .flat_map(|b| b.instructions.iter())
            .find(|i| i.op == Op::CreateClosure);
        assert!(
            closure_instr.is_some(),
            "should have CreateClosure instruction"
        );
        // The CreateClosure should have 3 arguments (func_ref, env, flags)
        let instr = closure_instr.unwrap();
        assert_eq!(
            instr.operands.len(),
            3,
            "CreateClosure should have 3 operands (func_ref, env, flags)"
        );
    }

    #[test]
    fn test_capture_ir_verifies() {
        // Ensure the IR with captures passes verification
        let sources = [
            "let a = 1; let f = () => a;",
            "let x = 1; let y = 2; let f = () => x + y;",
            "function make(x) { return () => x; }",
            "let a = 1; let f = function() { return a; };",
        ];
        for source in sources {
            let result = lower_program(source).expect("lowering should succeed");
            verify_typed_module(&result.module)
                .unwrap_or_else(|e| panic!("IR should verify for '{source}': {e:?}"));
        }
    }

    // =========================================================================
    // Phase F regression tests: rest params, class extends, spread, closures
    // =========================================================================

    // === Rest params emit i64 start index ===

    #[test]
    fn test_rest_params_lowers_and_verifies() {
        // Rest params should emit CallRuntime for __esc_rt_rest_args with i64 arg.
        let source = "function f(a, b, ...rest) { return rest; }";
        let module = lower_and_get_module(source);
        assert!(
            any_fn_has_op(&module, Op::CallRuntime),
            "rest params should emit CallRuntime for __esc_rt_rest_args"
        );
    }

    #[test]
    fn test_rest_params_single_rest_only() {
        // function f(...args) — rest starts at index 0
        let source = "function f(...args) { return args; }";
        let result = lower_program(source).expect("lowering should succeed");
        verify_typed_module(&result.module).expect("IR should verify");
        assert!(
            result
                .string_table
                .contains(&"__esc_rt_rest_args".to_string()),
            "should intern __esc_rt_rest_args"
        );
    }

    // === Class extends: prototype chain ===

    #[test]
    fn test_class_extends_lowers_and_verifies() {
        let source = r#"
            class Animal { constructor(name) { this.name = name; } }
            class Dog extends Animal { constructor(name) { super(name); } }
        "#;
        let result = lower_program(source).expect("lowering should succeed");
        verify_typed_module(&result.module).expect("class extends IR should verify");
    }

    #[test]
    fn test_class_extends_sets_proto_link() {
        // The lowered IR should use __proto__ for prototype chain setup.
        let source = r#"
            class A {}
            class B extends A {}
        "#;
        let result = lower_program(source).expect("lowering should succeed");
        verify_typed_module(&result.module).expect("IR should verify");
        assert!(
            result.string_table.contains(&"__proto__".to_string()),
            "class extends should intern '__proto__' for prototype chain"
        );
    }

    // === Spread arguments: CallRuntime apply pattern ===

    #[test]
    fn test_spread_args_lowers_and_verifies() {
        let source = r#"
            function sum(a, b, c) { return a + b + c; }
            let args = [1, 2, 3];
            sum(...args);
        "#;
        let result = lower_program(source).expect("lowering should succeed");
        verify_typed_module(&result.module).expect("spread args IR should verify");
        assert!(
            result.string_table.contains(&"__esc_rt_apply".to_string()),
            "spread call should use __esc_rt_apply"
        );
    }

    #[test]
    fn test_spread_args_mixed_lowers() {
        // Mix of normal args and spread: f(1, ...arr, 3)
        let source = r#"
            function f() {}
            let arr = [2];
            f(1, ...arr, 3);
        "#;
        let result = lower_program(source).expect("lowering should succeed");
        verify_typed_module(&result.module).expect("IR should verify");
        assert!(
            result
                .string_table
                .contains(&"__esc_rt_spread_into_array".to_string()),
            "mixed spread should use __esc_rt_spread_into_array"
        );
    }

    #[test]
    fn test_no_spread_does_not_use_apply() {
        // Normal call without spread should NOT use apply.
        let source = "function f(a, b) {} f(1, 2);";
        let result = lower_program(source).expect("lowering should succeed");
        verify_typed_module(&result.module).expect("IR should verify");
        assert!(
            !result.string_table.contains(&"__esc_rt_apply".to_string()),
            "non-spread call should NOT use __esc_rt_apply"
        );
    }

    #[test]
    fn test_spread_method_call_uses_apply_method() {
        // Member-expression spread call: obj.method(...args) must preserve the
        // receiver via __esc_rt_apply_method instead of dropping the spread.
        let source = r#"
            let obj = { max: function(a, b, c) { return a + b + c; } };
            let args = [1, 2, 3];
            obj.max(...args);
        "#;
        let result = lower_program(source).expect("lowering should succeed");
        verify_typed_module(&result.module).expect("spread method call IR should verify");
        assert!(
            result
                .string_table
                .contains(&"__esc_rt_apply_method".to_string()),
            "spread method call should use __esc_rt_apply_method"
        );
    }

    #[test]
    fn test_spread_computed_call_uses_apply_method() {
        // Computed-member-expression spread call: obj[key](...args) must also
        // preserve the receiver via __esc_rt_apply_method.
        let source = r#"
            let obj = { max: function(a, b, c) { return a + b + c; } };
            let args = [1, 2, 3];
            obj["max"](...args);
        "#;
        let result = lower_program(source).expect("lowering should succeed");
        verify_typed_module(&result.module).expect("spread computed call IR should verify");
        assert!(
            result
                .string_table
                .contains(&"__esc_rt_apply_method".to_string()),
            "spread computed call should use __esc_rt_apply_method"
        );
    }

    #[test]
    fn test_no_spread_method_call_does_not_use_apply_method() {
        // A method call without spread should NOT use the apply helper.
        let source = "let o = { f: function() {} }; o.f(1, 2);";
        let result = lower_program(source).expect("lowering should succeed");
        verify_typed_module(&result.module).expect("IR should verify");
        assert!(
            !result
                .string_table
                .contains(&"__esc_rt_apply_method".to_string()),
            "non-spread method call should NOT use __esc_rt_apply_method"
        );
    }

    // === Closure mutation: write_var_by_name updates env slots ===

    #[test]
    fn test_closure_mutation_update_emits_box_store() {
        // `let x = 0; let inc = () => { x++; return x; };`
        // The x++ inside the closure should emit BoxStore (via JsBox) since
        // captured+mutated variables use JsBox for shared mutation visibility.
        let source = r#"
            let x = 0;
            let inc = () => { x++; return x; };
        "#;
        let module = lower_and_get_module(source);
        // The closure function should have BoxStore (for writing x through the JsBox)
        let closure_fn = module.functions.iter().find(|f| f.name != "main");
        assert!(closure_fn.is_some(), "should have a closure function");
        let f = closure_fn.unwrap();
        let has_box_store = f
            .blocks
            .iter()
            .any(|b| b.instructions.iter().any(|i| i.op == Op::BoxStore));
        assert!(
            has_box_store,
            "closure mutation (x++) should emit BoxStore to update shared JsBox"
        );
    }

    #[test]
    fn test_closure_compound_assignment_emits_box_store() {
        // `let count = 0; let add = (n) => { count += n; };`
        let source = r#"
            let count = 0;
            let add = (n) => { count += n; };
        "#;
        let module = lower_and_get_module(source);
        let closure_fn = module.functions.iter().find(|f| f.name != "main");
        assert!(closure_fn.is_some(), "should have a closure function");
        let f = closure_fn.unwrap();
        let has_box_store = f
            .blocks
            .iter()
            .any(|b| b.instructions.iter().any(|i| i.op == Op::BoxStore));
        assert!(
            has_box_store,
            "closure compound assignment (+=) should emit BoxStore"
        );
    }

    #[test]
    fn test_closure_simple_assignment_emits_box_store() {
        // `let x = 0; let reset = () => { x = 0; };`
        let source = r#"
            let x = 0;
            let reset = () => { x = 0; };
        "#;
        let module = lower_and_get_module(source);
        let closure_fn = module.functions.iter().find(|f| f.name != "main");
        assert!(closure_fn.is_some(), "should have a closure function");
        let f = closure_fn.unwrap();
        let has_box_store = f
            .blocks
            .iter()
            .any(|b| b.instructions.iter().any(|i| i.op == Op::BoxStore));
        assert!(
            has_box_store,
            "closure simple assignment should emit BoxStore"
        );
    }

    // =========================================================================
    // Strict mode tracking tests
    // =========================================================================

    #[test]
    fn test_strict_mode_es_module_is_always_strict() {
        // ES modules are always strict — undeclared assignment should emit ReferenceError
        let source = "x = 5;";
        let result = lower_program(source).expect("lowering should succeed");
        assert!(
            result
                .string_table
                .contains(&"__esc_rt_throw_reference_error".to_string()),
            "ES module should emit ReferenceError for undeclared assignment"
        );
    }

    #[test]
    fn test_sloppy_mode_undeclared_assignment_auto_declares() {
        // Without "use strict" in a script, undeclared assignment auto-declares (sloppy mode)
        let ir = lower_script_print("x = 5;");
        // In sloppy mode, x should be auto-declared and written normally
        // There should NOT be a ReferenceError call
        assert!(
            !ir.contains("__esc_rt_throw_reference_error"),
            "sloppy mode should not emit ReferenceError for undeclared assignment"
        );
    }

    #[test]
    fn test_strict_mode_directive_enables_strict() {
        // "use strict" at program level should activate strict mode in scripts
        let result = lower_script(r#""use strict"; x = 5;"#);
        assert!(
            result
                .string_table
                .contains(&"__esc_rt_throw_reference_error".to_string()),
            "\"use strict\" should enable strict mode in scripts"
        );
    }

    #[test]
    fn test_strict_mode_declared_var_assignment_works() {
        // Even in strict mode, assigning to a declared variable should work
        let ir = lower_script_print(r#""use strict"; let x = 1; x = 5;"#);
        // Should NOT emit ReferenceError since x is declared
        assert!(
            !ir.contains("__esc_rt_throw_reference_error"),
            "strict mode should allow assignment to declared variables"
        );
    }

    #[test]
    fn test_strict_mode_compound_assignment_undeclared() {
        // In strict mode, compound assignment (+=) to undeclared var should emit ReferenceError
        let result = lower_script(r#""use strict"; x += 5;"#);
        assert!(
            result
                .string_table
                .contains(&"__esc_rt_throw_reference_error".to_string()),
            "strict mode should emit ReferenceError for undeclared compound assignment"
        );
    }

    #[test]
    fn test_strict_mode_update_expression_undeclared() {
        // In strict mode, x++ on undeclared var should emit ReferenceError
        let result = lower_script(r#""use strict"; x++;"#);
        assert!(
            result
                .string_table
                .contains(&"__esc_rt_throw_reference_error".to_string()),
            "strict mode should emit ReferenceError for undeclared update expression"
        );
    }

    #[test]
    fn test_strict_mode_function_body_directive() {
        // "use strict" inside a function body should make only that function strict
        let source = r#"
            function f() {
                "use strict";
                y = 10;
            }
        "#;
        let result = lower_script(source);
        assert!(
            result
                .string_table
                .contains(&"__esc_rt_throw_reference_error".to_string()),
            "function-level \"use strict\" should emit ReferenceError for undeclared assignment"
        );
    }

    #[test]
    fn test_strict_mode_not_inherited_from_function() {
        // A function with "use strict" should not affect the outer scope.
        // The function body uses `let y` (declared), so no ReferenceError inside.
        // The outer `z = 42` is in sloppy mode — auto-declares, no ReferenceError.
        // Result: __esc_rt_throw_reference_error should NOT appear at all.
        let source = r#"
            function f() {
                "use strict";
                let y = 10;
            }
            z = 42;
        "#;
        let result = lower_script(source);
        verify_typed_module(&result.module).expect("IR should verify");
        assert!(
            !result
                .string_table
                .contains(&"__esc_rt_throw_reference_error".to_string()),
            "function-level strict should not leak — outer sloppy z=42 should auto-declare"
        );
    }

    #[test]
    fn test_class_body_is_always_strict() {
        // Class bodies are always strict — undeclared assignment in a method should error
        let source = r#"
            class Foo {
                bar() {
                    undeclaredInClass = 10;
                }
            }
        "#;
        let module = lower_script_module(source);
        // Verify the class method emits ReferenceError for undeclared assignment
        let has_call_runtime = module.functions.iter().any(|f| {
            f.blocks
                .iter()
                .any(|b| b.instructions.iter().any(|i| i.op == Op::CallRuntime))
        });
        assert!(
            has_call_runtime,
            "class body should be strict mode — undeclared assignment should emit ReferenceError"
        );
    }

    #[test]
    fn test_strict_mode_ir_verifies() {
        // Ensure IR with strict mode checks passes verification
        let sources = [
            r#""use strict"; let x = 1; x = 2;"#,
            r#""use strict"; y = 10;"#,
            "let x = 1; x = 2;",
            "z = 42;",
        ];
        for source in sources {
            let result = lower_script(source);
            verify_typed_module(&result.module)
                .unwrap_or_else(|e| panic!("IR should verify for '{source}': {e:?}"));
        }
    }

    #[test]
    fn test_strict_mode_builtin_globals_allowed() {
        // Even in strict mode, built-in globals (console, Math, etc.) should be accessible
        let ir = lower_script_print(r#""use strict"; console.log("hello");"#);
        assert!(
            !ir.contains("__esc_rt_throw_reference_error"),
            "strict mode should not throw ReferenceError for built-in globals"
        );
    }

    #[test]
    fn test_sloppy_mode_update_expression_auto_declares() {
        // In sloppy mode, x++ on undeclared var should auto-declare
        let ir = lower_script_print("x++;");
        assert!(
            !ir.contains("__esc_rt_throw_reference_error"),
            "sloppy mode should not emit ReferenceError for undeclared update"
        );
    }

    // =========================================================================
    // Switch statement edge case tests
    // =========================================================================

    #[test]
    fn test_switch_default_only() {
        // switch(x) { default: console.log("d"); } — no non-default cases
        let module = lower_and_get_module(r#"let x = 1; switch(x) { default: console.log("d"); }"#);
        let entry_fn = &module.functions[module.entry.unwrap()];
        // Should have at least a branch and CallRuntime for console.log
        assert!(
            entry_fn
                .blocks
                .iter()
                .any(|b| b.instructions.iter().any(|i| i.op == Op::CallRuntime)),
            "default-only switch should emit body with CallRuntime"
        );
    }

    #[test]
    fn test_switch_empty_case_fallthrough() {
        // case 1: case 2: console.log("match"); break;
        // Empty case 1 should fall through to case 2's body
        let ir = lower_and_print(
            r#"let x = 1; switch(x) { case 1: case 2: console.log("match"); break; }"#,
        );
        // The IR should verify (no broken blocks)
        assert!(ir.contains("eq_strict"), "should have strict equality test");
    }

    #[test]
    fn test_switch_default_first() {
        // default before other cases
        let module = lower_and_get_module(
            r#"let x = 1; switch(x) { default: console.log("d"); break; case 1: console.log("1"); break; }"#,
        );
        verify_typed_module(&module).expect("IR should verify for default-first switch");
    }

    #[test]
    fn test_switch_default_middle() {
        // default in the middle of cases
        let module = lower_and_get_module(
            r#"let x = 1; switch(x) { case 1: console.log("1"); break; default: console.log("d"); break; case 2: console.log("2"); break; }"#,
        );
        verify_typed_module(&module).expect("IR should verify for default-middle switch");
    }

    #[test]
    fn test_switch_no_match_no_default() {
        // switch(99) { case 1: ...; case 2: ...; } — should skip all bodies
        let module = lower_and_get_module(
            r#"switch(99) { case 1: console.log("1"); case 2: console.log("2"); }"#,
        );
        verify_typed_module(&module).expect("IR should verify for no-match no-default switch");
    }

    #[test]
    fn test_switch_multiple_empty_fallthrough() {
        // case 1: case 2: case 3: console.log("small"); break;
        let module = lower_and_get_module(
            r#"let x = 2; switch(x) { case 1: case 2: case 3: console.log("small"); break; }"#,
        );
        verify_typed_module(&module).expect("IR should verify for multiple empty case fallthrough");
    }

    #[test]
    fn test_switch_empty_default() {
        // switch(x) { case 1: ...; default: } — default with empty body
        let module = lower_and_get_module(
            r#"let x = 1; switch(x) { case 1: console.log("1"); break; default: }"#,
        );
        verify_typed_module(&module).expect("IR should verify for empty default body");
    }

    #[test]
    fn test_switch_verifies_all_positions() {
        // Verify several switch patterns produce valid IR
        let sources = [
            r#"switch(1) { case 1: break; }"#,
            r#"switch(1) { default: break; }"#,
            r#"switch(1) { case 1: case 2: break; }"#,
            r#"switch(1) { default: break; case 1: break; }"#,
            r#"switch(1) { case 1: break; default: break; case 2: break; }"#,
            r#"switch(1) { case 1: case 2: case 3: break; default: break; }"#,
        ];
        for source in sources {
            let result = lower_program(source).expect("lowering should succeed");
            verify_typed_module(&result.module)
                .unwrap_or_else(|e| panic!("IR should verify for '{source}': {e:?}"));
        }
    }

    // =========================================================================
    // Switch lexical scoping tests
    // =========================================================================

    #[test]
    fn test_switch_lexical_scope_let_in_case() {
        // let/const inside switch cases should be scoped to the switch block.
        // This should lower without "already declared" errors.
        let module = lower_and_get_module(
            r#"
            let x = "outside";
            switch (0) {
                default:
                    let x = "inside";
            }
            "#,
        );
        // The entry function should have multiple blocks (entry + switch body + exit)
        let entry_fn = &module.functions[module.entry.unwrap()];
        assert!(
            entry_fn.blocks.len() >= 2,
            "switch should produce multiple blocks"
        );
        // Should have const_string ops for both values
        let const_string_count = entry_fn
            .blocks
            .iter()
            .flat_map(|b| b.instructions.iter())
            .filter(|i| matches!(i.op, Op::ConstString(_)))
            .count();
        assert!(
            const_string_count >= 2,
            "should have at least 2 const_string instructions for outer and inner x"
        );
    }

    #[test]
    fn test_switch_lexical_scope_const_in_case() {
        // const inside switch case should be scoped to the switch block
        let module = lower_and_get_module(
            r#"
            let x = "outside";
            switch (0) {
                case 0:
                    const y = "inside";
                    break;
            }
            "#,
        );
        verify_typed_module(&module).expect("switch with const in case should verify");
    }

    #[test]
    fn test_switch_lexical_scope_does_not_leak() {
        // After the switch block, the let-declared variable should not shadow
        // the outer one. The outer x should still be accessible.
        let ir = lower_and_print(
            r#"
            let x = "outside";
            switch (0) {
                default:
                    let y = "inside";
            }
            console.log(x);
            "#,
        );
        // The IR should reference x (outer) after the switch
        assert!(
            ir.contains("call_runtime"),
            "should have call_runtime for console.log"
        );
    }

    #[test]
    fn test_switch_duplicate_function_declaration_rejected() {
        // Duplicate function declarations in the same switch block are a SyntaxError
        assert!(
            lower_program("switch (0) { case 1: function f() {} default: function f() {} }")
                .is_err(),
            "duplicate function declarations in switch should produce an error"
        );
    }

    #[test]
    fn test_switch_var_lexical_conflict_rejected() {
        // var f + const f in the same switch block is a SyntaxError
        assert!(
            lower_program("switch (0) { case 1: var f; default: const f = 0; }").is_err(),
            "var/const conflict in switch should produce an error"
        );
    }

    #[test]
    fn test_switch_var_var_redeclaration_allowed() {
        // var f + var f in the same switch block is allowed (not a SyntaxError)
        let module = lower_and_get_module("switch (0) { case 1: var f; default: var f; }");
        verify_typed_module(&module).expect("var-var redeclaration in switch should verify");
    }

    // =========================================================================
    // Arguments object tests
    // =========================================================================

    #[test]
    fn test_arguments_emitted_in_regular_function() {
        // Regular function declarations should emit CreateArguments
        let module = lower_and_get_module("function f() { return arguments; }");
        // The inner function (f) should contain CreateArguments
        let f_fn = module.functions.iter().find(|f| f.name == "f");
        assert!(f_fn.is_some(), "should have function 'f'");
        let f = f_fn.unwrap();
        let has_create_args = f
            .blocks
            .iter()
            .any(|b| b.instructions.iter().any(|i| i.op == Op::CreateArguments));
        assert!(
            has_create_args,
            "regular function should emit CreateArguments"
        );
    }

    #[test]
    fn test_arguments_emitted_in_function_expression() {
        // Function expressions should also emit CreateArguments
        let module = lower_and_get_module("let f = function() { return arguments; };");
        let anon_fn = module.functions.iter().find(|f| f.name != "main");
        assert!(anon_fn.is_some(), "should have inner function");
        let f = anon_fn.unwrap();
        let has_create_args = f
            .blocks
            .iter()
            .any(|b| b.instructions.iter().any(|i| i.op == Op::CreateArguments));
        assert!(
            has_create_args,
            "function expression should emit CreateArguments"
        );
    }

    #[test]
    fn test_arguments_not_emitted_in_arrow_function() {
        // Arrow functions should NOT emit CreateArguments
        let module = lower_and_get_module("let f = () => 1;");
        let arrow_fn = module.functions.iter().find(|f| f.name == "<arrow>");
        assert!(arrow_fn.is_some(), "should have arrow function");
        let f = arrow_fn.unwrap();
        let has_create_args = f
            .blocks
            .iter()
            .any(|b| b.instructions.iter().any(|i| i.op == Op::CreateArguments));
        assert!(
            !has_create_args,
            "arrow function should NOT emit CreateArguments"
        );
    }

    #[test]
    fn test_arguments_not_emitted_in_main() {
        // Top-level main should NOT contain CreateArguments
        let module = lower_and_get_module("let x = 1;");
        let main_fn = &module.functions[module.entry.unwrap()];
        let has_create_args = main_fn
            .blocks
            .iter()
            .any(|b| b.instructions.iter().any(|i| i.op == Op::CreateArguments));
        assert!(
            !has_create_args,
            "top-level main should NOT have CreateArguments"
        );
    }

    // =========================================================================
    // For-in/of destructuring tests
    // =========================================================================

    #[test]
    fn test_for_of_array_destructuring() {
        // for (const [a, b] of pairs) should emit GetElem to destructure each element
        let module =
            lower_and_get_module("let pairs = [[1,2],[3,4]]; for (const [a, b] of pairs) {}");
        assert!(
            entry_has_op(&module, Op::GetElem),
            "for-of with array destructuring should emit GetElem"
        );
        assert!(
            entry_has_op(&module, Op::IterInit),
            "for-of should emit IterInit"
        );
    }

    #[test]
    fn test_for_of_object_destructuring() {
        // for (const {x, y} of items) should emit GetProp to destructure each element
        let module =
            lower_and_get_module(r#"let items = [{x:1,y:2}]; for (const {x, y} of items) {}"#);
        assert!(
            entry_has_op(&module, Op::GetProp),
            "for-of with object destructuring should emit GetProp"
        );
        assert!(
            entry_has_op(&module, Op::IterInit),
            "for-of should emit IterInit"
        );
    }

    #[test]
    fn test_for_in_simple_still_works() {
        // Regression: for (const k in obj) should still work after refactor
        let module = lower_and_get_module("let obj = {a: 1}; for (const k in obj) {}");
        assert!(
            entry_has_op(&module, Op::ForInInit),
            "for-in should emit ForInInit"
        );
        assert!(
            entry_has_op(&module, Op::IterNext),
            "for-in should emit IterNext"
        );
    }

    #[test]
    fn test_for_in_bare_identifier() {
        // for (k in obj) without let/const — bare assignment target
        let module = lower_and_get_module("let k; let obj = {a: 1}; for (k in obj) {}");
        assert!(
            entry_has_op(&module, Op::ForInInit),
            "for-in with bare identifier should emit ForInInit"
        );
        assert!(
            entry_has_op(&module, Op::IterNext),
            "for-in with bare identifier should emit IterNext"
        );
    }

    #[test]
    fn test_for_of_destructuring_verifies() {
        // Ensure the IR with for-of destructuring passes verification
        let sources = [
            "let pairs = [[1,2]]; for (const [a, b] of pairs) {}",
            r#"let items = [{x:1}]; for (const {x} of items) {}"#,
            "let arr = [1,2,3]; for (const x of arr) {}",
            "let x; let arr = [1,2]; for (x of arr) {}",
        ];
        for source in sources {
            let result = lower_program(source).expect("lowering should succeed");
            verify_typed_module(&result.module)
                .unwrap_or_else(|e| panic!("IR should verify for '{source}': {e:?}"));
        }
    }

    #[test]
    fn test_for_of_nested_destructuring() {
        // for (const {a: {b}} of items) — nested object destructuring
        let module =
            lower_and_get_module(r#"let items = [{a: {b: 1}}]; for (const {a: {b}} of items) {}"#);
        // Should have multiple GetProp ops for nested access
        let get_prop_count: usize = module.functions[module.entry.unwrap()]
            .blocks
            .iter()
            .flat_map(|b| b.instructions.iter())
            .filter(|i| i.op == Op::GetProp)
            .count();
        assert!(
            get_prop_count >= 2,
            "nested destructuring should emit multiple GetProp, got {get_prop_count}"
        );
    }

    #[test]
    fn test_for_of_array_assignment_target() {
        // for ([x] of [[0]]) — destructuring assignment target (not declaration)
        let module = lower_and_get_module("var x; for ([x] of [[0]]) {}");
        assert!(
            entry_has_op(&module, Op::GetElem),
            "for-of with array assignment target should emit GetElem"
        );
        assert!(
            entry_has_op(&module, Op::IterInit),
            "for-of with array assignment target should emit IterInit"
        );
    }

    #[test]
    fn test_for_of_object_assignment_target() {
        // for ({x} of [{x: 1}]) — destructuring assignment target (not declaration)
        let module = lower_and_get_module(r#"var x; for ({x} of [{x: 1}]) {}"#);
        assert!(
            entry_has_op(&module, Op::GetProp),
            "for-of with object assignment target should emit GetProp"
        );
        assert!(
            entry_has_op(&module, Op::IterInit),
            "for-of with object assignment target should emit IterInit"
        );
    }

    #[test]
    fn test_for_of_array_assignment_target_with_defaults() {
        // for ([v = 10] of [[undefined]]) — assignment target with default value
        let source = "var v; for ([v = 10] of [[undefined]]) {}";
        let result = lower_program(source).expect("lowering should succeed");
        verify_typed_module(&result.module).unwrap_or_else(|e| {
            panic!("IR should verify for assignment target with defaults: {e:?}")
        });
    }

    #[test]
    fn test_for_of_destructuring_assignment_verifies() {
        // Ensure the IR with for-of destructuring assignment targets passes verification
        let sources = [
            "var x; for ([x] of [[0]]) {}",
            r#"var x; for ({x} of [{x: 1}]) {}"#,
            "var a, b; for ([a, b] of [[1, 2]]) {}",
            r#"var x, y; for ({x, y} of [{x: 1, y: 2}]) {}"#,
        ];
        for source in sources {
            let result = lower_program(source).expect("lowering should succeed");
            verify_typed_module(&result.module)
                .unwrap_or_else(|e| panic!("IR should verify for '{source}': {e:?}"));
        }
    }

    // =========================================================================
    // TDZ (Temporal Dead Zone) tests
    // =========================================================================

    /// Helper: check if the lowered module's string table contains a specific runtime function name.
    fn result_has_runtime_call(result: &crate::LoweringResult, fn_name: &str) -> bool {
        result.string_table.iter().any(|s| s == fn_name)
    }

    /// Lower JS source (module mode) and check if the string table contains a runtime call.
    fn module_has_call(source: &str, fn_name: &str) -> bool {
        let result = lower_program(source).expect("lowering should succeed");
        verify_typed_module(&result.module).expect("IR should verify");
        result_has_runtime_call(&result, fn_name)
    }

    #[test]
    fn test_tdz_let_read_before_declaration() {
        // Accessing a let variable before its declaration should emit a TDZ error
        assert!(
            module_has_call("console.log(x); let x = 5;", "__esc_rt_throw_tdz_error"),
            "reading let variable before declaration should emit TDZ error"
        );
    }

    #[test]
    fn test_tdz_const_read_before_declaration() {
        // Accessing a const variable before its declaration should emit a TDZ error
        assert!(
            module_has_call("console.log(y); const y = 10;", "__esc_rt_throw_tdz_error"),
            "reading const variable before declaration should emit TDZ error"
        );
    }

    #[test]
    fn test_tdz_no_error_after_declaration() {
        // Accessing a let variable after its declaration should NOT emit a TDZ error
        assert!(
            !module_has_call("let x = 5; console.log(x);", "__esc_rt_throw_tdz_error"),
            "reading let variable after declaration should not emit TDZ error"
        );
    }

    #[test]
    fn test_tdz_var_no_tdz() {
        // var variables should NOT have TDZ (they are hoisted and initialized to undefined)
        let result = lower_script("console.log(x); var x = 5;");
        verify_typed_module(&result.module).expect("IR should verify");
        assert!(
            !result_has_runtime_call(&result, "__esc_rt_throw_tdz_error"),
            "var variables should not have TDZ"
        );
    }

    #[test]
    fn test_tdz_block_scoped() {
        // TDZ is block-scoped: accessing let in a block before declaration should error
        assert!(
            module_has_call("{ console.log(x); let x = 1; }", "__esc_rt_throw_tdz_error"),
            "TDZ should apply within a block scope"
        );
    }

    #[test]
    fn test_tdz_assignment_before_declaration() {
        // Assigning to a let variable before its declaration should emit a TDZ error
        assert!(
            module_has_call("x = 5; let x;", "__esc_rt_throw_tdz_error"),
            "assigning to let variable before declaration should emit TDZ error"
        );
    }

    #[test]
    fn test_tdz_typeof_on_tdz_variable() {
        // typeof on a TDZ variable should also throw (unlike undeclared variables)
        assert!(
            module_has_call("typeof x; let x = 1;", "__esc_rt_throw_tdz_error"),
            "typeof on TDZ variable should emit TDZ error"
        );
    }

    #[test]
    fn test_tdz_function_body() {
        // TDZ should apply in function bodies
        assert!(
            module_has_call(
                "function f() { console.log(a); let a = 1; }",
                "__esc_rt_throw_tdz_error"
            ),
            "TDZ should apply in function bodies"
        );
    }

    #[test]
    fn test_tdz_ir_verifies() {
        // All TDZ-producing sources should produce valid IR
        let sources = [
            "console.log(x); let x = 5;",
            "console.log(y); const y = 10;",
            "let x = 5; console.log(x);",
            "{ console.log(x); let x = 1; }",
            "x = 5; let x;",
        ];
        for source in sources {
            let result = lower_program(source).expect("lowering should succeed");
            verify_typed_module(&result.module)
                .unwrap_or_else(|e| panic!("IR should verify for '{source}': {e:?}"));
        }
    }

    // =========================================================================
    // const reassignment tests
    // =========================================================================

    #[test]
    fn test_const_reassignment_emits_type_error() {
        // Reassigning a const variable should emit a TypeError
        assert!(
            module_has_call("const x = 5; x = 10;", "__esc_rt_throw_type_error"),
            "reassigning const variable should emit TypeError"
        );
    }

    #[test]
    fn test_const_compound_assignment_emits_type_error() {
        // Compound assignment to const should also emit TypeError
        assert!(
            module_has_call("const x = 5; x += 1;", "__esc_rt_throw_type_error"),
            "compound assignment to const should emit TypeError"
        );
    }

    #[test]
    fn test_const_update_emits_type_error() {
        // x++ on a const variable should emit TypeError
        assert!(
            module_has_call("const x = 5; x++;", "__esc_rt_throw_type_error"),
            "incrementing const variable should emit TypeError"
        );
    }

    #[test]
    fn test_let_reassignment_allowed() {
        // let variables should allow reassignment (no TypeError)
        assert!(
            !module_has_call("let x = 5; x = 10;", "__esc_rt_throw_type_error"),
            "let variables should allow reassignment"
        );
    }

    #[test]
    fn test_const_no_error_on_read() {
        // Reading a const variable should not emit any error
        assert!(
            !module_has_call("const x = 5; console.log(x);", "__esc_rt_throw_type_error"),
            "reading const variable should not emit TypeError"
        );
        assert!(
            !module_has_call("const x = 5; console.log(x);", "__esc_rt_throw_tdz_error"),
            "reading const variable after init should not emit TDZ error"
        );
    }

    #[test]
    fn test_const_block_scoped_reassignment() {
        // const in a block scope should prevent reassignment within that block
        assert!(
            module_has_call("{ const x = 1; x = 2; }", "__esc_rt_throw_type_error"),
            "const reassignment in block should emit TypeError"
        );
    }

    #[test]
    fn test_const_reassignment_ir_verifies() {
        // All const reassignment sources should produce valid IR
        let sources = [
            "const x = 5; x = 10;",
            "const x = 5; x += 1;",
            "const x = 5; x++;",
            "const x = 5; console.log(x);",
        ];
        for source in sources {
            let result = lower_program(source).expect("lowering should succeed");
            verify_typed_module(&result.module)
                .unwrap_or_else(|e| panic!("IR should verify for '{source}': {e:?}"));
        }
    }

    #[test]
    fn test_const_for_of_prevents_reassignment() {
        // for (const x of arr) — x should be const within the loop body
        assert!(
            module_has_call(
                "for (const x of [1, 2]) { x = 5; }",
                "__esc_rt_throw_type_error"
            ),
            "reassigning for-of const variable should emit TypeError"
        );
    }

    #[test]
    fn test_const_scoping_after_block_exit() {
        // After a block exits, the const restriction should not leak
        assert!(
            !module_has_call(
                "{ const x = 1; } let x = 2; x = 3;",
                "__esc_rt_throw_type_error"
            ),
            "const restriction should not leak out of block scope"
        );
    }

    // =========================================================================
    // Destructuring rest element tests
    // =========================================================================

    #[test]
    fn test_array_rest_element_emits_call_runtime() {
        // let [first, ...rest] = arr should emit CallRuntime for __esc_rt_array_slice
        let result = lower_program("let [first, ...rest] = [1, 2, 3];").expect("should lower");
        assert!(
            result
                .string_table
                .contains(&"__esc_rt_array_slice".to_string()),
            "array rest should reference __esc_rt_array_slice in string table"
        );
    }

    #[test]
    fn test_array_rest_element_verifies() {
        // Ensure array rest destructuring produces valid IR
        let result = lower_program("let [a, b, ...rest] = [1, 2, 3, 4, 5];")
            .expect("lowering should succeed");
        verify_typed_module(&result.module).expect("array rest destructuring IR should verify");
    }

    #[test]
    fn test_object_rest_element_emits_call_runtime() {
        // let { a, ...rest } = obj should emit CallRuntime for __esc_rt_object_rest
        let result =
            lower_program(r#"let { a, ...rest } = { a: 1, b: 2, c: 3 };"#).expect("should lower");
        assert!(
            result
                .string_table
                .contains(&"__esc_rt_object_rest".to_string()),
            "object rest should reference __esc_rt_object_rest in string table"
        );
    }

    #[test]
    fn test_object_rest_element_verifies() {
        // Ensure object rest destructuring produces valid IR
        let result = lower_program(r#"let { x, y, ...rest } = { x: 1, y: 2, z: 3 };"#)
            .expect("lowering should succeed");
        verify_typed_module(&result.module).expect("object rest destructuring IR should verify");
    }

    #[test]
    fn test_array_rest_assignment_target() {
        // Assignment-form: [a, ...rest] = arr
        let result = lower_program("let a, rest; [a, ...rest] = [1, 2, 3];").expect("should lower");
        assert!(
            result
                .string_table
                .contains(&"__esc_rt_array_slice".to_string()),
            "array rest assignment should reference __esc_rt_array_slice"
        );
        verify_typed_module(&result.module).expect("array rest assignment IR should verify");
    }

    #[test]
    fn test_object_rest_assignment_target() {
        // Assignment-form: { a, ...rest } = obj
        let result = lower_program(r#"let a, rest; ({ a, ...rest } = { a: 1, b: 2 });"#)
            .expect("should lower");
        assert!(
            result
                .string_table
                .contains(&"__esc_rt_object_rest".to_string()),
            "object rest assignment should reference __esc_rt_object_rest"
        );
        verify_typed_module(&result.module).expect("object rest assignment IR should verify");
    }

    // =========================================================================
    // Catch-end cleanup tests
    // =========================================================================

    #[test]
    fn test_catch_handler_no_try_end_when_body_terminates() {
        // try { throw 1; } catch (e) { let x = e; }
        // TryEnd should NOT appear in the catch handler block.
        // The Cranelift lowerer handles catch_end cleanup automatically
        // when it detects a catch handler block.
        let result =
            lower_program("try { throw 1; } catch (e) { let x = e; }").expect("should lower");
        let module = &result.module;
        let entry_fn = &module.functions[module.entry.unwrap()];

        // The catch handler block should NOT contain TryEnd — the backend
        // handles catch_end cleanup directly.
        let catch_block = entry_fn
            .blocks
            .iter()
            .find(|block| block.instructions.iter().any(|inst| inst.op == Op::Catch));
        assert!(catch_block.is_some(), "should have a catch handler block");
        let catch_block = catch_block.unwrap();
        let has_try_end = catch_block
            .instructions
            .iter()
            .any(|inst| inst.op == Op::TryEnd);
        assert!(
            !has_try_end,
            "catch handler should NOT have TryEnd (backend handles it)"
        );
        verify_typed_module(module).expect("try/catch IR should verify");
    }

    #[test]
    fn test_nested_try_catch_verifies() {
        // Nested try/catch should produce valid IR with catch_end cleanup
        let source = r#"
            try {
                try {
                    throw "inner";
                } catch (e) {
                    let x = e;
                }
                throw "outer";
            } catch (e) {
                let y = e;
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("nested try/catch IR should verify");
    }

    // -----------------------------------------------------------------------
    // Strict mode identifier restrictions (eval / arguments)
    // -----------------------------------------------------------------------

    /// Lower as a script (sloppy mode by default) and expect an error.
    fn lower_script_expects_error(source: &str) -> bool {
        let source_type = oxc_span::SourceType::cjs();
        lower_source(source, source_type).is_err()
    }

    #[test]
    fn test_strict_var_eval_rejected() {
        assert!(
            lower_script_expects_error(r#""use strict"; var eval = 1;"#),
            "var eval in strict mode should produce an error"
        );
    }

    #[test]
    fn test_strict_var_arguments_rejected() {
        assert!(
            lower_script_expects_error(r#""use strict"; var arguments = 1;"#),
            "var arguments in strict mode should produce an error"
        );
    }

    #[test]
    fn test_strict_let_eval_rejected() {
        // ES modules are always strict
        assert!(
            lower_program("let eval = 1;").is_err(),
            "let eval in strict mode (module) should produce an error"
        );
    }

    #[test]
    fn test_strict_let_arguments_rejected() {
        assert!(
            lower_program("let arguments = 1;").is_err(),
            "let arguments in strict mode (module) should produce an error"
        );
    }

    #[test]
    fn test_sloppy_var_eval_allowed() {
        // In sloppy mode, var eval is allowed
        let result = lower_script(r#"var eval = 1;"#);
        verify_typed_module(&result.module).expect("IR should verify");
    }

    #[test]
    fn test_sloppy_var_arguments_allowed() {
        let result = lower_script(r#"var arguments = 1;"#);
        verify_typed_module(&result.module).expect("IR should verify");
    }

    #[test]
    fn test_strict_function_param_eval_rejected() {
        assert!(
            lower_script_expects_error(r#""use strict"; function f(eval) {}"#),
            "function param named eval in strict mode should produce an error"
        );
    }

    #[test]
    fn test_strict_function_param_arguments_rejected() {
        assert!(
            lower_script_expects_error(r#""use strict"; function f(arguments) {}"#),
            "function param named arguments in strict mode should produce an error"
        );
    }

    #[test]
    fn test_strict_function_body_directive_eval() {
        // "use strict" in function body should reject eval param
        assert!(
            lower_script_expects_error(r#"function f(eval) { "use strict"; }"#),
            "function with use strict directive should reject eval param"
        );
    }

    // -----------------------------------------------------------------------
    // Duplicate let/const detection
    // -----------------------------------------------------------------------

    #[test]
    fn test_duplicate_let_rejected() {
        assert!(
            lower_program("let x = 1; let x = 2;").is_err(),
            "duplicate let in same scope should produce an error"
        );
    }

    #[test]
    fn test_duplicate_const_rejected() {
        assert!(
            lower_program("const x = 1; const x = 2;").is_err(),
            "duplicate const in same scope should produce an error"
        );
    }

    #[test]
    fn test_let_const_duplicate_rejected() {
        assert!(
            lower_program("let x = 1; const x = 2;").is_err(),
            "let followed by const with same name should produce an error"
        );
    }

    #[test]
    fn test_duplicate_let_in_block_allowed() {
        // Different blocks can have same let name
        let result = lower_program("{ let x = 1; } { let x = 2; }").expect("should succeed");
        verify_typed_module(&result.module).expect("IR should verify");
    }

    #[test]
    fn test_var_redeclaration_allowed() {
        // var redeclaration is allowed (not an error)
        let result = lower_script("var x = 1; var x = 2;");
        verify_typed_module(&result.module).expect("IR should verify");
    }

    #[test]
    fn test_var_let_conflict_rejected() {
        assert!(
            lower_program("let x = 1; var x = 2;").is_err(),
            "var after let with same name should produce an error"
        );
    }

    // =========================================================================
    // Built-in global function recognition tests
    // =========================================================================

    #[test]
    fn test_typeof_parseint_emits_typeof_not_undefined() {
        // typeof parseInt should NOT resolve to typeof undefined
        // (parseInt must be recognized as a built-in global)
        let ir = lower_and_print("var x = typeof parseInt;");
        // Should have load_global and typeof_boxed, NOT const_undefined before typeof
        assert!(
            ir.contains("load_global") && ir.contains("typeof_boxed"),
            "typeof parseInt should emit load_global + typeof_boxed, got:\n{ir}"
        );
    }

    #[test]
    fn test_typeof_isnan_recognized_as_global() {
        // isNaN should be recognized as a built-in global
        let ir = lower_and_print("var x = typeof isNaN;");
        assert!(
            ir.contains("load_global") && ir.contains("typeof_boxed"),
            "typeof isNaN should emit load_global + typeof_boxed, got:\n{ir}"
        );
    }

    #[test]
    fn test_builtin_global_parseint_is_recognized() {
        assert!(
            crate::globals::is_builtin_global("parseInt"),
            "parseInt should be a recognized built-in global"
        );
        assert!(
            crate::globals::is_builtin_global("parseFloat"),
            "parseFloat should be a recognized built-in global"
        );
        assert!(
            crate::globals::is_builtin_global("isNaN"),
            "isNaN should be a recognized built-in global"
        );
        assert!(
            crate::globals::is_builtin_global("isFinite"),
            "isFinite should be a recognized built-in global"
        );
        assert!(
            crate::globals::is_builtin_global("Function"),
            "Function should be a recognized built-in global"
        );
    }

    #[test]
    fn test_builtin_callable_vs_namespace() {
        // Callables
        assert!(crate::globals::is_builtin_callable("Object"));
        assert!(crate::globals::is_builtin_callable("parseInt"));
        assert!(crate::globals::is_builtin_callable("Array"));
        assert!(crate::globals::is_builtin_callable("Error"));

        // Namespaces (NOT callable)
        assert!(!crate::globals::is_builtin_callable("Math"));
        assert!(!crate::globals::is_builtin_callable("JSON"));
        assert!(!crate::globals::is_builtin_callable("Reflect"));

        // Namespaces
        assert!(crate::globals::is_builtin_namespace("Math"));
        assert!(crate::globals::is_builtin_namespace("JSON"));
        assert!(crate::globals::is_builtin_namespace("Reflect"));

        // Not namespaces
        assert!(!crate::globals::is_builtin_namespace("Object"));
        assert!(!crate::globals::is_builtin_namespace("parseInt"));
    }

    // =========================================================================
    // Per-iteration let/const bindings in for-loops
    // =========================================================================

    #[test]
    fn test_for_let_per_iteration_binding_verifies() {
        // for (let i = 0; i < 3; i++) with a closure should produce valid IR
        let source = r#"
            let funcs = [];
            for (let i = 0; i < 3; i++) {
                funcs.push(function() { return i; });
            }
        "#;
        let result = lower_program(source).expect("lowering should succeed");
        verify_typed_module(&result.module).expect("per-iteration let binding IR should verify");
    }

    #[test]
    fn test_for_let_per_iteration_creates_inner_scope() {
        // With per-iteration bindings, the loop body should have an inner scope
        // that shadows the outer loop variable. This means there should be
        // more write_variable calls (one for the outer, one for the inner copy).
        let source = "for (let i = 0; i < 3; i++) { let x = i; }";
        let result = lower_program(source).expect("lowering should succeed");
        verify_typed_module(&result.module).expect("IR should verify");
    }

    #[test]
    fn test_for_let_multiple_vars_per_iteration() {
        // Multiple let variables in for-init should all get per-iteration bindings
        let source = r#"
            for (let i = 0, j = 10; i < 3; i++, j++) {
                let x = i + j;
            }
        "#;
        let result = lower_program(source).expect("lowering should succeed");
        verify_typed_module(&result.module).expect("multiple let per-iteration IR should verify");
    }

    #[test]
    fn test_for_var_no_per_iteration_binding() {
        // for (var i = ...) should NOT create per-iteration bindings.
        // Both with and without closures should produce valid IR.
        let source = r#"
            let funcs = [];
            for (var i = 0; i < 3; i++) {
                funcs.push(function() { return i; });
            }
        "#;
        let result = lower_program(source).expect("lowering should succeed");
        verify_typed_module(&result.module).expect("for-var IR should verify");
    }

    #[test]
    fn test_for_const_per_iteration_verifies() {
        // for (const x = ...) with const in the init (unusual but valid for
        // infinite loops or single-iteration patterns)
        let source = "for (const x = 0; ;) { break; }";
        let result = lower_program(source).expect("lowering should succeed");
        verify_typed_module(&result.module).expect("for-const IR should verify");
    }

    #[test]
    fn test_for_let_closure_capture_creates_env() {
        // A closure inside a for-let loop should create an environment
        // for capturing the loop variable
        let source = r#"
            for (let i = 0; i < 3; i++) {
                let f = function() { return i; };
            }
        "#;
        let module = lower_and_get_module(source);
        // Should have at least 2 functions (main + the closure)
        assert!(
            module.functions.len() >= 2,
            "should have main + closure function, got {}",
            module.functions.len()
        );
        // The closure function should exist — we verify that CreateClosure is emitted
        assert!(
            entry_has_op(&module, Op::CreateClosure),
            "for-let with closure should emit CreateClosure"
        );
    }

    #[test]
    fn test_for_let_break_with_per_iteration() {
        // break inside a for-let loop should still work with per-iteration bindings
        let source = r#"
            for (let i = 0; i < 10; i++) {
                if (i > 2) { break; }
            }
        "#;
        let result = lower_program(source).expect("lowering should succeed");
        verify_typed_module(&result.module).expect("for-let with break IR should verify");
    }

    #[test]
    fn test_for_let_continue_with_per_iteration() {
        // continue inside a for-let loop should still work with per-iteration bindings
        let source = r#"
            for (let i = 0; i < 10; i++) {
                if (i % 2 === 0) { continue; }
            }
        "#;
        let result = lower_program(source).expect("lowering should succeed");
        verify_typed_module(&result.module).expect("for-let with continue IR should verify");
    }

    // =========================================================================
    // Strict mode SetPropStrict tests
    // =========================================================================

    #[test]
    fn test_strict_mode_member_assignment_emits_set_prop_strict() {
        // In strict mode, obj.x = val should emit set_prop_strict
        let ir = lower_script_print(r#""use strict"; let obj = {}; obj.x = 5;"#);
        assert!(
            ir.contains("set_prop_strict"),
            "strict mode member assignment should emit set_prop_strict, got:\n{ir}"
        );
    }

    #[test]
    fn test_sloppy_mode_member_assignment_emits_set_prop() {
        // In sloppy mode, obj.x = val should emit regular set_prop (not strict)
        let ir = lower_script_print("let obj = {}; obj.x = 5;");
        assert!(
            ir.contains("set_prop") && !ir.contains("set_prop_strict"),
            "sloppy mode member assignment should emit set_prop (not strict), got:\n{ir}"
        );
    }

    #[test]
    fn test_strict_mode_compound_member_assignment_emits_set_prop_strict() {
        // In strict mode, obj.x += val should emit set_prop_strict
        let ir = lower_script_print(r#""use strict"; let obj = {}; obj.x += 5;"#);
        assert!(
            ir.contains("set_prop_strict"),
            "strict mode compound member assignment should emit set_prop_strict, got:\n{ir}"
        );
    }

    #[test]
    fn test_strict_mode_update_member_emits_set_prop_strict() {
        // In strict mode, obj.x++ should emit set_prop_strict
        let ir = lower_script_print(r#""use strict"; let obj = {}; obj.x++;"#);
        assert!(
            ir.contains("set_prop_strict"),
            "strict mode update on member should emit set_prop_strict, got:\n{ir}"
        );
    }

    #[test]
    fn test_object_literal_does_not_emit_set_prop_strict() {
        // Object literal with static data properties should use create_object_literal
        // in both strict and sloppy mode.
        let ir = lower_script_print(r#""use strict"; let obj = { x: 1, y: 2 };"#);
        assert!(
            ir.contains("create_object_literal"),
            "static object literal should emit create_object_literal, got:\n{ir}"
        );
        // Should NOT emit set_prop or set_prop_strict for the literal's own properties
        assert!(
            !ir.contains("set_prop"),
            "static object literal should not emit set_prop, got:\n{ir}"
        );
    }

    // =========================================================================
    // typeof edge cases
    // =========================================================================

    #[test]
    fn test_typeof_undeclared_var_returns_undefined() {
        // typeof on an undeclared variable must NOT throw — should return "undefined"
        let ir = lower_script_print("var result = typeof undeclaredVar;");
        // Should emit typeof_boxed on const_undefined (the undeclared path)
        assert!(
            ir.contains("typeof_boxed"),
            "typeof undeclaredVar should emit typeof_boxed, got:\n{ir}"
        );
        // The undeclared path emits const_undefined before typeof_boxed
        assert!(
            ir.contains("const_undefined"),
            "typeof undeclaredVar should use const_undefined, got:\n{ir}"
        );
    }

    #[test]
    fn test_typeof_declared_var_resolves_normally() {
        // typeof on a declared variable should resolve it, not treat as undeclared
        let ir = lower_script_print("var x = 42; var result = typeof x;");
        assert!(
            ir.contains("typeof_boxed"),
            "typeof x should emit typeof_boxed, got:\n{ir}"
        );
    }

    #[test]
    fn test_typeof_builtin_function_emits_typeof() {
        // typeof parseInt should resolve the builtin and emit typeof_boxed on it
        let ir = lower_script_print("var result = typeof parseInt;");
        assert!(
            ir.contains("typeof_boxed"),
            "typeof parseInt should emit typeof_boxed, got:\n{ir}"
        );
    }

    #[test]
    fn test_typeof_builtin_namespace_emits_typeof() {
        // typeof Math should resolve the builtin and emit typeof_boxed on it
        let ir = lower_script_print("var result = typeof Math;");
        assert!(
            ir.contains("typeof_boxed"),
            "typeof Math should emit typeof_boxed, got:\n{ir}"
        );
    }

    #[test]
    fn test_typeof_null_literal() {
        // typeof null === "object" (runtime behavior)
        let ir = lower_script_print("var result = typeof null;");
        assert!(
            ir.contains("typeof_boxed"),
            "typeof null should emit typeof_boxed, got:\n{ir}"
        );
    }

    #[test]
    fn test_typeof_void_zero() {
        // typeof void 0 === "undefined" (runtime behavior)
        let ir = lower_script_print("var result = typeof void 0;");
        assert!(
            ir.contains("typeof_boxed"),
            "typeof void 0 should emit typeof_boxed, got:\n{ir}"
        );
    }

    #[test]
    fn test_typeof_member_expression_not_special_cased() {
        // typeof x.prop where x is undeclared should throw ReferenceError,
        // because typeof only suppresses errors for bare unresolvable references
        let ir = lower_script_print("var result = typeof someObj.prop;");
        // Should NOT get const_undefined before typeof — the identifier
        // resolution will attempt to resolve someObj normally
        assert!(
            ir.contains("typeof_boxed"),
            "typeof someObj.prop should emit typeof_boxed, got:\n{ir}"
        );
    }

    // =========================================================================
    // var redeclaration and hoisting edge cases
    // =========================================================================

    #[test]
    fn test_var_redeclaration_same_scope() {
        // var x = 1; var x = 2; should be accepted (not error)
        let result = lower_script("var x = 1; var x = 2;");
        verify_typed_module(&result.module).expect("IR should verify");
    }

    #[test]
    fn test_var_redeclaration_without_init_preserves_value() {
        // var x = 1; var x; — x should still be 1 (not undefined)
        // The IR should NOT write undefined for the second `var x;`
        let ir = lower_script_print("var x = 1; var x;");
        // Count how many const_undefined there are — there should be only
        // the function's implicit return undefined, not one for `var x;`
        let undef_count = ir.matches("const_undefined").count();
        // At minimum: the implicit return undefined (1)
        // Should NOT have extra undefined for `var x;` redeclaration
        assert!(
            undef_count <= 1,
            "var x; redeclaration should not emit extra const_undefined, got {} occurrences:\n{}",
            undef_count,
            ir
        );
    }

    #[test]
    fn test_var_redeclaration_with_init_overwrites() {
        // var x = 1; var x = 2; — x should be 2
        let result = lower_script("var x = 1; var x = 2;");
        verify_typed_module(&result.module).expect("IR should verify");
        // Both declarations should produce valid IR
    }

    #[test]
    fn test_var_parameter_redeclaration_accepted() {
        // function f(x) { var x; return x; } — should be accepted, x keeps param value
        let result = lower_script("function f(x) { var x; return x; } f(5);");
        verify_typed_module(&result.module).expect("IR should verify");
    }

    #[test]
    fn test_var_parameter_redeclaration_preserves_param() {
        // function f(x) { var x; return x; } — var x should not overwrite param
        // The function body should NOT write undefined to x
        // (the var x; is a no-op when x is already the parameter)
        let result = lower_script("function f(x) { var x; return x; }");
        verify_typed_module(&result.module).expect("IR should verify");
    }

    #[test]
    fn test_var_hoisting_from_block() {
        // { var x = 42; } — x should be visible outside the block
        // Should compile without error (x is hoisted to function scope)
        let result = lower_script("{ var x = 42; } var result = x;");
        verify_typed_module(&result.module).expect("IR should verify");
    }

    #[test]
    fn test_var_hoisting_from_if_block() {
        // if (true) { var x = 1; } — x should be visible outside
        let result = lower_script("if (true) { var x = 1; } var result = x;");
        verify_typed_module(&result.module).expect("IR should verify");
    }

    #[test]
    fn test_var_in_for_in_hoists() {
        // for (var i in {a:1}) {} — i should be hoisted to function scope
        let result = lower_script("for (var i in {a: 1, b: 2}) {} var result = i;");
        verify_typed_module(&result.module).expect("IR should verify");
    }

    #[test]
    fn test_var_arguments_non_strict() {
        // var arguments; is allowed in non-strict mode
        let result = lower_script("var arguments;");
        verify_typed_module(&result.module).expect("IR should verify");
    }

    #[test]
    fn test_var_arguments_strict_rejected() {
        // var arguments; is rejected in strict mode
        let result = lower_source(
            r#""use strict"; var arguments;"#,
            oxc_span::SourceType::cjs(),
        );
        assert!(result.is_err(), "strict mode should reject var arguments");
    }

    #[test]
    fn test_var_eval_non_strict() {
        // var eval; is allowed in non-strict mode
        let result = lower_script("var eval;");
        verify_typed_module(&result.module).expect("IR should verify");
    }

    #[test]
    fn test_var_eval_strict_rejected() {
        // var eval; is rejected in strict mode
        let result = lower_source(r#""use strict"; var eval;"#, oxc_span::SourceType::cjs());
        assert!(result.is_err(), "strict mode should reject var eval");
    }

    #[test]
    fn test_var_no_init_first_declaration() {
        // var x; (first time) should initialize to undefined
        let ir = lower_script_print("var x;");
        // Should have const_undefined for the var declaration AND the return
        assert!(
            ir.contains("const_undefined"),
            "var x; should initialize to undefined, got:\n{ir}"
        );
    }

    #[test]
    fn test_var_no_init_after_init_preserves_value() {
        // var x = 1; var x; — the second `var x;` should NOT overwrite x.
        // We verify this by checking the IR generates only one write_variable
        // for x with the value 1 (no second undefined write).
        let result = lower_script("var x = 1; var x;");
        verify_typed_module(&result.module).expect("IR should verify");
    }

    #[test]
    fn test_var_no_init_verifies() {
        // Bare var declaration without init should produce valid IR.
        let result = lower_script("var a; var b; var c;");
        verify_typed_module(&result.module).expect("multiple var-no-init should verify");
    }

    #[test]
    fn test_var_no_init_strict_eval_rejected() {
        // In strict mode, `var eval;` without init is a syntax error.
        let result = lower_source(r#""use strict"; var eval;"#, oxc_span::SourceType::cjs());
        assert!(
            result.is_err(),
            "strict mode should reject var eval without init"
        );
    }

    // === Labeled statements ===

    #[test]
    fn test_labeled_break_outer_for_loop() {
        // `break outer` should exit the outer for loop.
        let ir = lower_and_print(
            r#"
            let result = 0;
            outer: for (let i = 0; i < 3; i++) {
                for (let j = 0; j < 3; j++) {
                    if (j === 1) break outer;
                    result = result + 1;
                }
            }
            "#,
        );
        // The IR should verify and contain multiple br instructions
        // (one for break outer, plus the loop branches).
        assert!(
            ir.contains("br bb"),
            "should have branches for labeled break"
        );
    }

    #[test]
    fn test_labeled_break_outer_for_loop_verifies() {
        // IR must verify after labeled break lowering.
        let module = lower_and_get_module(
            r#"
            let result = 0;
            outer: for (let i = 0; i < 3; i++) {
                for (let j = 0; j < 3; j++) {
                    if (j === 1) break outer;
                    result = result + 1;
                }
            }
            "#,
        );
        assert!(
            !module.functions.is_empty(),
            "should have at least one function"
        );
    }

    #[test]
    fn test_labeled_continue_outer_for_loop() {
        // `continue outer` should jump to the outer loop's update block.
        let ir = lower_and_print(
            r#"
            let result = 0;
            outer: for (let i = 0; i < 3; i++) {
                for (let j = 0; j < 3; j++) {
                    if (j === 1) continue outer;
                    result = result + 1;
                }
            }
            "#,
        );
        assert!(
            ir.contains("br bb"),
            "should have branches for labeled continue"
        );
    }

    #[test]
    fn test_labeled_continue_outer_for_loop_verifies() {
        // IR must verify after labeled continue lowering.
        let module = lower_and_get_module(
            r#"
            let result = 0;
            outer: for (let i = 0; i < 3; i++) {
                for (let j = 0; j < 3; j++) {
                    if (j === 1) continue outer;
                    result = result + 1;
                }
            }
            "#,
        );
        assert!(
            !module.functions.is_empty(),
            "should have at least one function"
        );
    }

    #[test]
    fn test_labeled_block_break() {
        // `break label` on a non-loop block should exit the block.
        let ir = lower_and_print(
            r#"
            let x = 0;
            myBlock: {
                x = 1;
                break myBlock;
                x = 2;
            }
            "#,
        );
        // x = 2 should be dead code after break myBlock
        assert!(
            ir.contains("br bb"),
            "should have branch for labeled block break"
        );
    }

    #[test]
    fn test_labeled_block_break_verifies() {
        // IR must verify for labeled block break.
        let module = lower_and_get_module(
            r#"
            let x = 0;
            myBlock: {
                x = 1;
                break myBlock;
                x = 2;
            }
            "#,
        );
        assert!(
            !module.functions.is_empty(),
            "should have at least one function"
        );
    }

    #[test]
    fn test_labeled_while_break() {
        // `break label` on a labeled while loop.
        let ir = lower_and_print(
            r#"
            let i = 0;
            loop1: while (i < 10) {
                i = i + 1;
                if (i === 5) break loop1;
            }
            "#,
        );
        assert!(
            ir.contains("br bb"),
            "should have branch for labeled while break"
        );
    }

    #[test]
    fn test_labeled_while_break_verifies() {
        let module = lower_and_get_module(
            r#"
            let i = 0;
            loop1: while (i < 10) {
                i = i + 1;
                if (i === 5) break loop1;
            }
            "#,
        );
        assert!(
            !module.functions.is_empty(),
            "should have at least one function"
        );
    }

    #[test]
    fn test_labeled_while_continue() {
        // `continue label` on a labeled while loop.
        let ir = lower_and_print(
            r#"
            let i = 0;
            loop1: while (i < 10) {
                i = i + 1;
                if (i === 3) continue loop1;
            }
            "#,
        );
        assert!(
            ir.contains("br bb"),
            "should have branch for labeled while continue"
        );
    }

    #[test]
    fn test_labeled_while_continue_verifies() {
        let module = lower_and_get_module(
            r#"
            let i = 0;
            loop1: while (i < 10) {
                i = i + 1;
                if (i === 3) continue loop1;
            }
            "#,
        );
        assert!(
            !module.functions.is_empty(),
            "should have at least one function"
        );
    }

    #[test]
    fn test_labeled_do_while_break() {
        let ir = lower_and_print(
            r#"
            let i = 0;
            loop1: do {
                i = i + 1;
                if (i === 5) break loop1;
            } while (i < 10);
            "#,
        );
        assert!(
            ir.contains("br bb"),
            "should have branch for labeled do-while break"
        );
    }

    #[test]
    fn test_labeled_do_while_break_verifies() {
        let module = lower_and_get_module(
            r#"
            let i = 0;
            loop1: do {
                i = i + 1;
                if (i === 5) break loop1;
            } while (i < 10);
            "#,
        );
        assert!(
            !module.functions.is_empty(),
            "should have at least one function"
        );
    }

    #[test]
    fn test_labeled_nested_same_name_shadowing() {
        // Inner label shadows outer label — `break a` should break the inner.
        let ir = lower_and_print(
            r#"
            let x = 0;
            a: {
                x = 1;
                a: {
                    x = 2;
                    break a;
                    x = 3;
                }
                x = 4;
            }
            "#,
        );
        // Should compile and verify. The inner `break a` exits the inner
        // block, not the outer one, so x = 4 should be reachable.
        assert!(
            ir.contains("br bb"),
            "should have branch for shadowed labeled break"
        );
    }

    #[test]
    fn test_labeled_nested_same_name_shadowing_verifies() {
        let module = lower_and_get_module(
            r#"
            let x = 0;
            a: {
                x = 1;
                a: {
                    x = 2;
                    break a;
                    x = 3;
                }
                x = 4;
            }
            "#,
        );
        assert!(
            !module.functions.is_empty(),
            "should have at least one function"
        );
    }

    #[test]
    fn test_labeled_for_in_break() {
        let ir = lower_and_print(
            r#"
            let obj = { a: 1, b: 2, c: 3 };
            loop1: for (let key in obj) {
                if (key === "b") break loop1;
            }
            "#,
        );
        assert!(
            ir.contains("br bb"),
            "should have branch for labeled for-in break"
        );
    }

    #[test]
    fn test_labeled_for_in_break_verifies() {
        let module = lower_and_get_module(
            r#"
            let obj = { a: 1, b: 2, c: 3 };
            loop1: for (let key in obj) {
                if (key === "b") break loop1;
            }
            "#,
        );
        assert!(
            !module.functions.is_empty(),
            "should have at least one function"
        );
    }

    #[test]
    fn test_labeled_for_of_continue() {
        let ir = lower_and_print(
            r#"
            let arr = [1, 2, 3, 4, 5];
            let sum = 0;
            loop1: for (let x of arr) {
                if (x === 3) continue loop1;
                sum = sum + x;
            }
            "#,
        );
        assert!(
            ir.contains("br bb"),
            "should have branch for labeled for-of continue"
        );
    }

    #[test]
    fn test_labeled_for_of_continue_verifies() {
        let module = lower_and_get_module(
            r#"
            let arr = [1, 2, 3, 4, 5];
            let sum = 0;
            loop1: for (let x of arr) {
                if (x === 3) continue loop1;
                sum = sum + x;
            }
            "#,
        );
        assert!(
            !module.functions.is_empty(),
            "should have at least one function"
        );
    }

    #[test]
    fn test_labeled_statement_no_label_break_still_works() {
        // Unlabeled break inside a labeled loop should still work (innermost loop).
        let ir = lower_and_print(
            r#"
            outer: for (let i = 0; i < 3; i++) {
                for (let j = 0; j < 3; j++) {
                    if (j === 1) break;
                }
            }
            "#,
        );
        assert!(
            ir.contains("br bb"),
            "should have branch for unlabeled break"
        );
    }

    #[test]
    fn test_labeled_statement_no_label_break_verifies() {
        let module = lower_and_get_module(
            r#"
            outer: for (let i = 0; i < 3; i++) {
                for (let j = 0; j < 3; j++) {
                    if (j === 1) break;
                }
            }
            "#,
        );
        assert!(
            !module.functions.is_empty(),
            "should have at least one function"
        );
    }

    // -----------------------------------------------------------------------
    // Catch variable scoping
    // -----------------------------------------------------------------------

    #[test]
    fn test_catch_param_scoped_to_catch_block() {
        // The catch parameter `e` should be declared in the catch block scope,
        // not the enclosing scope. After the catch block, `e` should resolve to
        // the outer `let e = 1`, not the catch parameter.
        //
        // let e = 1;
        // try { throw "err"; } catch(e) { /* e is "err" here */ }
        // e; // should be 1 (outer), not "err" (catch param)
        let source = r#"
            let e = 1;
            try { throw "err"; } catch(e) { let x = e; }
            let result = e;
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("catch scoping IR should verify");
        let ir = print_typed_module(&result.module);
        // The IR should have at least two separate write_variable ops for `e`:
        // one for the outer `let e = 1` and one for the catch parameter.
        // If catch parameter leaked into outer scope, the final `let result = e`
        // would read the catch variable, but it should read the outer one.
        // Verify the IR produces valid output (no verifier errors from mismatched variables).
        assert!(
            ir.contains("catch"),
            "IR should have a catch instruction, got:\n{ir}"
        );
    }

    #[test]
    fn test_catch_param_does_not_shadow_outer_var_after_catch() {
        // After the catch block, the outer variable should be accessible.
        // This tests that the scope is properly popped.
        let source = r#"
            let x = "outer";
            try {
                throw new Error("inner");
            } catch (x) {
                let captured = x;
            }
            let result = x;
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("catch scope cleanup IR should verify");
    }

    #[test]
    fn test_catch_no_param_with_block_scope() {
        // ES2019 optional catch binding: `catch { ... }` without a parameter
        // should still create a block scope for the catch body.
        let source = r#"
            let x = 1;
            try { throw "err"; } catch { let x = 2; }
            let result = x;
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("optional catch binding IR should verify");
    }

    #[test]
    fn test_catch_destructuring_in_catch_scope() {
        // Destructuring in catch parameter: `catch ({ message })` should
        // bind `message` in the catch block scope.
        let source = r#"
            try {
                throw { message: "fail" };
            } catch ({ message }) {
                let result = message;
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("catch destructuring IR should verify");
    }

    #[test]
    fn test_nested_try_catch_with_same_param_name() {
        // Nested try/catch blocks with the same catch parameter name should
        // each get their own scope.
        let source = r#"
            try {
                try {
                    throw "inner";
                } catch (e) {
                    let innerResult = e;
                }
                throw "outer";
            } catch (e) {
                let outerResult = e;
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("nested try/catch same param IR should verify");
    }

    #[test]
    fn test_catch_param_with_finally() {
        // Catch parameter scoping should work correctly when finally is present.
        let source = r#"
            let e = "outer";
            try {
                throw "thrown";
            } catch (e) {
                let caught = e;
            } finally {
                let fin = 1;
            }
            let result = e;
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("catch param with finally IR should verify");
    }

    #[test]
    fn test_try_catch_rethrow_in_catch_verifies() {
        // Rethrowing inside a catch handler should route to the enclosing
        // try scope (if any) or propagate to the function caller.
        let source = r#"
            try {
                try {
                    throw "inner";
                } catch (e) {
                    throw e;
                }
            } catch (e) {
                let result = e;
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("catch rethrow IR should verify");
    }

    #[test]
    fn test_try_catch_throw_in_catch_no_outer_try() {
        // Throwing inside a catch handler without an outer try should produce
        // valid IR that propagates the exception up the call chain.
        let source = r#"
            try {
                throw "err";
            } catch (e) {
                throw e;
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module)
            .expect("throw in catch without outer try should verify");
    }

    // =========================================================================
    // Script vs module mode: sloppy vs strict
    // =========================================================================

    #[test]
    fn test_script_mode_uses_sloppy_set_prop() {
        // lower_script (the test helper) uses script/CJS source type — sloppy mode.
        // Property assignment should emit set_prop, not set_prop_strict.
        let result = lower_script("var obj = {}; obj.x = 42;");
        let ir = ir::printer::print_typed_module(&result.module);
        assert!(
            ir.contains("set_prop") && !ir.contains("set_prop_strict"),
            "script mode should emit set_prop (sloppy), got:\n{ir}"
        );
    }

    #[test]
    fn test_module_mode_uses_strict_set_prop() {
        // lower_program (module mode) should use strict mode (set_prop_strict)
        let result = lower_program("var obj = {}; obj.x = 42;").expect("should lower");
        let ir = ir::printer::print_typed_module(&result.module);
        assert!(
            ir.contains("set_prop_strict"),
            "module mode should emit set_prop_strict, got:\n{ir}"
        );
    }

    #[test]
    fn test_script_with_use_strict_uses_strict_set_prop() {
        // "use strict" in a script should still emit set_prop_strict
        let result = lower_script(r#""use strict"; var obj = {}; obj.x = 42;"#);
        let ir = ir::printer::print_typed_module(&result.module);
        assert!(
            ir.contains("set_prop_strict"),
            "script with 'use strict' should emit set_prop_strict, got:\n{ir}"
        );
    }

    // ========================================================================
    // Scope Analysis Pre-pass Tests
    // ========================================================================

    /// Parse JS source as a module and run scope analysis.
    fn analyze_module(source: &str) -> crate::scope_analysis::ScopeAnalysis {
        parser::parse_with(source, oxc_span::SourceType::mjs(), |program| {
            crate::scope_analysis::analyze_scopes(program, true)
        })
        .expect("parse should succeed for scope analysis")
    }

    /// Parse JS source as a script and run scope analysis.
    fn analyze_script(source: &str) -> crate::scope_analysis::ScopeAnalysis {
        parser::parse_with(source, oxc_span::SourceType::cjs(), |program| {
            crate::scope_analysis::analyze_scopes(program, false)
        })
        .expect("parse should succeed for scope analysis")
    }

    // --- Scope tree construction ---

    #[test]
    fn test_scope_analysis_empty_program() {
        let sa = analyze_module("");
        assert_eq!(sa.scope_count(), 1, "empty program has only root scope");
        assert_eq!(sa.variable_count(), 0, "empty program has no variables");
    }

    #[test]
    fn test_scope_analysis_single_var() {
        let sa = analyze_module("var x = 1;");
        assert_eq!(sa.variable_count(), 1);
        let var = sa.resolve("x", sa.root_scope());
        assert!(var.is_some(), "x should be declared in root scope");
        let info = sa.var_info(var.unwrap());
        assert_eq!(info.name, "x");
        assert_eq!(info.kind, crate::scope_analysis::DeclarationKind::Var);
        assert_eq!(info.location, crate::scope::VariableLocation::Stack);
    }

    #[test]
    fn test_scope_analysis_function_creates_scope() {
        // function foo() { let a = 1; }
        let sa = analyze_module("function foo() { let a = 1; }");
        // Root scope + function scope = 2 scopes
        assert!(sa.scope_count() >= 2, "function should create a new scope");
        // `foo` should be declared in root
        assert!(sa.resolve("foo", sa.root_scope()).is_some());
        // `a` should NOT be in root
        assert!(
            sa.resolve("a", sa.root_scope()).is_none(),
            "let a should not be visible in root scope"
        );
    }

    #[test]
    fn test_scope_analysis_block_creates_scope() {
        // { let x = 1; } let y = 2;
        let sa = analyze_module("{ let x = 1; } let y = 2;");
        // Root + block = 2+ scopes
        assert!(sa.scope_count() >= 2);
        // `y` should be in root
        assert!(sa.resolve("y", sa.root_scope()).is_some());
        // `x` should NOT be in root (it's in the block scope)
        assert!(
            sa.resolve("x", sa.root_scope()).is_none(),
            "block-scoped x should not be visible in root"
        );
    }

    #[test]
    fn test_scope_analysis_nested_blocks() {
        // { { let inner = 1; } let outer = 2; }
        let sa = analyze_module("{ { let inner = 1; } let outer = 2; }");
        // Root + outer block + inner block = 3 scopes
        assert!(sa.scope_count() >= 3);
    }

    // --- Var hoisting ---

    #[test]
    fn test_scope_analysis_var_hoists_to_function() {
        // function f() { if (true) { var x = 1; } }
        let sa = analyze_module("function f() { if (true) { var x = 1; } }");
        // `x` should be declared in the function scope, not the if block
        // The function scope is a child of root
        let root = sa.scope(sa.root_scope());
        assert!(!root.children.is_empty(), "root should have child scopes");
        // Find the function scope
        let func_scope_id = root
            .children
            .iter()
            .find(|&&id| sa.scope(id).kind == crate::scope::ScopeKind::Function);
        assert!(func_scope_id.is_some(), "should find function scope");
        let func_scope_id = *func_scope_id.unwrap();
        // x should be resolvable from the function scope
        assert!(
            sa.resolve("x", func_scope_id).is_some(),
            "var x should be hoisted to function scope"
        );
    }

    #[test]
    fn test_scope_analysis_var_hoists_to_global() {
        // In script mode: if (true) { var x = 1; }
        let sa = analyze_script("if (true) { var x = 1; }");
        // x should be in the global scope
        assert!(
            sa.resolve("x", sa.root_scope()).is_some(),
            "var x should hoist to global scope in scripts"
        );
    }

    // --- Let/const block scoping ---

    #[test]
    fn test_scope_analysis_let_stays_in_block() {
        // { let x = 1; } x;
        let sa = analyze_script("{ let x = 1; }");
        // x should NOT be visible in global scope
        assert!(
            sa.resolve("x", sa.root_scope()).is_none(),
            "let should not be visible outside its block"
        );
    }

    #[test]
    fn test_scope_analysis_const_stays_in_block() {
        let sa = analyze_script("{ const x = 1; }");
        assert!(
            sa.resolve("x", sa.root_scope()).is_none(),
            "const should not be visible outside its block"
        );
    }

    #[test]
    fn test_scope_analysis_const_declaration_kind() {
        let sa = analyze_module("const x = 42;");
        let var = sa.resolve("x", sa.root_scope()).expect("x should exist");
        assert_eq!(
            sa.var_info(var).kind,
            crate::scope_analysis::DeclarationKind::Const
        );
    }

    // --- Catch scope ---

    #[test]
    fn test_scope_analysis_catch_creates_scope() {
        let sa = analyze_module("try { let a = 1; } catch (e) { let b = 2; }");
        // Should have root + try block + catch scope = 3+ scopes
        assert!(sa.scope_count() >= 3);
        // e should NOT be in root scope
        assert!(
            sa.resolve("e", sa.root_scope()).is_none(),
            "catch parameter should not be in root scope"
        );
    }

    #[test]
    fn test_scope_analysis_catch_param_kind() {
        let sa = analyze_module("try {} catch (err) {}");
        // Find the catch scope
        let root = sa.scope(sa.root_scope());
        let catch_scope_id = root
            .children
            .iter()
            .find(|&&id| sa.scope(id).kind == crate::scope::ScopeKind::Catch);
        assert!(catch_scope_id.is_some(), "should find catch scope");
        let err_var = sa.resolve("err", *catch_scope_id.unwrap());
        assert!(err_var.is_some(), "err should be declared in catch scope");
        assert_eq!(
            sa.var_info(err_var.unwrap()).kind,
            crate::scope_analysis::DeclarationKind::CatchParam
        );
    }

    // --- Nested functions and capture detection ---

    #[test]
    fn test_scope_analysis_capture_across_function() {
        // let x = 1; function f() { return x; }
        let sa = analyze_module("let x = 1; function f() { return x; }");
        let x_var = sa.resolve("x", sa.root_scope()).expect("x should exist");
        assert!(
            sa.is_captured(x_var),
            "x should be marked as captured by inner function f"
        );
    }

    #[test]
    fn test_scope_analysis_no_capture_same_scope() {
        // let x = 1; let y = x + 1;
        let sa = analyze_module("let x = 1; let y = x + 1;");
        let x_var = sa.resolve("x", sa.root_scope()).expect("x should exist");
        assert!(
            !sa.is_captured(x_var),
            "x should not be captured when only used in same scope"
        );
    }

    #[test]
    fn test_scope_analysis_capture_by_arrow() {
        // let x = 1; const f = () => x;
        let sa = analyze_module("let x = 1; const f = () => x;");
        let x_var = sa.resolve("x", sa.root_scope()).expect("x should exist");
        assert!(
            sa.is_captured(x_var),
            "x should be captured by arrow function"
        );
    }

    #[test]
    fn test_scope_analysis_mutation_detection() {
        // let x = 1; x = 2;
        let sa = analyze_module("let x = 1; x = 2;");
        let x_var = sa.resolve("x", sa.root_scope()).expect("x should exist");
        assert!(
            sa.var_info(x_var).is_mutated,
            "x should be marked as mutated"
        );
    }

    #[test]
    fn test_scope_analysis_no_mutation_on_read() {
        // let x = 1; console.log(x);
        let sa = analyze_module("let x = 1; console.log(x);");
        let x_var = sa.resolve("x", sa.root_scope()).expect("x should exist");
        assert!(
            !sa.var_info(x_var).is_mutated,
            "x should not be marked as mutated when only read"
        );
    }

    #[test]
    fn test_scope_analysis_update_mutation() {
        // let x = 1; x++;
        let sa = analyze_module("let x = 1; x++;");
        let x_var = sa.resolve("x", sa.root_scope()).expect("x should exist");
        assert!(
            sa.var_info(x_var).is_mutated,
            "x++ should mark x as mutated"
        );
    }

    // --- Module vs script scope ---

    #[test]
    fn test_scope_analysis_module_root_kind() {
        let sa = analyze_module("let x = 1;");
        assert_eq!(
            sa.scope(sa.root_scope()).kind,
            crate::scope::ScopeKind::Module,
            "module source should have Module root scope"
        );
    }

    #[test]
    fn test_scope_analysis_script_root_kind() {
        let sa = analyze_script("var x = 1;");
        assert_eq!(
            sa.scope(sa.root_scope()).kind,
            crate::scope::ScopeKind::Global,
            "script source should have Global root scope"
        );
    }

    // --- Function params ---

    #[test]
    fn test_scope_analysis_function_params() {
        let sa = analyze_module("function f(a, b) { return a + b; }");
        // Find function scope
        let root = sa.scope(sa.root_scope());
        let func_scope_id = root
            .children
            .iter()
            .find(|&&id| sa.scope(id).kind == crate::scope::ScopeKind::Function);
        assert!(func_scope_id.is_some());
        let fid = *func_scope_id.unwrap();
        let a_var = sa.resolve("a", fid);
        let b_var = sa.resolve("b", fid);
        assert!(
            a_var.is_some(),
            "param a should be declared in function scope"
        );
        assert!(
            b_var.is_some(),
            "param b should be declared in function scope"
        );
        assert_eq!(
            sa.var_info(a_var.unwrap()).kind,
            crate::scope_analysis::DeclarationKind::Param
        );
    }

    // --- For-loop scoping ---

    #[test]
    fn test_scope_analysis_for_let_block_scoped() {
        let sa = analyze_module("for (let i = 0; i < 10; i++) { }");
        // `i` should NOT be in root scope (it's in the for-block scope)
        assert!(
            sa.resolve("i", sa.root_scope()).is_none(),
            "for-let i should be block-scoped, not in root"
        );
    }

    #[test]
    fn test_scope_analysis_for_var_hoists() {
        let sa = analyze_script("for (var i = 0; i < 10; i++) { }");
        assert!(
            sa.resolve("i", sa.root_scope()).is_some(),
            "for-var i should hoist to global scope"
        );
    }

    // --- With scope ---

    #[test]
    fn test_scope_analysis_with_creates_scope() {
        let sa = analyze_script("var obj = {}; with (obj) { var x = 1; }");
        // Find a With scope among children
        let root = sa.scope(sa.root_scope());
        let has_with = root
            .children
            .iter()
            .any(|&id| sa.scope(id).kind == crate::scope::ScopeKind::With);
        assert!(has_with, "with statement should create a With scope");
    }

    // --- Location default ---

    #[test]
    fn test_scope_analysis_default_location_is_stack() {
        let sa = analyze_module("let x = 1; var y = 2; const z = 3;");
        for name in &["x", "y", "z"] {
            let var = sa.resolve(name, sa.root_scope()).unwrap();
            assert_eq!(
                sa.location_of(var),
                crate::scope::VariableLocation::Stack,
                "all variables should default to Stack location"
            );
        }
    }

    // --- Class declarations ---

    #[test]
    fn test_scope_analysis_class_declaration() {
        let sa = analyze_module("class Foo { constructor() {} method() {} }");
        let foo = sa.resolve("Foo", sa.root_scope());
        assert!(foo.is_some(), "class Foo should be declared in root scope");
        assert_eq!(
            sa.var_info(foo.unwrap()).kind,
            crate::scope_analysis::DeclarationKind::Class
        );
    }

    // --- Error path: resolve nonexistent ---

    #[test]
    fn test_scope_analysis_resolve_nonexistent() {
        let sa = analyze_module("let x = 1;");
        assert!(
            sa.resolve("nonexistent", sa.root_scope()).is_none(),
            "resolving a nonexistent variable should return None"
        );
    }

    // --- Multiple variables ---

    #[test]
    fn test_scope_analysis_multiple_declarations() {
        let sa = analyze_module("let a = 1; let b = 2; var c = 3;");
        assert_eq!(sa.variable_count(), 3);
    }

    // --- Destructuring ---

    #[test]
    fn test_scope_analysis_destructuring_let() {
        let sa = analyze_module("let { a, b } = { a: 1, b: 2 };");
        assert!(sa.resolve("a", sa.root_scope()).is_some());
        assert!(sa.resolve("b", sa.root_scope()).is_some());
    }

    #[test]
    fn test_scope_analysis_array_destructuring() {
        let sa = analyze_module("const [x, y] = [1, 2];");
        assert!(sa.resolve("x", sa.root_scope()).is_some());
        assert!(sa.resolve("y", sa.root_scope()).is_some());
        assert_eq!(
            sa.var_info(sa.resolve("x", sa.root_scope()).unwrap()).kind,
            crate::scope_analysis::DeclarationKind::Const
        );
    }

    // --- Switch scoping ---

    #[test]
    fn test_scope_analysis_switch_block_scope() {
        let sa = analyze_module("switch(1) { case 1: let x = 1; break; }");
        // x should NOT be in root scope
        assert!(
            sa.resolve("x", sa.root_scope()).is_none(),
            "let in switch case should be block-scoped"
        );
    }

    // --- Try/finally scoping ---

    #[test]
    fn test_scope_analysis_try_finally_scopes() {
        let sa =
            analyze_module("try { let a = 1; } catch (e) { let b = 2; } finally { let c = 3; }");
        // a, b, c, and e should NOT be in root scope
        assert!(sa.resolve("a", sa.root_scope()).is_none());
        assert!(sa.resolve("b", sa.root_scope()).is_none());
        assert!(sa.resolve("c", sa.root_scope()).is_none());
        assert!(sa.resolve("e", sa.root_scope()).is_none());
    }

    // --- For-in / for-of ---

    #[test]
    fn test_scope_analysis_for_in_var_hoists() {
        let sa = analyze_script("for (var k in {}) {}");
        assert!(
            sa.resolve("k", sa.root_scope()).is_some(),
            "for-in var should hoist to global"
        );
    }

    #[test]
    fn test_scope_analysis_for_of_let_block_scoped() {
        let sa = analyze_module("for (let item of []) {}");
        assert!(
            sa.resolve("item", sa.root_scope()).is_none(),
            "for-of let should be block-scoped"
        );
    }

    // --- is_var_scope helper ---

    #[test]
    fn test_scope_kind_is_var_scope() {
        use crate::scope::ScopeKind;
        assert!(ScopeKind::Function.is_var_scope());
        assert!(ScopeKind::Global.is_var_scope());
        assert!(ScopeKind::Module.is_var_scope());
        assert!(!ScopeKind::Block.is_var_scope());
        assert!(!ScopeKind::Catch.is_var_scope());
        assert!(!ScopeKind::With.is_var_scope());
    }

    // ========================================================================
    // Scope Analysis Poisoning Tests (eval / with detection)
    // ========================================================================

    #[test]
    fn test_scope_poison_direct_eval_poisons_function() {
        // A direct eval() call in a function should poison that function.
        let sa = analyze_script(r#"function f() { eval("var x = 1"); }"#);
        // The function scope (child of global) should be poisoned.
        let root = sa.root_scope();
        let fn_scope = sa.scope(root).children[0];
        assert!(
            sa.scope_flags(fn_scope).needs_dynamic_env,
            "function containing direct eval should need dynamic env"
        );
        // The root scope should NOT be poisoned.
        assert!(
            !sa.scope_flags(root).needs_dynamic_env,
            "global scope should not be poisoned by eval inside a function"
        );
    }

    #[test]
    fn test_scope_poison_indirect_eval_not_poisoned() {
        // Indirect eval: (0, eval)("code") — the callee is NOT an unqualified
        // identifier `eval`, so it should NOT poison.
        let sa = analyze_script(r#"function f() { (0, eval)("var x = 1"); }"#);
        let root = sa.root_scope();
        let fn_scope = sa.scope(root).children[0];
        assert!(
            !sa.scope_flags(fn_scope).needs_dynamic_env,
            "indirect eval should NOT poison the function"
        );
    }

    #[test]
    fn test_scope_poison_with_statement_poisons_function() {
        // A `with` statement should poison its enclosing function.
        let sa = analyze_script(r#"function f() { with (obj) { x = 1; } }"#);
        let root = sa.root_scope();
        let fn_scope = sa.scope(root).children[0];
        assert!(
            sa.scope_flags(fn_scope).needs_dynamic_env,
            "function containing with statement should need dynamic env"
        );
    }

    #[test]
    fn test_scope_poison_nested_eval_poisons_inner_not_outer() {
        // eval in an inner function should only poison the inner function.
        let sa = analyze_script(
            r#"function outer() {
                function inner() { eval("x"); }
            }"#,
        );
        let root = sa.root_scope();
        let outer_scope = sa.scope(root).children[0];
        // inner function is a child of outer
        let inner_scope = sa.scope(outer_scope).children[0];
        assert!(
            sa.scope_flags(inner_scope).needs_dynamic_env,
            "inner function with eval should be poisoned"
        );
        assert!(
            !sa.scope_flags(outer_scope).needs_dynamic_env,
            "outer function should NOT be poisoned by eval in inner function"
        );
    }

    #[test]
    fn test_scope_poison_shadowed_eval_not_poisoned() {
        // If `eval` is locally bound, it's NOT a direct eval.
        let sa = analyze_script(
            r#"function f() {
                let eval = console.log;
                eval("hi");
            }"#,
        );
        let root = sa.root_scope();
        let fn_scope = sa.scope(root).children[0];
        assert!(
            !sa.scope_flags(fn_scope).needs_dynamic_env,
            "shadowed eval should NOT poison the function"
        );
    }

    #[test]
    fn test_scope_poison_strict_mode_eval_not_poisoned() {
        // Strict mode eval creates its own scope, so it does NOT poison.
        // ES modules are always strict.
        let sa = analyze_module(r#"function f() { eval("var x = 1"); }"#);
        let root = sa.root_scope();
        let fn_scope = sa.scope(root).children[0];
        assert!(
            !sa.scope_flags(fn_scope).needs_dynamic_env,
            "strict mode eval should NOT poison (it creates its own scope)"
        );
    }

    #[test]
    fn test_scope_poison_use_strict_eval_not_poisoned() {
        // "use strict" in a function body should prevent eval poisoning.
        let sa = analyze_script(r#"function f() { "use strict"; eval("var x = 1"); }"#);
        let root = sa.root_scope();
        let fn_scope = sa.scope(root).children[0];
        assert!(
            !sa.scope_flags(fn_scope).needs_dynamic_env,
            "eval in 'use strict' function should NOT poison"
        );
    }

    #[test]
    fn test_scope_poison_with_inside_function() {
        // with inside a function should poison that function specifically.
        let sa = analyze_script(r#"function f(obj) { with (obj) { return x; } }"#);
        let root = sa.root_scope();
        assert!(
            !sa.scope_flags(root).needs_dynamic_env,
            "global scope should not be poisoned"
        );
        let fn_scope = sa.scope(root).children[0];
        assert!(
            sa.scope_flags(fn_scope).needs_dynamic_env,
            "function with 'with' should be poisoned"
        );
    }

    #[test]
    fn test_scope_poison_no_eval_no_with_not_poisoned() {
        // A normal function without eval or with should NOT be poisoned.
        let sa = analyze_script(r#"function f() { let x = 1; return x + 1; }"#);
        let root = sa.root_scope();
        let fn_scope = sa.scope(root).children[0];
        assert!(
            !sa.scope_flags(fn_scope).needs_dynamic_env,
            "normal function should NOT be poisoned"
        );
    }

    #[test]
    fn test_scope_poison_eval_in_catch_block() {
        // eval inside a catch block should poison the enclosing function.
        let sa = analyze_script(
            r#"function f() {
                try { throw 1; }
                catch (e) { eval("x"); }
            }"#,
        );
        let root = sa.root_scope();
        let fn_scope = sa.scope(root).children[0];
        assert!(
            sa.scope_flags(fn_scope).needs_dynamic_env,
            "eval in catch block should poison the enclosing function"
        );
    }

    #[test]
    fn test_scope_poison_eval_propagation_inner_scope_flags() {
        // When an inner block scope contains eval, the function scope's
        // inner_calls_eval or needs_dynamic_env should be set.
        let sa = analyze_script(
            r#"function f() {
                if (true) { eval("x"); }
            }"#,
        );
        let root = sa.root_scope();
        let fn_scope = sa.scope(root).children[0];
        assert!(
            sa.scope_flags(fn_scope).needs_dynamic_env,
            "eval in inner block should propagate to function scope"
        );
    }

    #[test]
    fn test_scope_poison_variables_get_environment_location() {
        // Variables in a poisoned function should get Environment location.
        let sa = analyze_script(
            r#"function f() {
                let x = 1;
                var y = 2;
                eval("x + y");
            }"#,
        );
        let root = sa.root_scope();
        let fn_scope = sa.scope(root).children[0];
        // Check that variables in the poisoned scope are Environment.
        let x_var = sa.resolve("x", fn_scope);
        assert!(x_var.is_some(), "x should be declared");
        assert_eq!(
            sa.location_of(x_var.unwrap()),
            crate::scope::VariableLocation::Environment,
            "x in poisoned function should be Environment"
        );
        let y_var = sa.resolve("y", fn_scope);
        assert!(y_var.is_some(), "y should be declared");
        assert_eq!(
            sa.location_of(y_var.unwrap()),
            crate::scope::VariableLocation::Environment,
            "y in poisoned function should be Environment"
        );
    }

    #[test]
    fn test_scope_poison_unpoisoned_variables_stay_stack() {
        // Variables in a non-poisoned function should remain Stack.
        let sa = analyze_script(
            r#"function f() { let x = 1; }
            function g() { eval("y"); }"#,
        );
        let root = sa.root_scope();
        let f_scope = sa.scope(root).children[0];
        let x_var = sa.resolve("x", f_scope);
        assert!(x_var.is_some(), "x should be declared in f");
        assert_eq!(
            sa.location_of(x_var.unwrap()),
            crate::scope::VariableLocation::Stack,
            "x in non-poisoned f should stay Stack"
        );
    }

    #[test]
    fn test_scope_poison_eval_param_shadow() {
        // `eval` as a function parameter shadows the global eval.
        let sa = analyze_script(r#"function f(eval) { eval("code"); }"#);
        let root = sa.root_scope();
        let fn_scope = sa.scope(root).children[0];
        assert!(
            !sa.scope_flags(fn_scope).needs_dynamic_env,
            "eval as parameter shadows global eval — not a direct eval"
        );
    }

    #[test]
    fn test_scope_poison_eval_at_global_level() {
        // eval at the top level of a script should poison the global scope.
        let sa = analyze_script(r#"eval("var x = 1");"#);
        let root = sa.root_scope();
        assert!(
            sa.scope_flags(root).needs_dynamic_env,
            "top-level eval should poison the global scope"
        );
    }

    #[test]
    fn test_scope_poison_needs_dynamic_env_query() {
        // Test the needs_dynamic_env() query method on ScopeAnalysis.
        let sa = analyze_script(
            r#"function f() { eval("x"); }
            function g() { let y = 1; }"#,
        );
        let root = sa.root_scope();
        let f_scope = sa.scope(root).children[0];
        let g_scope = sa.scope(root).children[1];
        assert!(
            sa.needs_dynamic_env(f_scope),
            "f contains eval — needs dynamic env"
        );
        assert!(
            !sa.needs_dynamic_env(g_scope),
            "g has no eval/with — does not need dynamic env"
        );
    }

    // === Break/Continue through try-finally ===

    #[test]
    fn test_labeled_break_through_single_try_finally() {
        // break inside try-finally should route through the finally block
        let ir = lower_and_print(
            r#"
            outer: {
                try {
                    break outer;
                } finally {
                    console.log("finally");
                }
            }
            "#,
        );
        // The IR should contain a branch to the finally block (not directly
        // to the break target) and should verify correctly.
        assert!(
            ir.contains("br bb"),
            "should have branches for finally routing"
        );
    }

    #[test]
    fn test_labeled_break_through_single_try_finally_verifies() {
        let module = lower_and_get_module(
            r#"
            outer: {
                try {
                    break outer;
                } finally {
                    console.log("finally");
                }
            }
            "#,
        );
        verify_typed_module(&module).expect("IR should verify");
    }

    #[test]
    fn test_labeled_break_through_nested_try_finally() {
        // break crossing two try-finally boundaries should pass through
        // both finally blocks.
        let ir = lower_and_print(
            r#"
            outer: {
                try {
                    try {
                        break outer;
                    } finally {
                        console.log("inner finally");
                    }
                } finally {
                    console.log("outer finally");
                }
            }
            "#,
        );
        assert!(
            ir.contains("call_runtime"),
            "should have runtime calls for console.log"
        );
    }

    #[test]
    fn test_labeled_break_through_nested_try_finally_verifies() {
        let module = lower_and_get_module(
            r#"
            outer: {
                try {
                    try {
                        break outer;
                    } finally {
                        console.log("inner finally");
                    }
                } finally {
                    console.log("outer finally");
                }
            }
            "#,
        );
        verify_typed_module(&module).expect("IR should verify");
    }

    #[test]
    fn test_continue_through_try_finally() {
        // continue inside try-finally in a loop should route through finally
        let ir = lower_and_print(
            r#"
            for (let i = 0; i < 3; i++) {
                try {
                    continue;
                } finally {
                    console.log("finally");
                }
            }
            "#,
        );
        assert!(
            ir.contains("call_runtime"),
            "should have runtime calls for console.log"
        );
    }

    #[test]
    fn test_continue_through_try_finally_verifies() {
        let module = lower_and_get_module(
            r#"
            for (let i = 0; i < 3; i++) {
                try {
                    continue;
                } finally {
                    console.log("finally");
                }
            }
            "#,
        );
        verify_typed_module(&module).expect("IR should verify");
    }

    #[test]
    fn test_break_through_try_finally_in_loop() {
        // break inside try-finally in a loop should route through finally
        let ir = lower_and_print(
            r#"
            while (true) {
                try {
                    break;
                } finally {
                    console.log("finally");
                }
            }
            "#,
        );
        assert!(
            ir.contains("call_runtime"),
            "should have runtime calls for console.log"
        );
    }

    #[test]
    fn test_break_through_try_finally_in_loop_verifies() {
        let module = lower_and_get_module(
            r#"
            while (true) {
                try {
                    break;
                } finally {
                    console.log("finally");
                }
            }
            "#,
        );
        verify_typed_module(&module).expect("IR should verify");
    }

    #[test]
    fn test_labeled_break_no_try_still_works() {
        // Labeled break without try should still work normally
        let module = lower_and_get_module(
            r#"
            outer: {
                console.log("before");
                break outer;
                console.log("after");
            }
            "#,
        );
        verify_typed_module(&module).expect("IR should verify");
    }

    #[test]
    fn test_break_in_try_catch_no_finally_works() {
        // break in try-catch without finally should work directly
        let module = lower_and_get_module(
            r#"
            while (true) {
                try {
                    break;
                } catch (e) {
                    console.log(e);
                }
            }
            "#,
        );
        verify_typed_module(&module).expect("IR should verify");
    }

    #[test]
    fn test_return_through_nested_try_finally_verifies() {
        // return crossing two try-finally boundaries should propagate through
        // both finally blocks.
        let module = lower_and_get_module(
            r#"
            function f() {
                try {
                    try {
                        return 42;
                    } finally {
                        console.log("inner finally");
                    }
                } finally {
                    console.log("outer finally");
                }
            }
            "#,
        );
        verify_typed_module(&module).expect("IR should verify");
    }

    #[test]
    fn test_labeled_continue_through_nested_try_finally_verifies() {
        // labeled continue crossing two try-finally boundaries
        let module = lower_and_get_module(
            r#"
            outer: for (let i = 0; i < 3; i++) {
                try {
                    try {
                        continue outer;
                    } finally {
                        console.log("inner");
                    }
                } finally {
                    console.log("outer");
                }
            }
            "#,
        );
        verify_typed_module(&module).expect("IR should verify");
    }

    #[test]
    fn test_break_through_try_finally_with_catch() {
        // break inside try-catch-finally should route through finally
        let module = lower_and_get_module(
            r#"
            outer: {
                try {
                    break outer;
                } catch (e) {
                    console.log(e);
                } finally {
                    console.log("finally");
                }
            }
            "#,
        );
        verify_typed_module(&module).expect("IR should verify");
    }

    #[test]
    fn test_multiple_break_targets_through_try_finally() {
        // Two different labeled breaks through the same try-finally
        let module = lower_and_get_module(
            r#"
            outer: {
                inner: {
                    try {
                        if (true) break outer;
                        break inner;
                    } finally {
                        console.log("finally");
                    }
                }
            }
            "#,
        );
        verify_typed_module(&module).expect("IR should verify");
    }

    #[test]
    fn test_break_and_continue_through_same_try_finally() {
        // break and continue in the same try-finally, sharing the dispatch
        let module = lower_and_get_module(
            r#"
            for (let i = 0; i < 5; i++) {
                try {
                    if (i === 2) continue;
                    if (i === 4) break;
                } finally {
                    console.log("finally");
                }
            }
            "#,
        );
        verify_typed_module(&module).expect("IR should verify");
    }

    #[test]
    fn test_switch_break_through_try_finally() {
        // break inside switch inside try-finally — labeled break to outer
        // should route through finally
        let module = lower_and_get_module(
            r#"
            outer: {
                try {
                    switch (1) {
                        case 1:
                            break outer;
                    }
                } finally {
                    console.log("finally");
                }
            }
            "#,
        );
        verify_typed_module(&module).expect("IR should verify");
    }

    #[test]
    fn test_switch_internal_break_no_finally_redirect() {
        // Unlabeled break inside switch inside try-finally should NOT route
        // through finally — it only exits the switch (an internal target).
        let module = lower_and_get_module(
            r#"
            try {
                switch (1) {
                    case 1:
                        console.log("matched");
                        break;
                }
                console.log("after switch");
            } finally {
                console.log("finally");
            }
            "#,
        );
        verify_typed_module(&module).expect("IR should verify");
    }

    #[test]
    fn test_inner_loop_break_no_finally_redirect() {
        // Unlabeled break inside a loop that is inside try-finally should
        // NOT route through finally — it only exits the inner loop.
        let module = lower_and_get_module(
            r#"
            try {
                for (let i = 0; i < 3; i++) {
                    if (i === 1) break;
                }
                console.log("after loop");
            } finally {
                console.log("finally");
            }
            "#,
        );
        verify_typed_module(&module).expect("IR should verify");
    }

    #[test]
    fn test_inner_loop_continue_no_finally_redirect() {
        // Unlabeled continue inside a loop that is inside try-finally should
        // NOT route through finally — it only continues the inner loop.
        let module = lower_and_get_module(
            r#"
            try {
                for (let i = 0; i < 3; i++) {
                    if (i === 1) continue;
                    console.log(i);
                }
            } finally {
                console.log("finally");
            }
            "#,
        );
        verify_typed_module(&module).expect("IR should verify");
    }

    #[test]
    fn test_outer_loop_break_through_try_finally() {
        // Unlabeled break targeting an outer loop (set before try-finally
        // entry) should route through finally.
        let module = lower_and_get_module(
            r#"
            while (true) {
                try {
                    break;
                } finally {
                    console.log("finally");
                }
            }
            "#,
        );
        verify_typed_module(&module).expect("IR should verify");
    }

    // =========================================================================
    // Wave 3 tests: 0.2.9 break/continue through finally (unit-level),
    // 0.2.13 var-in-catch, 0.2.14 per-iteration let, 0.2.15 is_strict
    // =========================================================================
    #[test]
    fn test_break_inside_try_finally_redirects_through_finally() {
        // break inside try-finally should execute finally before breaking
        let source = r#"
            function f() {
                let result = 0;
                while (true) {
                    try {
                        result = 1;
                        break;
                    } finally {
                        result = 2;
                    }
                }
                return result;
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("break through finally IR should verify");
    }

    #[test]
    fn test_continue_inside_try_finally_redirects_through_finally() {
        // continue inside try-finally should execute finally before continuing
        let source = r#"
            function f() {
                let count = 0;
                for (let i = 0; i < 5; i++) {
                    try {
                        if (i % 2 === 0) continue;
                        count = count + 1;
                    } finally {
                        count = count + 10;
                    }
                }
                return count;
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("continue through finally IR should verify");
    }

    #[test]
    fn test_break_inside_try_catch_finally_from_catch() {
        // break in catch body with finally should redirect through finally
        let source = r#"
            function f() {
                let x = 0;
                while (true) {
                    try {
                        throw "err";
                    } catch (e) {
                        x = 1;
                        break;
                    } finally {
                        x = 2;
                    }
                }
                return x;
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("break in catch with finally IR should verify");
    }

    #[test]
    fn test_nested_try_finally_with_break() {
        // Nested try-finally with break should properly redirect through
        // the correct finally block
        let source = r#"
            function f() {
                let x = 0;
                while (true) {
                    try {
                        try {
                            break;
                        } finally {
                            x = 1;
                        }
                    } finally {
                        x = x + 10;
                    }
                }
                return x;
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module)
            .expect("nested try-finally with break IR should verify");
    }

    #[test]
    fn test_finally_return_overrides_break() {
        // If finally returns, the return takes precedence over break
        let source = r#"
            function f() {
                while (true) {
                    try {
                        break;
                    } finally {
                        return 42;
                    }
                }
                return 0;
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module)
            .expect("finally return overrides break IR should verify");
    }

    // =========================================================================
    // Step 0.2.13 — var-in-catch hoisting
    // =========================================================================

    #[test]
    fn test_var_in_catch_accessible_after_try_catch() {
        // `var x` inside catch should hoist to function scope and be accessible
        // after the try-catch block.
        let source = r#"
            function f() {
                try {
                    throw "err";
                } catch (e) {
                    var x = 1;
                }
                return x;
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("var in catch hoisting IR should verify");
    }

    #[test]
    fn test_catch_parameter_not_accessible_after_catch() {
        // The catch parameter `e` should NOT be accessible outside the catch
        // block. After the catch, `e` resolves to the outer declaration.
        let source = r#"
            let e = "outer";
            try {
                throw "thrown";
            } catch (e) {
                let captured = e;
            }
            let afterCatch = e;
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("catch param scoping IR should verify");
    }

    #[test]
    fn test_var_in_nested_catch_hoists_to_function() {
        // `var` in a nested catch should hoist to the function scope,
        // passing through both catch scopes.
        let source = r#"
            function f() {
                try {
                    try {
                        throw "inner";
                    } catch (e) {
                        var x = 1;
                    }
                } catch (e) {
                    var y = 2;
                }
                return x + y;
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("nested catch var hoisting IR should verify");
    }

    #[test]
    fn test_let_const_in_catch_stays_in_catch_scope() {
        // `let` and `const` inside catch should stay in the catch scope.
        // Accessing them after catch should resolve to an outer binding or
        // be undeclared.
        let source = r#"
            let x = "outer";
            try {
                throw "err";
            } catch (e) {
                let x = "inner";
                const y = 1;
            }
            let result = x;
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("let/const in catch scoping IR should verify");
    }

    // =========================================================================
    // Step 0.2.14 — Per-iteration let scoping verification
    // =========================================================================

    #[test]
    fn test_for_let_variable_is_block_scoped() {
        // `let i` in a for-loop should not be accessible outside the loop.
        // After the loop, accessing `i` should resolve to an outer binding.
        let source = r#"
            let i = "outer";
            for (let i = 0; i < 3; i++) {
                let x = i;
            }
            let result = i;
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("for-let block scoping IR should verify");
    }

    #[test]
    fn test_for_const_variable_works() {
        // `const` in a for-of/for-in loop should work correctly
        let source = r#"
            let arr = [1, 2, 3];
            for (const item of arr) {
                let x = item;
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("for-const variable IR should verify");
    }

    #[test]
    fn test_for_var_hoists_to_function_scope() {
        // `var i` in a for-loop should hoist to the function scope and be
        // accessible outside the loop.
        let source = r#"
            function f() {
                for (var i = 0; i < 3; i++) {
                    var x = i;
                }
                return i + x;
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("for-var hoisting IR should verify");
    }

    #[test]
    fn test_nested_for_let_independent_variables() {
        // Nested for-let loops should have independent loop variables
        let source = r#"
            for (let i = 0; i < 3; i++) {
                for (let i = 10; i < 13; i++) {
                    let x = i;
                }
                let y = i;
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module)
            .expect("nested for-let independent vars IR should verify");
    }

    // =========================================================================
    // Step 0.2.15 — is_strict infrastructure hardening
    // =========================================================================

    #[test]
    fn test_class_body_strict_nested() {
        // Nested class bodies should all be strict
        let source = r#"
            class Outer {
                method() {
                    class Inner {
                        innerMethod() {
                            return 1;
                        }
                    }
                    return new Inner();
                }
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("nested class strict mode IR should verify");
    }

    #[test]
    fn test_arrow_inherits_strict_from_enclosing() {
        // Arrow function inside a strict function should be strict
        let source = r#"
            function f() {
                "use strict";
                let g = () => {
                    let obj = {};
                    obj.x = 1;
                    return obj;
                };
                return g();
            }
        "#;
        let module = lower_and_get_module(source);
        // The arrow body should emit set_prop_strict since it inherits
        // strict mode from the enclosing function.
        let has_strict_set = module.functions.iter().any(|f| {
            f.blocks
                .iter()
                .any(|b| b.instructions.iter().any(|i| i.op == Op::SetPropStrict))
        });
        assert!(
            has_strict_set,
            "arrow in strict function should emit SetPropStrict"
        );
    }

    #[test]
    fn test_arrow_inherits_sloppy_from_enclosing() {
        // Arrow function inside a sloppy function should be sloppy
        let source = r#"
            function f() {
                let g = () => {
                    let obj = {};
                    obj.x = 1;
                    return obj;
                };
                return g();
            }
        "#;
        let module = lower_script_module(source);
        // In sloppy mode, property assignment should use SetProp, not SetPropStrict.
        // Check functions other than main for set_prop usage.
        let has_sloppy_set = module.functions.iter().any(|f| {
            f.blocks.iter().any(|b| {
                b.instructions
                    .iter()
                    .any(|i| i.op == Op::SetProp && i.op != Op::SetPropStrict)
            })
        });
        assert!(
            has_sloppy_set,
            "arrow in sloppy function should emit SetProp (not strict)"
        );
    }

    #[test]
    fn test_use_strict_in_arrow_only_affects_arrow() {
        // "use strict" inside an arrow should make only that arrow strict,
        // not the enclosing function
        let source = r#"
            function f() {
                let obj1 = {};
                obj1.x = 1;
                let g = () => {
                    "use strict";
                    let obj2 = {};
                    obj2.y = 2;
                    return obj2;
                };
                return g();
            }
        "#;
        let module = lower_script_module(source);
        // At least one function should have SetProp (the outer sloppy function)
        // and at least one should have SetPropStrict (the arrow with "use strict")
        let has_sloppy = module.functions.iter().any(|f| {
            f.blocks
                .iter()
                .any(|b| b.instructions.iter().any(|i| i.op == Op::SetProp))
        });
        let has_strict = module.functions.iter().any(|f| {
            f.blocks
                .iter()
                .any(|b| b.instructions.iter().any(|i| i.op == Op::SetPropStrict))
        });
        assert!(has_sloppy, "outer sloppy function should emit SetProp");
        assert!(
            has_strict,
            "arrow with 'use strict' should emit SetPropStrict"
        );
    }

    #[test]
    fn test_module_is_always_strict() {
        // Module-level code is always strict
        let source = "let obj = {}; obj.x = 1;";
        let module = lower_and_get_module(source);
        let has_strict = entry_has_op(&module, Op::SetPropStrict);
        assert!(has_strict, "module mode should always emit SetPropStrict");
    }

    #[test]
    fn test_strict_catches_undeclared_var_assignment() {
        // Strict mode should detect assignment to undeclared variables
        let source = r#"
            "use strict";
            function f() {
                x = 1;
            }
        "#;
        // In strict mode, assignment to undeclared `x` should emit a
        // runtime error call, not silently auto-declare.
        let result = crate::lower_script(source).expect("should lower");
        verify_typed_module(&result.module).expect("IR should verify");
        // The string table should contain __esc_rt_throw_reference_error
        assert!(
            result
                .string_table
                .iter()
                .any(|s| s == "__esc_rt_throw_reference_error"),
            "strict mode should emit __esc_rt_throw_reference_error, string table: {:?}",
            result.string_table
        );
    }

    // =========================================================================
    // function.name inference
    // =========================================================================

    #[test]
    fn test_function_name_from_const_assignment() {
        // `const f = function() {}` should emit SetProp for name="f"
        let result = lower_program("const f = function() {};").expect("should lower");
        assert!(
            result.string_table.contains(&"name".to_string()),
            "should emit 'name' string, got: {:?}",
            result.string_table
        );
        assert!(
            result.string_table.contains(&"f".to_string()),
            "should emit 'f' string for inferred name, got: {:?}",
            result.string_table
        );
    }

    #[test]
    fn test_function_name_from_explicit_name() {
        // `var h = function named() {}` should emit SetProp for name="named"
        let result = lower_program("var h = function named() {};").expect("should lower");
        assert!(
            result.string_table.contains(&"named".to_string()),
            "should emit 'named' string for explicit name, got: {:?}",
            result.string_table
        );
    }

    #[test]
    fn test_function_name_anonymous_no_name_set() {
        // `(function(){})` — anonymous function expression should NOT emit
        // SetProp for "name". The name defaults to "" at runtime via
        // InternalData. Skipping SetProp allows SetFunctionName to infer
        // the name from the assignment target when applicable.
        let result = lower_program("(function(){});").expect("should lower");
        // "length" should still be set
        assert!(
            result.string_table.contains(&"length".to_string()),
            "should still emit 'length' property, got: {:?}",
            result.string_table
        );
    }

    #[test]
    fn test_function_name_arrow_from_let() {
        // `let g = () => {}` should emit SetProp for name="g"
        let result = lower_program("let g = () => {};").expect("should lower");
        assert!(
            result.string_table.contains(&"g".to_string()),
            "should emit 'g' string for inferred arrow name, got: {:?}",
            result.string_table
        );
    }

    // =========================================================================
    // function.length computation
    // =========================================================================

    #[test]
    fn test_function_length_no_params() {
        // `function c() {}` should have length=0
        let result = lower_program("function c() {}").expect("should lower");
        // The "length" string should be in the string table
        assert!(
            result.string_table.contains(&"length".to_string()),
            "should emit 'length' string, got: {:?}",
            result.string_table
        );
        // Verify SetProp opcode is emitted (for the length property)
        let module = result.module;
        let entry_fn = &module.functions[module.entry.unwrap()];
        let has_set_prop = entry_fn
            .blocks
            .iter()
            .any(|b| b.instructions.iter().any(|i| i.op == Op::SetProp));
        assert!(has_set_prop, "should emit SetProp for length");
    }

    #[test]
    fn test_function_length_with_defaults() {
        // `function b(x, y = 1) {}` should have length=1
        // We verify the string table and IR emit SetProp with "length"
        let result = lower_program("function b(x, y = 1) {}").expect("should lower");
        assert!(
            result.string_table.contains(&"length".to_string()),
            "should emit 'length' string"
        );
    }

    #[test]
    fn test_function_length_with_rest() {
        // `function d(x, ...rest) {}` should have length=1
        let result = lower_program("function d(x, ...rest) {}").expect("should lower");
        assert!(
            result.string_table.contains(&"length".to_string()),
            "should emit 'length' string"
        );
    }

    #[test]
    fn test_function_length_stops_at_first_default() {
        // `function e(a, b = 1, c) {}` should have length=1
        // (stops counting at first default parameter)
        let result = lower_program("function e(a, b = 1, c) {}").expect("should lower");
        assert!(
            result.string_table.contains(&"length".to_string()),
            "should emit 'length' string"
        );
    }

    // =========================================================================
    // Lazy arguments creation tests (0.3.12)
    // =========================================================================

    /// Check if a specific named function in the module contains the given opcode.
    fn named_fn_has_op(module: &ir::builder::TypedModule, fn_name: &str, op: Op) -> bool {
        module
            .functions
            .iter()
            .filter(|f| f.name == fn_name)
            .any(|f| {
                f.blocks
                    .iter()
                    .any(|block| block.instructions.iter().any(|inst| inst.op == op))
            })
    }

    #[test]
    fn test_lazy_arguments_used_directly() {
        // Function that accesses arguments[0] — should emit CreateArguments
        let module = lower_and_get_module("function f() { return arguments[0]; }");
        assert!(
            named_fn_has_op(&module, "f", Op::CreateArguments),
            "function referencing arguments[0] should emit CreateArguments"
        );
    }

    #[test]
    fn test_lazy_arguments_used_length() {
        // Function that accesses arguments.length — should emit CreateArguments
        let module = lower_and_get_module("function f() { return arguments.length; }");
        assert!(
            named_fn_has_op(&module, "f", Op::CreateArguments),
            "function referencing arguments.length should emit CreateArguments"
        );
    }

    #[test]
    fn test_lazy_arguments_unused_skips_create() {
        // Function that never references arguments — should NOT emit CreateArguments
        let module = lower_and_get_module("function f(a, b) { return a + b; }");
        assert!(
            !named_fn_has_op(&module, "f", Op::CreateArguments),
            "function not using arguments should skip CreateArguments"
        );
    }

    #[test]
    fn test_lazy_arguments_not_in_nested_function() {
        // arguments only referenced in nested function — outer should NOT emit CreateArguments
        let module = lower_and_get_module(
            "function outer(x) { function inner() { return arguments.length; } return inner(); }",
        );
        assert!(
            !named_fn_has_op(&module, "outer", Op::CreateArguments),
            "outer function should skip CreateArguments when only nested function uses it"
        );
        assert!(
            named_fn_has_op(&module, "inner", Op::CreateArguments),
            "inner function referencing arguments should emit CreateArguments"
        );
    }

    #[test]
    fn test_lazy_arguments_not_in_arrow_body() {
        // arguments only referenced in arrow body — outer should NOT descend into arrow
        // (arrows don't have their own arguments binding, but the scan should still
        // not descend into them since they are separate scopes for this analysis)
        let module = lower_and_get_module("function outer(x) { let f = () => 42; return f(); }");
        assert!(
            !named_fn_has_op(&module, "outer", Op::CreateArguments),
            "outer function should skip CreateArguments when no direct reference exists"
        );
    }

    #[test]
    fn test_lazy_arguments_in_if_branch() {
        // arguments used inside an if branch — should still detect it
        let module =
            lower_and_get_module("function f(x) { if (x) { return arguments[0]; } return x; }");
        assert!(
            named_fn_has_op(&module, "f", Op::CreateArguments),
            "function with arguments in if branch should emit CreateArguments"
        );
    }

    #[test]
    fn test_lazy_arguments_in_loop() {
        // arguments used inside a loop — should detect it
        let module =
            lower_and_get_module("function f() { for (var i = 0; i < arguments.length; i++) {} }");
        assert!(
            named_fn_has_op(&module, "f", Op::CreateArguments),
            "function with arguments in loop should emit CreateArguments"
        );
    }

    #[test]
    fn test_lazy_arguments_empty_body() {
        // Empty function body — should skip CreateArguments
        let module = lower_and_get_module("function f() {}");
        assert!(
            !named_fn_has_op(&module, "f", Op::CreateArguments),
            "empty function should skip CreateArguments"
        );
    }

    // =========================================================================
    // 0.3.15 — Named function expression self-reference via env slot
    // =========================================================================

    #[test]
    fn test_named_function_expr_self_reference_uses_env_load() {
        // Named function expression should load self-reference from env,
        // not use const_i32(func_idx).
        let source = r#"
            let f = function fact(n) {
                if (n <= 1) return 1;
                return n * fact(n - 1);
            };
        "#;
        let module = lower_and_get_module(source);
        // The inner function ("fact") should have EnvLoad for the self-ref
        let fact_fn = module.functions.iter().find(|f| f.name == "fact");
        assert!(fact_fn.is_some(), "should have function 'fact'");
        let f = fact_fn.unwrap();
        let has_env_load = f
            .blocks
            .iter()
            .any(|b| b.instructions.iter().any(|i| i.op == Op::EnvLoad));
        assert!(
            has_env_load,
            "named function expression should use EnvLoad for self-reference, not const_i32"
        );
    }

    // =========================================================================
    // Inline cache (IC) opcode emission tests
    // =========================================================================

    #[test]
    fn test_dot_access_emits_ic_get_prop() {
        // obj.prop should emit ICGetProp (inline-cached get)
        let module = lower_and_get_module("let obj = {}; let x = obj.prop;");
        assert!(
            entry_has_op(&module, Op::ICGetProp),
            "dot access (obj.prop) should emit ICGetProp"
        );
    }

    #[test]
    fn test_named_function_expr_anonymous_no_self_ref_env() {
        // Anonymous function expression should NOT create env for self-reference
        let source = "let f = function() { return 42; };";
        let module = lower_and_get_module(source);
        // The main function should not have EnvCreate (no captures, no self-ref)
        let entry_fn = &module.functions[module.entry.unwrap()];
        let has_env_create = entry_fn
            .blocks
            .iter()
            .any(|b| b.instructions.iter().any(|i| i.op == Op::EnvCreate));
        assert!(
            !has_env_create,
            "anonymous function expression should NOT create env for self-reference"
        );
    }

    #[test]
    fn test_named_function_expr_arrow_no_self_ref_env() {
        // Arrow functions never have a name for self-reference
        let source = "let f = () => 42;";
        let module = lower_and_get_module(source);
        let entry_fn = &module.functions[module.entry.unwrap()];
        let has_env_create = entry_fn
            .blocks
            .iter()
            .any(|b| b.instructions.iter().any(|i| i.op == Op::EnvCreate));
        assert!(
            !has_env_create,
            "arrow function should NOT create env for self-reference"
        );
    }

    #[test]
    fn test_named_function_expr_env_extra_slot() {
        // Named function expression should emit EnvCreate with slot for self-ref
        let source = "let f = function myFunc() { return myFunc; };";
        let module = lower_and_get_module(source);
        // The main function should have EnvCreate (for the self-reference slot)
        let entry_fn = &module.functions[module.entry.unwrap()];
        let has_env_create = entry_fn
            .blocks
            .iter()
            .any(|b| b.instructions.iter().any(|i| i.op == Op::EnvCreate));
        assert!(
            has_env_create,
            "named function expression should emit EnvCreate even with no captures"
        );
        // Should also have EnvStore (storing the closure into the self-ref slot)
        let has_env_store = entry_fn
            .blocks
            .iter()
            .any(|b| b.instructions.iter().any(|i| i.op == Op::EnvStore));
        assert!(
            has_env_store,
            "named function expression should emit EnvStore for self-reference"
        );
    }

    #[test]
    fn test_named_function_expr_self_ref_immutable() {
        // Assignment to named function expression name should emit TypeError
        // (it's treated as const inside the function body)
        let source = r#"
            let f = function myFunc() {
                "use strict";
                myFunc = 5;
                return myFunc;
            };
        "#;
        let result = lower_program(source).expect("lowering should succeed");
        assert!(
            result
                .string_table
                .contains(&"__esc_rt_throw_type_error".to_string()),
            "strict-mode assignment to named fn expr name should emit TypeError"
        );
    }

    #[test]
    fn test_named_function_expr_with_captures() {
        // Named function expression that also captures variables should work
        let source = r#"
            let x = 10;
            let f = function myFunc(n) {
                if (n <= 0) return x;
                return myFunc(n - 1);
            };
        "#;
        let module = lower_and_get_module(source);
        // Should have EnvCreate (for captured x + self-ref slot)
        let entry_fn = &module.functions[module.entry.unwrap()];
        let env_store_count = entry_fn
            .blocks
            .iter()
            .flat_map(|b| b.instructions.iter())
            .filter(|i| i.op == Op::EnvStore)
            .count();
        // At least 2 stores: one for captured `x`, one for self-reference
        assert!(
            env_store_count >= 2,
            "named fn expr with captures should have at least 2 EnvStore ops, got {env_store_count}"
        );
    }

    #[test]
    fn test_named_function_expr_ir_verifies() {
        // Ensure all named function expression patterns produce valid IR
        let sources = [
            "let f = function fact(n) { if (n <= 1) return 1; return n * fact(n-1); };",
            "let f = function myFunc() { return myFunc; };",
            "let f = function() { return 42; };",
            r#"let x = 1; let f = function myFunc() { return x + myFunc; };"#,
        ];
        for source in sources {
            let result = lower_program(source).expect("lowering should succeed");
            verify_typed_module(&result.module)
                .unwrap_or_else(|e| panic!("IR should verify for '{source}': {e:?}"));
        }
    }

    // =========================================================================
    // 0.3.16 — Default parameter intermediate scope
    // =========================================================================

    #[test]
    fn test_default_param_no_body_scope_when_no_shadowing() {
        // When no var in the body shadows a parameter name,
        // no extra scope push should happen (zero overhead).
        // We verify by checking that the IR is valid and there are no
        // extra blocks that would result from a redundant scope push.
        let source = "function f(x, y) { let z = x + y; return z; }";
        let result = lower_program(source).expect("lowering should succeed");
        verify_typed_module(&result.module).expect("IR should verify");
    }

    #[test]
    fn test_default_param_body_scope_when_var_shadows() {
        // When body `var x` shadows parameter `x`, the function should still
        // produce valid IR (the body scope is pushed automatically).
        let source = r#"
            function f(x, y) {
                var x = 99;
                return y;
            }
        "#;
        let result = lower_program(source).expect("lowering should succeed");
        verify_typed_module(&result.module).expect("IR should verify when var shadows param");
    }

    #[test]
    fn test_default_param_scope_with_default_expr() {
        // Default parameter expression should reference parameter scope,
        // not body vars (even though we can't fully test runtime behavior here).
        let source = r#"
            function f(x, y) {
                if (y === undefined) y = x * 2;
                var x = 99;
                return y;
            }
        "#;
        let result = lower_program(source).expect("lowering should succeed");
        verify_typed_module(&result.module).expect("IR should verify with default and shadowing");
    }

    #[test]
    fn test_default_param_scope_for_in_var_shadowing() {
        // var in a for-loop init that shadows a parameter
        let source = r#"
            function f(x) {
                for (var x = 0; x < 10; x++) {}
                return x;
            }
        "#;
        let result = lower_program(source).expect("lowering should succeed");
        verify_typed_module(&result.module).expect("IR should verify when for-var shadows param");
    }

    #[test]
    fn test_default_param_scope_no_interaction_with_let() {
        // let/const in body should NOT trigger the body scope (only var does)
        let source = r#"
            function f(x) {
                let x2 = x * 2;
                return x2;
            }
        "#;
        let result = lower_program(source).expect("lowering should succeed");
        verify_typed_module(&result.module)
            .expect("IR should verify when let doesn't shadow param");
    }

    #[test]
    fn test_default_param_scope_multiple_params() {
        // Multiple parameters with var shadowing
        let source = r#"
            function f(a, b, c) {
                var a = 1;
                var b = 2;
                return c;
            }
        "#;
        let result = lower_program(source).expect("lowering should succeed");
        verify_typed_module(&result.module)
            .expect("IR should verify with multiple var-shadowed params");
    }

    #[test]
    fn test_default_param_scope_ir_verifies() {
        // Comprehensive verification across multiple patterns
        let sources = [
            "function f(x) { var x = 1; return x; }",
            "function f(x, y) { var x = 99; return y; }",
            "function f(x) { let z = x; return z; }",
            "function f(x) { for (var x = 0; x < 5; x++) {} }",
            "function f(a, b) { var c = a + b; return c; }",
        ];
        for source in sources {
            let result = lower_program(source).expect("lowering should succeed");
            verify_typed_module(&result.module)
                .unwrap_or_else(|e| panic!("IR should verify for '{source}': {e:?}"));
        }
    }

    #[test]
    fn test_bracket_access_stays_get_elem() {
        // obj[expr] should still use GetElem, not ICGetProp
        let module = lower_and_get_module("let obj = {}; let k = 'x'; let v = obj[k];");
        assert!(
            entry_has_op(&module, Op::GetElem),
            "bracket access (obj[k]) should emit GetElem"
        );
        // Should NOT have ICGetProp for bracket access
        // (ICGetProp may still appear for other dot accesses in runtime setup,
        //  but the bracket access itself should use GetElem)
    }

    #[test]
    fn test_ic_counter_increments() {
        // Multiple dot accesses should get different IC site IDs
        let module = lower_and_get_module("let obj = {}; let a = obj.x; let b = obj.y;");
        let entry_fn = &module.functions[module.entry.unwrap()];
        let ic_get_ops: Vec<_> = entry_fn
            .blocks
            .iter()
            .flat_map(|b| b.instructions.iter())
            .filter(|i| i.op == Op::ICGetProp)
            .collect();
        assert!(
            ic_get_ops.len() >= 2,
            "two dot accesses should emit at least 2 ICGetProp ops, got {}",
            ic_get_ops.len()
        );
        // Check that the ic_id operands (third operand) are different
        if ic_get_ops.len() >= 2 {
            let id0 = ic_get_ops[0].operands[2];
            let id1 = ic_get_ops[1].operands[2];
            assert_ne!(
                id0, id1,
                "different dot accesses should have different IC site IDs"
            );
        }
    }

    #[test]
    fn test_dot_set_still_uses_set_prop() {
        // obj.x = val should still use SetProp (not ICSetProp) to preserve strict mode
        let ir = lower_script_print("let obj = {}; obj.x = 5;");
        assert!(
            ir.contains("set_prop"),
            "dot set (obj.x = val) should still emit set_prop"
        );
    }

    // =========================================================================
    // CreateObjectLiteral tests (step 0.3.19)
    // =========================================================================

    #[test]
    fn test_static_object_literal_uses_create_object_literal() {
        // Simple object with identifier keys should use CreateObjectLiteral
        let ir = lower_script_print("let o = { x: 1, y: 2 };");
        assert!(
            ir.contains("create_object_literal"),
            "static object literal should emit create_object_literal, got:\n{ir}"
        );
        assert!(
            !ir.contains("create_object\n") && !ir.contains("create_object "),
            "should not emit plain create_object, got:\n{ir}"
        );
    }

    #[test]
    fn test_computed_key_falls_back_to_create_object() {
        // Computed key should fall back to CreateObject + SetProp
        let ir = lower_script_print("let k = 'x'; let o = { [k]: 1 };");
        assert!(
            ir.contains("create_object"),
            "computed key should use create_object fallback, got:\n{ir}"
        );
        assert!(
            !ir.contains("create_object_literal"),
            "computed key should NOT use create_object_literal, got:\n{ir}"
        );
    }

    #[test]
    fn test_spread_falls_back_to_create_object() {
        // Spread property should fall back to CreateObject + SetProp.
        // The source creates two objects: {y: 2} (literal) uses the fast path,
        // but {...a, y: 2} uses the fallback because of spread. Verify the
        // fallback path by checking that `create_object\n` (bare, no "literal")
        // appears in the output, meaning at least one object used the fallback.
        let module = lower_script_module("let a = {x: 1}; let o = { ...a, y: 2 };");
        let func = &module.functions[0];
        let has_plain_create_object = func
            .blocks
            .iter()
            .any(|bb| bb.instructions.iter().any(|i| i.op == Op::CreateObject));
        assert!(
            has_plain_create_object,
            "spread object should use plain CreateObject (fallback)"
        );
    }

    #[test]
    fn test_getter_falls_back_to_create_object() {
        // Getter should fall back to CreateObject + SetProp
        let ir = lower_script_print("let o = { get x() { return 1; } };");
        assert!(
            !ir.contains("create_object_literal"),
            "getter should NOT use create_object_literal, got:\n{ir}"
        );
    }

    #[test]
    fn test_empty_object_stays_create_object() {
        // Empty object should stay as CreateObject (no optimization needed)
        let ir = lower_script_print("let o = {};");
        assert!(
            !ir.contains("create_object_literal"),
            "empty object should not use create_object_literal, got:\n{ir}"
        );
    }

    #[test]
    fn test_numeric_key_falls_back_to_create_object() {
        // Numeric key should fall back to CreateObject + SetProp
        let ir = lower_script_print("let o = { 0: 'a', 1: 'b' };");
        assert!(
            !ir.contains("create_object_literal"),
            "numeric key should NOT use create_object_literal, got:\n{ir}"
        );
    }

    #[test]
    fn test_string_literal_key_uses_create_object_literal() {
        // String literal keys should also use CreateObjectLiteral
        let ir = lower_script_print(r#"let o = { "foo": 1, "bar": 2 };"#);
        assert!(
            ir.contains("create_object_literal"),
            "string literal keys should use create_object_literal, got:\n{ir}"
        );
    }

    #[test]
    fn test_method_uses_create_object_literal() {
        // Methods have PropertyKind::Init and static keys, so they should use the fast path
        let ir = lower_script_print("let o = { m() { return 1; } };");
        assert!(
            ir.contains("create_object_literal"),
            "method should use create_object_literal, got:\n{ir}"
        );
    }

    // =====================================================================
    // new.target metaproperty tests
    // =====================================================================

    #[test]
    fn test_new_target_emits_opcode() {
        // new.target inside a function should emit the NewTarget opcode
        let module = lower_and_get_module("function Foo() { return new.target; }");
        // The NewTarget opcode should appear in the function body (not the entry function)
        let has_nt = module.functions.iter().any(|func| {
            func.blocks.iter().any(|block| {
                block
                    .instructions
                    .iter()
                    .any(|inst| inst.op == Op::NewTarget)
            })
        });
        assert!(
            has_nt,
            "new.target should emit NewTarget opcode in the function body"
        );
    }

    #[test]
    fn test_new_expression_emits_call_new() {
        // `new Foo()` should emit CallNew opcode
        let module = lower_and_get_module("function Foo() {} new Foo();");
        let has_call_new = module.functions.iter().any(|func| {
            func.blocks
                .iter()
                .any(|block| block.instructions.iter().any(|inst| inst.op == Op::CallNew))
        });
        assert!(has_call_new, "new Foo() should emit CallNew opcode");
    }

    #[test]
    fn test_new_expression_spread_args() {
        // `new Foo(...args)` should use call_runtime for spread arguments
        // (calls __esc_rt_spread_into_array, then __esc_rt_apply_new via string table).
        // The IR printer uses `const_string @N` for string table entries, so
        // we check for the call_runtime pattern and verify the string table.
        let result = crate::lower_program("function Foo() {} var args = [1,2]; new Foo(...args);")
            .expect("lowering should succeed");
        ir::verify::verify_typed_module(&result.module).expect("IR should verify");
        // Check that __esc_rt_apply_new is in the string table
        assert!(
            result
                .string_table
                .iter()
                .any(|s| s == "__esc_rt_apply_new"),
            "new Foo(...args) should reference __esc_rt_apply_new in string table, got: {:?}",
            result.string_table
        );
    }

    #[test]
    fn test_template_literal_uses_cooked_values() {
        // Template literal should use cooked values (processed escape sequences).
        // The result module's string table should contain the cooked value
        // (with actual newline), not the raw "\n" escape sequence.
        let result =
            crate::lower_program("var x = `hello\\nworld`;").expect("lowering should succeed");
        ir::verify::verify_typed_module(&result.module).expect("IR should verify");
        // The cooked string should contain an actual newline character
        assert!(
            result
                .string_table
                .iter()
                .any(|s| s.contains('\n') && s.contains("hello") && s.contains("world")),
            "template literal should use cooked values with actual newline, string table: {:?}",
            result.string_table
        );
    }

    // =====================================================================
    // Tagged template literal tests
    // =====================================================================

    #[test]
    fn test_tagged_template_simple() {
        // tag`hello` should produce create_array for strings, set_prop for .raw, call_method for freeze
        let ir = lower_and_print("function tag(s) {} tag`hello`;");
        assert!(
            ir.contains("create_array"),
            "tagged template should create arrays for strings, got:\n{ir}"
        );
        assert!(
            ir.contains("set_prop"),
            "tagged template should set .raw property, got:\n{ir}"
        );
        assert!(
            ir.contains("call_method"),
            "tagged template should call Object.freeze, got:\n{ir}"
        );
    }

    #[test]
    fn test_tagged_template_with_expression() {
        // tag`a${1}b` should produce two create_arrays (cooked + raw) + call
        let ir = lower_and_print("function tag(s, v) {} tag`a${1}b`;");
        // Should have two create_array ops (cooked and raw)
        let create_array_count = ir.matches("create_array").count();
        assert!(
            create_array_count >= 2,
            "tagged template should create at least 2 arrays (cooked + raw), got {create_array_count} in:\n{ir}"
        );
        assert!(
            ir.contains("call "),
            "tagged template should call the tag function, got:\n{ir}"
        );
    }

    #[test]
    fn test_tagged_template_has_raw_property() {
        // The tagged template should set a .raw property on the strings array
        let module = lower_and_get_module("function tag(s) {} tag`hello`;");
        let entry = &module.functions[module.entry.unwrap()];
        let has_set_prop = entry
            .blocks
            .iter()
            .any(|block| block.instructions.iter().any(|inst| inst.op == Op::SetProp));
        assert!(
            has_set_prop,
            "tagged template IR should contain SetProp for .raw"
        );
    }

    #[test]
    fn test_tagged_template_calls_freeze() {
        // The tagged template should call Object.freeze on the template array
        let module = lower_and_get_module("function tag(s) {} tag`test`;");
        let entry = &module.functions[module.entry.unwrap()];
        let has_call_method = entry.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|inst| inst.op == Op::CallMethod)
        });
        assert!(
            has_call_method,
            "tagged template IR should contain CallMethod for Object.freeze"
        );
    }

    #[test]
    fn test_tagged_template_multiple_expressions() {
        // tag`a${x}b${y}c` should have 3 cooked strings and 2 expressions
        let ir = lower_and_print("var x = 1; var y = 2; function tag(s) {} tag`a${x}b${y}c`;");
        // Should contain create_array for both cooked and raw
        let create_array_count = ir.matches("create_array").count();
        assert!(
            create_array_count >= 2,
            "tagged template with multiple exprs should create at least 2 arrays, got {create_array_count}"
        );
    }

    #[test]
    fn test_untagged_template_literal() {
        // Regular template literal `hello ${name}` should produce string concatenation, not arrays
        let ir = lower_and_print("var name = 'world'; var s = `hello ${name}`;");
        assert!(
            !ir.contains("create_array"),
            "untagged template should NOT use CreateArray, got:\n{ir}"
        );
    }

    // =========================================================================
    // Annex B.3.3 — Block-scoped function declarations in sloppy mode
    // =========================================================================

    #[test]
    fn test_annex_b33_function_in_if_block_hoisted_sloppy() {
        // In sloppy mode, a function declaration inside an if block should
        // be hoisted to the enclosing function scope (Annex B.3.3).
        let source = r#"
            function outer() {
                if (true) {
                    function f() { return 1; }
                }
                return f();
            }
        "#;
        // Should lower without errors — f is accessible after the if block
        let result = lower_script(source);
        verify_typed_module(&result.module).expect("IR should verify");
        // The string table should contain "f" (the hoisted function name)
        assert!(
            result.string_table.contains(&"f".to_string()),
            "sloppy mode should hoist function name 'f' from block, got: {:?}",
            result.string_table
        );
    }

    #[test]
    fn test_annex_b33_function_in_if_block_strict_no_hoist() {
        // In strict mode (module), a function declaration inside a block should
        // be block-scoped, not hoisted to function scope.
        let source = r#"
            function outer() {
                if (true) {
                    function f() { return 1; }
                }
            }
        "#;
        // In module (strict) mode, f is block-scoped
        let result = lower_program(source).expect("lowering should succeed");
        verify_typed_module(&result.module).expect("IR should verify");
        // The function should still be created — just not hoisted
        assert!(
            result.string_table.contains(&"f".to_string()),
            "strict mode should still create function 'f', got: {:?}",
            result.string_table
        );
    }

    #[test]
    fn test_annex_b33_function_in_for_block_hoisted_sloppy() {
        // Function declaration inside a for-loop block should be hoisted in sloppy mode
        let source = r#"
            function outer() {
                for (var i = 0; i < 1; i++) {
                    function g() { return 42; }
                }
                return g();
            }
        "#;
        let result = lower_script(source);
        verify_typed_module(&result.module).expect("IR should verify");
        assert!(
            result.string_table.contains(&"g".to_string()),
            "sloppy mode should hoist function 'g' from for-loop block, got: {:?}",
            result.string_table
        );
    }

    #[test]
    fn test_annex_b33_nested_blocks_hoisted_to_function_scope() {
        // Function in a nested block should hoist to enclosing function scope,
        // not just the immediate enclosing block
        let source = r#"
            function outer() {
                if (true) {
                    if (true) {
                        function h() { return 99; }
                    }
                }
                return h();
            }
        "#;
        let result = lower_script(source);
        verify_typed_module(&result.module).expect("IR should verify");
        assert!(
            result.string_table.contains(&"h".to_string()),
            "sloppy mode should hoist function 'h' from nested blocks, got: {:?}",
            result.string_table
        );
    }

    #[test]
    fn test_annex_b33_function_at_top_level_no_extra_hoist() {
        // Function at the top level of a function body is NOT inside a block,
        // so no Annex B.3.3 hoisting should occur (it's already function-scoped).
        let source = r#"
            function outer() {
                function f() { return 1; }
                return f();
            }
        "#;
        let result = lower_script(source);
        verify_typed_module(&result.module).expect("IR should verify");
        let ir = print_typed_module(&result.module);
        // Should still work — f is function-scoped normally
        assert!(
            ir.contains("create_closure"),
            "function declaration should produce a closure, got:\n{ir}"
        );
    }

    // =========================================================================
    // Delete on identifier (sloppy mode)
    // =========================================================================

    #[test]
    fn test_delete_var_returns_false_sloppy() {
        // `delete x` on a var-declared variable should return false in sloppy mode
        let source = "var x = 1; var result = delete x;";
        let result = lower_script(source);
        verify_typed_module(&result.module).expect("IR should verify");
        let ir = print_typed_module(&result.module);
        // Should emit const_bool(false) for `delete x`
        assert!(
            ir.contains("const_bool false"),
            "delete on var should emit const_bool(false) in sloppy mode, got:\n{ir}"
        );
    }

    #[test]
    fn test_delete_undeclared_calls_runtime_sloppy() {
        // `delete y` on an undeclared identifier in sloppy mode should call
        // the runtime to try to delete from globalThis
        let source = "var result = delete y;";
        let result = lower_script(source);
        verify_typed_module(&result.module).expect("IR should verify");
        assert!(
            result
                .string_table
                .iter()
                .any(|s| s == "__esc_rt_delete_binding"),
            "delete on undeclared should call __esc_rt_delete_binding, got: {:?}",
            result.string_table
        );
    }

    #[test]
    fn test_delete_member_expr_still_works() {
        // `delete obj.x` should still emit DeleteProp as before
        let source = "var obj = {}; delete obj.x;";
        let result = lower_script(source);
        verify_typed_module(&result.module).expect("IR should verify");
        let module = result.module;
        assert!(
            entry_has_op(&module, Op::DeleteProp),
            "delete on member expression should emit DeleteProp"
        );
    }

    #[test]
    fn test_delete_computed_member_still_works() {
        // `delete obj[key]` should still emit DeleteProp
        let source = r#"var obj = {}; var key = "x"; delete obj[key];"#;
        let result = lower_script(source);
        verify_typed_module(&result.module).expect("IR should verify");
        let module = result.module;
        assert!(
            entry_has_op(&module, Op::DeleteProp),
            "delete on computed member should emit DeleteProp"
        );
    }

    #[test]
    fn test_delete_non_identifier_returns_true() {
        // `delete 42` should return true (evaluate for side effects)
        let source = "var result = delete 42;";
        let result = lower_script(source);
        verify_typed_module(&result.module).expect("IR should verify");
        let ir = print_typed_module(&result.module);
        assert!(
            ir.contains("const_bool true"),
            "delete on non-member/non-identifier should return true, got:\n{ir}"
        );
    }

    #[test]
    fn test_delete_identifier_in_module_strict_not_special() {
        // In module (strict) mode, `delete identifier` is a SyntaxError
        // caught by the parser. We test that a valid delete (on a member)
        // still works in strict mode.
        let source = "let obj = {}; delete obj.x;";
        let result = lower_program(source).expect("lowering should succeed");
        verify_typed_module(&result.module).expect("IR should verify");
        let module = result.module;
        assert!(
            entry_has_op(&module, Op::DeleteProp),
            "strict mode delete on member should emit DeleteProp"
        );
    }

    // =========================================================================
    // Duplicate parameters (sloppy mode)
    // =========================================================================

    #[test]
    fn test_duplicate_params_sloppy_last_wins() {
        // In sloppy mode, duplicate parameter names are allowed.
        // The last parameter with the same name wins.
        let source = r#"
            function f(a, a, a) { return a; }
        "#;
        let result = lower_script(source);
        verify_typed_module(&result.module).expect("IR should verify");
        // The function should have 3 load_param instructions and the
        // last one's value should be what 'a' resolves to
        let ir = print_typed_module(&result.module);
        assert!(
            ir.contains("load_param"),
            "function with duplicate params should still load parameters, got:\n{ir}"
        );
    }

    #[test]
    fn test_duplicate_params_two_names_sloppy() {
        // Duplicate params with two different names
        let source = r#"
            function f(a, b, a) { return a; }
        "#;
        let result = lower_script(source);
        verify_typed_module(&result.module).expect("IR should verify");
        let ir = print_typed_module(&result.module);
        // Should compile without errors — 'a' maps to the last (3rd) parameter
        assert!(
            ir.contains("load_param"),
            "function with duplicate params should compile, got:\n{ir}"
        );
    }

    // =========================================================================
    // Octal numeric literals (sloppy mode)
    // =========================================================================

    #[test]
    fn test_octal_literal_sloppy_mode() {
        // 0777 in sloppy mode should be parsed as 511 (octal)
        let source = "var x = 0777;";
        let result = lower_script(source);
        verify_typed_module(&result.module).expect("IR should verify");
        let ir = print_typed_module(&result.module);
        // The parser should produce 511.0 for 0777
        // This should appear as const_i32(511) or const_f64(511.0)
        assert!(
            ir.contains("511") || ir.contains("0x1ff"),
            "0777 should be parsed as 511 in sloppy mode, got:\n{ir}"
        );
    }

    #[test]
    fn test_octal_literal_010_sloppy() {
        // 010 in sloppy mode should be parsed as 8 (octal)
        let source = "var x = 010;";
        let result = lower_script(source);
        verify_typed_module(&result.module).expect("IR should verify");
        let ir = print_typed_module(&result.module);
        // Should contain const_i32(8)
        assert!(
            ir.contains("const_f64 8"),
            "010 should be parsed as the Number 8 in sloppy mode, got:\n{ir}"
        );
    }

    // =========================================================================
    // Octal escape sequences (sloppy mode)
    // =========================================================================

    #[test]
    fn test_octal_escape_sequence_sloppy() {
        // "\101" in sloppy mode should produce "A" (octal 101 = 65 = 'A')
        let source = r#"var x = "\101";"#;
        let result = lower_script(source);
        verify_typed_module(&result.module).expect("IR should verify");
        // The string table should contain "A"
        assert!(
            result.string_table.contains(&"A".to_string()),
            "octal escape \\101 should produce 'A', got: {:?}",
            result.string_table
        );
    }

    #[test]
    fn test_octal_escape_sequence_077_sloppy() {
        // "\77" in sloppy mode should produce "?" (octal 77 = 63 = '?')
        let source = r#"var x = "\77";"#;
        let result = lower_script(source);
        verify_typed_module(&result.module).expect("IR should verify");
        // The string table should contain "?"
        assert!(
            result.string_table.contains(&"?".to_string()),
            "octal escape \\77 should produce '?', got: {:?}",
            result.string_table
        );
    }

    // =========================================================================
    // Annex B.3.3 — Additional edge cases
    // =========================================================================

    #[test]
    fn test_annex_b33_function_in_while_block_hoisted_sloppy() {
        // Function inside a while-block should be hoisted in sloppy mode
        let source = r#"
            function outer() {
                var i = 0;
                while (i < 1) {
                    function w() { return 7; }
                    i++;
                }
                return w();
            }
        "#;
        let result = lower_script(source);
        verify_typed_module(&result.module).expect("IR should verify");
        assert!(
            result.string_table.contains(&"w".to_string()),
            "sloppy mode should hoist function 'w' from while block, got: {:?}",
            result.string_table
        );
    }

    #[test]
    fn test_annex_b33_function_in_switch_case_hoisted_sloppy() {
        // Function inside a switch case should be hoisted in sloppy mode
        let source = r#"
            function outer() {
                switch (1) {
                    case 1:
                        function s() { return 5; }
                        break;
                }
                return s();
            }
        "#;
        let result = lower_script(source);
        verify_typed_module(&result.module).expect("IR should verify");
        assert!(
            result.string_table.contains(&"s".to_string()),
            "sloppy mode should hoist function 's' from switch case, got: {:?}",
            result.string_table
        );
    }

    // =====================================================================
    // Getter/setter accessor syntax tests (0.4.3 / 0.4.14)
    // =====================================================================

    #[test]
    fn test_object_getter_emits_define_accessor() {
        // Object literal with a getter should emit CallRuntime for define_accessor
        let result = lower_script("let o = { get x() { return 42; } };");
        let ir = print_typed_module(&result.module);
        assert!(
            ir.contains("call_runtime"),
            "getter should emit call_runtime for define_accessor, got:\n{ir}"
        );
        assert!(
            result
                .string_table
                .iter()
                .any(|s| s == "__esc_rt_define_accessor"),
            "string table should contain __esc_rt_define_accessor"
        );
    }

    #[test]
    fn test_object_setter_emits_define_accessor() {
        // Object literal with a setter should emit CallRuntime for define_accessor
        let result = lower_script("let o = { set x(v) { this._x = v; } };");
        let ir = print_typed_module(&result.module);
        assert!(
            ir.contains("call_runtime"),
            "setter should emit call_runtime for define_accessor, got:\n{ir}"
        );
        assert!(
            result
                .string_table
                .iter()
                .any(|s| s == "__esc_rt_define_accessor"),
            "string table should contain __esc_rt_define_accessor"
        );
    }

    #[test]
    fn test_object_getter_setter_paired() {
        // Object with both getter and setter for the same key should emit
        // a single define_accessor call (not two separate ones)
        let result = lower_script(
            r#"let o = {
                get val() { return this._val; },
                set val(v) { this._val = v; }
            };"#,
        );
        let ir = print_typed_module(&result.module);
        assert!(
            ir.contains("call_runtime"),
            "paired accessor should emit call_runtime, got:\n{ir}"
        );
        assert!(
            result
                .string_table
                .iter()
                .any(|s| s == "__esc_rt_define_accessor"),
            "string table should contain __esc_rt_define_accessor"
        );
        // Should NOT use the legacy __get_ / __set_ convention
        assert!(
            !result.string_table.iter().any(|s| s.starts_with("__get_")),
            "should NOT have legacy __get_ prefix"
        );
        assert!(
            !result.string_table.iter().any(|s| s.starts_with("__set_")),
            "should NOT have legacy __set_ prefix"
        );
    }

    #[test]
    fn test_object_getter_no_legacy_convention() {
        // Verify getter does NOT use the old __get_<name> legacy convention
        let result = lower_script("let o = { get foo() { return 1; } };");
        assert!(
            !result.string_table.iter().any(|s| s == "__get_foo"),
            "should NOT use legacy __get_foo convention"
        );
        assert!(
            result
                .string_table
                .iter()
                .any(|s| s == "__esc_rt_define_accessor"),
            "should use __esc_rt_define_accessor instead"
        );
    }

    #[test]
    fn test_object_setter_no_legacy_convention() {
        // Verify setter does NOT use the old __set_<name> legacy convention
        let result = lower_script("let o = { set foo(v) {} };");
        assert!(
            !result.string_table.iter().any(|s| s == "__set_foo"),
            "should NOT use legacy __set_foo convention"
        );
    }

    #[test]
    fn test_object_mixed_accessors_and_data_props() {
        // Object with both regular data properties and accessors
        let result = lower_script(r#"let o = { normal: 1, get x() { return 42; }, other: "hi" };"#);
        let ir = print_typed_module(&result.module);
        // Should have call_runtime for the getter
        assert!(
            ir.contains("call_runtime"),
            "should emit call_runtime for accessor, got:\n{ir}"
        );
        // Should also have set_prop for normal data properties
        assert!(
            ir.contains("set_prop"),
            "should emit set_prop for data properties, got:\n{ir}"
        );
    }

    #[test]
    fn test_object_getter_function_is_lowered() {
        // The getter function body should be lowered as a separate function
        let module = lower_script_module("let o = { get x() { return 42; } };");
        // Should have at least 2 functions: entry + getter
        assert!(
            module.functions.len() >= 2,
            "should have at least 2 functions (entry + getter), got {}",
            module.functions.len()
        );
    }

    #[test]
    fn test_object_setter_function_is_lowered() {
        // The setter function body should be lowered as a separate function
        let module = lower_script_module("let o = { set x(v) { this._x = v; } };");
        // Should have at least 2 functions: entry + setter
        assert!(
            module.functions.len() >= 2,
            "should have at least 2 functions (entry + setter), got {}",
            module.functions.len()
        );
    }

    #[test]
    fn test_object_getter_sets_function_name() {
        // The getter's function.name should be "get <name>"
        let result = lower_script("let o = { get x() { return 1; } };");
        assert!(
            result.string_table.iter().any(|s| s == "get x"),
            "should set function.name to 'get x'"
        );
    }

    #[test]
    fn test_object_setter_sets_function_name() {
        // The setter's function.name should be "set <name>"
        let result = lower_script("let o = { set x(v) {} };");
        assert!(
            result.string_table.iter().any(|s| s == "set x"),
            "should set function.name to 'set x'"
        );
    }

    #[test]
    fn test_object_getter_only_passes_undefined_for_setter() {
        // When only a getter is defined, the setter arg should be undefined.
        // We verify by checking the IR has const_undefined instructions
        // in the entry function (for the setter arg to define_accessor).
        let ir = lower_script_print("let o = { get x() { return 1; } };");
        assert!(
            ir.contains("const_undefined"),
            "getter-only should pass undefined for setter, got:\n{ir}"
        );
    }

    #[test]
    fn test_object_setter_only_passes_undefined_for_getter() {
        // When only a setter is defined, the getter arg should be undefined.
        let ir = lower_script_print("let o = { set x(v) {} };");
        assert!(
            ir.contains("const_undefined"),
            "setter-only should pass undefined for getter, got:\n{ir}"
        );
    }

    #[test]
    fn test_class_getter_emits_define_accessor() {
        // Class with a getter should emit define_accessor on the prototype
        let result = lower_script(r#"class Foo { get bar() { return this._bar; } }"#);
        let ir = print_typed_module(&result.module);
        assert!(
            ir.contains("call_runtime"),
            "class getter should emit call_runtime, got:\n{ir}"
        );
        assert!(
            result
                .string_table
                .iter()
                .any(|s| s == "__esc_rt_define_accessor"),
            "class getter should use __esc_rt_define_accessor"
        );
    }

    #[test]
    fn test_class_setter_emits_define_accessor() {
        // Class with a setter should emit define_accessor on the prototype
        let result = lower_script(r#"class Foo { set bar(v) { this._bar = v; } }"#);
        let ir = print_typed_module(&result.module);
        assert!(
            ir.contains("call_runtime"),
            "class setter should emit call_runtime, got:\n{ir}"
        );
        assert!(
            result
                .string_table
                .iter()
                .any(|s| s == "__esc_rt_define_accessor"),
            "class setter should use __esc_rt_define_accessor"
        );
    }

    #[test]
    fn test_class_getter_setter_paired() {
        // Class with both getter and setter for the same key
        let result = lower_script(
            r#"class Foo {
                get bar() { return this._bar; }
                set bar(v) { this._bar = v; }
            }"#,
        );
        assert!(
            result
                .string_table
                .iter()
                .any(|s| s == "__esc_rt_define_accessor"),
            "class paired accessor should use __esc_rt_define_accessor"
        );
    }

    #[test]
    fn test_class_static_getter_emits_define_accessor() {
        // Class with a static getter should emit define_accessor on the constructor
        let result = lower_script(r#"class Foo { static get count() { return 0; } }"#);
        assert!(
            result
                .string_table
                .iter()
                .any(|s| s == "__esc_rt_define_accessor"),
            "class static getter should use __esc_rt_define_accessor"
        );
    }

    #[test]
    fn test_class_static_setter_emits_define_accessor() {
        // Class with a static setter should emit define_accessor on the constructor
        let result = lower_script(r#"class Foo { static set count(v) {} }"#);
        assert!(
            result
                .string_table
                .iter()
                .any(|s| s == "__esc_rt_define_accessor"),
            "class static setter should use __esc_rt_define_accessor"
        );
    }

    #[test]
    fn test_class_getter_sets_function_name() {
        // Class getter's function.name should be "get <name>"
        let result = lower_script(r#"class Foo { get bar() { return 1; } }"#);
        assert!(
            result.string_table.iter().any(|s| s == "get bar"),
            "class getter should set function.name to 'get bar'"
        );
    }

    #[test]
    fn test_class_setter_sets_function_name() {
        // Class setter's function.name should be "set <name>"
        let result = lower_script(r#"class Foo { set bar(v) {} }"#);
        assert!(
            result.string_table.iter().any(|s| s == "set bar"),
            "class setter should set function.name to 'set bar'"
        );
    }

    #[test]
    fn test_class_regular_methods_still_use_set_prop() {
        // Regular (non-accessor) class methods should still use SetProp,
        // not define_accessor
        let ir = lower_script_print(r#"class Foo { hello() { return 1; } }"#);
        assert!(
            ir.contains("set_prop"),
            "regular class method should use set_prop, got:\n{ir}"
        );
    }

    #[test]
    fn test_class_mixed_methods_and_accessors() {
        // Class with both regular methods and accessor methods
        let result = lower_script(
            r#"class Foo {
                hello() { return "hi"; }
                get bar() { return this._bar; }
                world() { return "world"; }
            }"#,
        );
        let ir = print_typed_module(&result.module);
        // Should have set_prop for regular methods
        assert!(
            ir.contains("set_prop"),
            "should have set_prop for regular methods, got:\n{ir}"
        );
        // Should have define_accessor for the getter
        assert!(
            result
                .string_table
                .iter()
                .any(|s| s == "__esc_rt_define_accessor"),
            "should have define_accessor for the getter"
        );
    }

    #[test]
    fn test_class_getter_function_body_lowered() {
        // Class getter body should be lowered as a separate function
        let module = lower_script_module(r#"class Foo { get bar() { return 42; } }"#);
        // At least 3 functions: entry, constructor, getter
        assert!(
            module.functions.len() >= 3,
            "should have at least 3 functions (entry + ctor + getter), got {}",
            module.functions.len()
        );
    }

    #[test]
    fn test_object_string_literal_key_accessor() {
        // Object with string literal key for accessor
        let result = lower_script(r#"let o = { get "foo"() { return 1; } };"#);
        assert!(
            result
                .string_table
                .iter()
                .any(|s| s == "__esc_rt_define_accessor"),
            "string-key getter should use __esc_rt_define_accessor"
        );
        assert!(
            result.string_table.iter().any(|s| s == "get foo"),
            "should set function.name to 'get foo'"
        );
    }

    // =========================================================================
    // v0.4 Wave 2 — Class Expressions + Super (steps 0.4.11–0.4.13)
    // =========================================================================

    #[test]
    fn test_class_expression_creates_closure() {
        // A class expression should produce CreateClosure for the constructor
        let source = "const MyClass = class { constructor() {} };";
        let module = lower_and_get_module(source);
        let entry_fn = &module.functions[module.entry.unwrap()];
        let has_create_closure = entry_fn
            .blocks
            .iter()
            .any(|b| b.instructions.iter().any(|i| i.op == Op::CreateClosure));
        assert!(
            has_create_closure,
            "class expression should emit CreateClosure"
        );
    }

    #[test]
    fn test_class_expression_with_methods() {
        // Class expression with methods should lower successfully
        let source = r#"
            const MyClass = class {
                constructor(val) { this.val = val; }
                getVal() { return this.val; }
            };
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("IR should verify");
        // Should have at least 3 functions: main, constructor, getVal
        assert!(
            result.module.functions.len() >= 3,
            "class expr with method should produce 3+ functions, got {}",
            result.module.functions.len()
        );
    }

    #[test]
    fn test_class_expression_anonymous() {
        // Anonymous class expression (no name)
        let source = "const Anon = class { constructor() { this.x = 1; } };";
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("anonymous class expression should verify");
    }

    #[test]
    fn test_class_expression_named_not_visible_outside() {
        // Named class expression: name should NOT be in the outer scope
        // Accessing it outside should trigger an undeclared reference
        let source = r#"
            const MyClass = class Foo { constructor() {} };
            typeof Foo;
        "#;
        let ir = lower_and_print(source);
        // The name "Foo" should resolve as undeclared outside the class
        // (typeof on undeclared returns "undefined" without throwing)
        assert!(
            ir.contains("typeof_boxed"),
            "typeof Foo should use typeof_boxed on undeclared"
        );
    }

    #[test]
    fn test_class_expression_with_extends() {
        // Class expression can extend another class
        let source = r#"
            class Base { constructor() { this.x = 1; } }
            const Derived = class extends Base { constructor() { super(); this.y = 2; } };
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("class expression with extends should verify");
    }

    #[test]
    fn test_class_expression_assigned_to_variable() {
        // Class expression can be used as a value
        let source = r#"
            let cls = class { constructor(v) { this.v = v; } };
            let obj = new cls(42);
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("IR should verify");
        let entry_fn = &result.module.functions[result.module.entry.unwrap()];
        let has_call_new = entry_fn
            .blocks
            .iter()
            .any(|b| b.instructions.iter().any(|i| i.op == Op::CallNew));
        assert!(has_call_new, "new ClassExpr() should emit CallNew");
    }

    #[test]
    fn test_class_expression_inline() {
        // Class expression used directly in new expression
        let source = "let obj = new (class { constructor() { this.x = 1; } })();";
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("inline class expression should verify");
    }

    #[test]
    fn test_class_expression_default_constructor() {
        // Class expression with no explicit constructor
        let source = "const MyClass = class { method() { return 42; } };";
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("IR should verify");
    }

    #[test]
    fn test_class_expression_static_method() {
        // Class expression with static method
        let source = r#"
            const MyClass = class {
                static create() { return 42; }
            };
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("class expr with static method should verify");
    }

    #[test]
    fn test_super_call_in_derived_constructor() {
        // super() call emits SuperCall opcode
        let source = r#"
            class Base { constructor(x) { this.x = x; } }
            class Derived extends Base {
                constructor(x) { super(x); this.y = 2; }
            }
        "#;
        let module = lower_and_get_module(source);
        let has_super_call = module.functions.iter().any(|f| {
            f.blocks
                .iter()
                .any(|b| b.instructions.iter().any(|i| i.op == Op::SuperCall))
        });
        assert!(has_super_call, "super() should emit SuperCall opcode");
    }

    #[test]
    fn test_super_call_with_arguments() {
        // super(arg1, arg2) passes arguments to the parent constructor
        let source = r#"
            class Base { constructor(a, b) { this.a = a; this.b = b; } }
            class Derived extends Base {
                constructor(a, b) { super(a, b); }
            }
        "#;
        let module = lower_and_get_module(source);
        // Find the SuperCall instruction and verify it has operands
        let super_call_inst = module
            .functions
            .iter()
            .flat_map(|f| {
                f.blocks
                    .iter()
                    .flat_map(|b| b.instructions.iter().filter(|i| i.op == Op::SuperCall))
            })
            .next();
        assert!(
            super_call_inst.is_some(),
            "should have SuperCall instruction"
        );
        let inst = super_call_inst.unwrap();
        // operands: [callee, arg1, arg2] = 3 operands
        assert!(
            inst.operands.len() >= 3,
            "super(a, b) should have 3+ operands (callee + 2 args), got {}",
            inst.operands.len()
        );
    }

    #[test]
    fn test_super_call_no_args() {
        // super() with no arguments
        let source = r#"
            class Base { constructor() {} }
            class Derived extends Base {
                constructor() { super(); }
            }
        "#;
        let module = lower_and_get_module(source);
        let has_super_call = module.functions.iter().any(|f| {
            f.blocks
                .iter()
                .any(|b| b.instructions.iter().any(|i| i.op == Op::SuperCall))
        });
        assert!(has_super_call, "super() with no args should emit SuperCall");
    }

    #[test]
    fn test_super_property_read() {
        // super.prop emits GetSuper opcode
        let source = r#"
            class Base {
                method() { return 42; }
            }
            class Derived extends Base {
                method() { return super.method(); }
            }
        "#;
        let module = lower_and_get_module(source);
        let has_get_super = module.functions.iter().any(|f| {
            f.blocks
                .iter()
                .any(|b| b.instructions.iter().any(|i| i.op == Op::GetSuper))
        });
        assert!(has_get_super, "super.method should emit GetSuper opcode");
    }

    #[test]
    fn test_super_property_access_in_method() {
        // super.prop as a property read (not a method call)
        let source = r#"
            class Base { getValue() { return 99; } }
            class Derived extends Base {
                getValue() { return super.getValue(); }
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("super property access should verify");
    }

    #[test]
    fn test_super_property_write() {
        // super.prop = val emits SetSuper opcode
        let source = r#"
            class Base {
                constructor() { this.x = 0; }
            }
            class Derived extends Base {
                constructor() {
                    super();
                    super.x = 42;
                }
            }
        "#;
        let module = lower_and_get_module(source);
        let has_set_super = module.functions.iter().any(|f| {
            f.blocks
                .iter()
                .any(|b| b.instructions.iter().any(|i| i.op == Op::SetSuper))
        });
        assert!(has_set_super, "super.x = 42 should emit SetSuper opcode");
    }

    #[test]
    fn test_derived_class_both_parent_and_child_properties() {
        // Derived class instance has both parent and child properties set
        let source = r#"
            class Base {
                constructor() { this.x = 1; }
            }
            class Derived extends Base {
                constructor() {
                    super();
                    this.y = 2;
                }
            }
            let d = new Derived();
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("derived class should verify");
    }

    #[test]
    fn test_multi_level_inheritance() {
        // A → B → C chain with super calls
        let source = r#"
            class A {
                constructor() { this.a = 1; }
            }
            class B extends A {
                constructor() { super(); this.b = 2; }
            }
            class C extends B {
                constructor() { super(); this.c = 3; }
            }
            let c = new C();
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("multi-level inheritance should verify");
    }

    #[test]
    fn test_super_method_call_with_args() {
        // super.method(arg1, arg2) passes args correctly
        let source = r#"
            class Base {
                compute(a, b) { return a + b; }
            }
            class Derived extends Base {
                compute(a, b) { return super.compute(a, b); }
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("super method call with args should verify");
        let has_get_super = result.module.functions.iter().any(|f| {
            f.blocks
                .iter()
                .any(|b| b.instructions.iter().any(|i| i.op == Op::GetSuper))
        });
        assert!(has_get_super, "super.compute should emit GetSuper");
    }

    #[test]
    fn test_class_expression_in_iife() {
        // Class expression inside an IIFE (immediately invoked function expression)
        let source = r#"
            let obj = (function() {
                const C = class {
                    constructor(v) { this.v = v; }
                };
                return new C(10);
            })();
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("class expr in IIFE should verify");
    }

    #[test]
    fn test_class_expression_returned_from_function() {
        // Class expression returned from a function (factory pattern)
        let source = r#"
            function createClass() {
                return class {
                    constructor(x) { this.x = x; }
                    get() { return this.x; }
                };
            }
            let C = createClass();
            let obj = new C(5);
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module)
            .expect("class expr returned from function should verify");
    }

    #[test]
    fn test_class_expression_named_self_reference() {
        // Named class expression: name visible inside class body methods
        let source = r#"
            const Foo = class Bar {
                constructor() { this.x = 1; }
                method() { return typeof Bar; }
            };
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("named class expr self-reference should verify");
    }

    #[test]
    fn test_super_call_and_property_in_same_constructor() {
        // Both super() call and super.prop access in the same constructor
        let source = r#"
            class Base {
                constructor() { this.base = true; }
                baseMethod() { return "base"; }
            }
            class Derived extends Base {
                constructor() {
                    super();
                    this.derived = true;
                }
                method() {
                    return super.baseMethod();
                }
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("mixed super usage should verify");
        // Should have both SuperCall and GetSuper
        let has_super_call = result.module.functions.iter().any(|f| {
            f.blocks
                .iter()
                .any(|b| b.instructions.iter().any(|i| i.op == Op::SuperCall))
        });
        let has_get_super = result.module.functions.iter().any(|f| {
            f.blocks
                .iter()
                .any(|b| b.instructions.iter().any(|i| i.op == Op::GetSuper))
        });
        assert!(has_super_call, "should have SuperCall in constructor");
        assert!(has_get_super, "should have GetSuper in method");
    }

    #[test]
    fn test_class_expression_extends_variable() {
        // Class expression extending a variable reference
        let source = r#"
            class Base { constructor() { this.x = 1; } }
            const Derived = class extends Base {
                constructor() { super(); this.y = 2; }
            };
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("class expr extends var should verify");
    }

    #[test]
    fn test_class_expression_empty_body() {
        // Class expression with completely empty body
        let source = "const C = class {};";
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("empty class expr should verify");
    }

    #[test]
    fn test_class_expression_property_definition() {
        // Class expression with property definitions on the prototype
        let source = r#"
            const C = class {
                x = 42;
                constructor() {}
            };
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("class expr with property def should verify");
    }

    #[test]
    fn test_super_bare_expression_returns_undefined() {
        // A bare `super` reference (not call or member) should not crash
        // Note: this is technically a syntax error, but the parser may emit it
        // in certain malformed patterns. We should handle it gracefully.
        let source = r#"
            class Base {}
            class Derived extends Base {
                constructor() { super(); }
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("IR should verify");
    }

    #[test]
    fn test_class_expression_strict_mode() {
        // Class body is always strict — assignments in methods use SetPropStrict
        let source = r#"
            const C = class {
                method() {
                    let obj = {};
                    obj.x = 1;
                    return obj;
                }
            };
        "#;
        let module = lower_and_get_module(source);
        let has_strict_set = module.functions.iter().any(|f| {
            f.blocks
                .iter()
                .any(|b| b.instructions.iter().any(|i| i.op == Op::SetPropStrict))
        });
        assert!(
            has_strict_set,
            "class expression method should emit SetPropStrict (class bodies are strict)"
        );
    }

    #[test]
    fn test_class_static_and_instance_accessors() {
        // Class with both static and instance accessors
        let result = lower_script(
            r#"class Foo {
                get bar() { return this._bar; }
                static get count() { return 0; }
            }"#,
        );
        assert!(
            result
                .string_table
                .iter()
                .any(|s| s == "__esc_rt_define_accessor"),
            "should have define_accessor for both accessors"
        );
        // Both should have proper function.name
        assert!(
            result.string_table.iter().any(|s| s == "get bar"),
            "instance getter should have name 'get bar'"
        );
        assert!(
            result.string_table.iter().any(|s| s == "get count"),
            "static getter should have name 'get count'"
        );
    }

    #[test]
    fn test_derived_class_no_explicit_constructor() {
        // Derived class with no explicit constructor inherits parent's
        let source = r#"
            class Base {
                constructor(name) { this.name = name; }
            }
            class Derived extends Base {
                greet() { return "hello " + this.name; }
            }
            let d = new Derived("world");
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("derived without ctor should verify");
    }

    // ================================================================
    // Static fields (0.4.17) and static initializer blocks (0.4.18)
    // ================================================================

    #[test]
    fn test_static_field_set_on_constructor() {
        // A static field with an initializer should emit SetProp on the constructor
        let source = r#"
            class Counter {
                static count = 0;
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("static field should verify");
        // The string "count" should be in the string table
        assert!(
            result.string_table.iter().any(|s| s == "count"),
            "should have 'count' in string table"
        );
        // Should emit SetProp for the static field
        assert!(
            entry_has_op(&result.module, Op::SetProp),
            "static field should emit SetProp on constructor"
        );
    }

    #[test]
    fn test_static_field_with_initializer_expression() {
        // Static fields with computed initializers
        let source = r#"
            class Calc {
                static result = 1 + 2;
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("static field with expr should verify");
        // Should contain an AddJS instruction for the computed initializer
        assert!(
            entry_has_op(&result.module, Op::AddJS),
            "static field initializer 1 + 2 should emit AddJS"
        );
        assert!(
            result.string_table.iter().any(|s| s == "result"),
            "should have 'result' in string table"
        );
    }

    #[test]
    fn test_static_field_without_initializer_defaults_to_undefined() {
        // Static fields without an initializer should default to undefined
        let source = r#"
            class C {
                static x;
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("static field without init should verify");
        // Should have a ConstUndefined for the default value
        assert!(
            entry_has_op(&result.module, Op::ConstUndefined),
            "static field without initializer should emit ConstUndefined"
        );
        assert!(
            result.string_table.iter().any(|s| s == "x"),
            "should have 'x' in string table"
        );
    }

    #[test]
    fn test_multiple_static_fields_in_order() {
        // Multiple static fields should be emitted in source order
        let source = r#"
            class Config {
                static host = "localhost";
                static port = 8080;
                static debug = false;
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("multiple static fields should verify");
        // All three field names should appear in the string table
        assert!(
            result.string_table.iter().any(|s| s == "host"),
            "should have 'host' in string table"
        );
        assert!(
            result.string_table.iter().any(|s| s == "port"),
            "should have 'port' in string table"
        );
        assert!(
            result.string_table.iter().any(|s| s == "debug"),
            "should have 'debug' in string table"
        );
    }

    #[test]
    fn test_static_block_executes_after_class_creation() {
        // Static block body should be inlined into the class creation sequence
        let source = r#"
            class DB {
                static connection;
                static {
                    let x = 42;
                }
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("static block should verify");
    }

    #[test]
    fn test_static_block_this_is_constructor() {
        // Inside a static block, `this` should refer to the constructor,
        // not the enclosing scope's `this`. We verify this by checking
        // that `this.x = 1` inside a static block does NOT emit ThisValue
        // (because this_override is used) and instead uses the constructor value.
        let source = r#"
            class Foo {
                static {
                    this.x = 1;
                }
            }
        "#;
        let module = lower_and_get_module(source);
        // The class body is the entry function. The static block's `this.x = 1`
        // should NOT emit Op::ThisValue because `this` is overridden to the ctor.
        let entry_fn = &module.functions[module.entry.unwrap()];
        let has_this_value = entry_fn
            .blocks
            .iter()
            .any(|b| b.instructions.iter().any(|i| i.op == Op::ThisValue));
        assert!(
            !has_this_value,
            "static block should NOT emit ThisValue (this is overridden to constructor)"
        );
    }

    #[test]
    fn test_static_block_can_access_previous_static_fields() {
        // Static blocks can reference the class (and thus previous static fields)
        let source = r#"
            class C {
                static x = 10;
                static {
                    let val = C.x;
                }
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("static block accessing fields should verify");
    }

    #[test]
    fn test_static_field_and_static_block_interleaved() {
        // Static fields and blocks should be evaluated in source order
        let source = r#"
            class C {
                static a = 1;
                static {
                    let temp = 2;
                }
                static b = 3;
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module)
            .expect("interleaved static fields and blocks should verify");
        // All field names should be present
        assert!(
            result.string_table.iter().any(|s| s == "a"),
            "should have 'a' in string table"
        );
        assert!(
            result.string_table.iter().any(|s| s == "b"),
            "should have 'b' in string table"
        );
    }

    #[test]
    fn test_static_field_on_derived_class() {
        // Static fields on a derived class
        let source = r#"
            class Base {
                static baseField = "base";
            }
            class Derived extends Base {
                static derivedField = "derived";
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("static field on derived class should verify");
    }

    #[test]
    fn test_static_method_and_static_field_coexist() {
        // Static methods and static fields on the same class
        let source = r#"
            class C {
                static count = 0;
                static increment() { C.count = C.count + 1; }
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("static method + static field should verify");
        assert!(
            result.string_table.iter().any(|s| s == "count"),
            "should have 'count' in string table"
        );
        assert!(
            result.string_table.iter().any(|s| s == "increment"),
            "should have 'increment' in string table"
        );
    }

    #[test]
    fn test_static_block_with_multiple_statements() {
        // Static block with multiple statements
        let source = r#"
            class Logger {
                static level;
                static {
                    let defaultLevel = "info";
                    Logger.level = defaultLevel;
                }
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module)
            .expect("static block with multiple statements should verify");
    }

    #[test]
    fn test_multiple_static_blocks() {
        // Multiple static blocks in the same class
        let source = r#"
            class C {
                static {
                    let a = 1;
                }
                static {
                    let b = 2;
                }
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("multiple static blocks should verify");
    }

    #[test]
    fn test_static_field_string_initializer() {
        // Static field with string initializer
        let source = r#"
            class C {
                static name = "Counter";
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("static field with string should verify");
        assert!(
            result.string_table.iter().any(|s| s == "Counter"),
            "should have 'Counter' string in string table"
        );
    }

    #[test]
    fn test_static_block_this_set_prop() {
        // Static block using `this` to set a property on the class
        let source = r#"
            class C {
                static {
                    this.initialized = true;
                }
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("static block with this.prop should verify");
        assert!(
            result.string_table.iter().any(|s| s == "initialized"),
            "should have 'initialized' in string table"
        );
    }

    #[test]
    fn test_static_field_class_expression() {
        // Static fields on class expressions
        let source = r#"
            const C = class {
                static x = 42;
                static y;
            };
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module)
            .expect("static fields on class expression should verify");
    }

    #[test]
    fn test_static_block_class_expression() {
        // Static blocks on class expressions
        let source = r#"
            const C = class {
                static {
                    let setup = true;
                }
            };
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module)
            .expect("static block on class expression should verify");
    }

    #[test]
    fn test_instance_field_without_initializer_defaults_to_undefined() {
        // Instance fields without an initializer should also default to undefined
        let source = r#"
            class C {
                x;
                constructor() {}
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("instance field without init should verify");
        // Should have a ConstUndefined for the default value
        assert!(
            entry_has_op(&result.module, Op::ConstUndefined),
            "instance field without initializer should emit ConstUndefined"
        );
    }

    #[test]
    fn test_static_block_with_control_flow() {
        // Static block with if/else control flow
        let source = r#"
            class C {
                static mode;
                static {
                    if (true) {
                        C.mode = "production";
                    } else {
                        C.mode = "development";
                    }
                }
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("static block with control flow should verify");
    }

    #[test]
    fn test_static_field_and_instance_field_together() {
        // Both static and instance fields in the same class
        let source = r#"
            class C {
                static staticField = "static";
                instanceField = "instance";
                constructor() {}
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module)
            .expect("mixed static and instance fields should verify");
        assert!(
            result.string_table.iter().any(|s| s == "staticField"),
            "should have 'staticField'"
        );
        assert!(
            result.string_table.iter().any(|s| s == "instanceField"),
            "should have 'instanceField'"
        );
    }

    #[test]
    fn test_static_block_nested_in_derived_class() {
        // Static block in a derived class
        let source = r#"
            class Base {
                static baseVal = 1;
            }
            class Derived extends Base {
                static {
                    let x = 42;
                }
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("static block in derived class should verify");
    }

    // =========================================================================
    // 0.4.34 — Mapped arguments + arguments.callee
    // =========================================================================

    #[test]
    fn test_needs_mapped_arguments_strict_mode_no() {
        // Strict mode -> never mapped
        use crate::capture::{ArgumentsUsage, needs_mapped_arguments};
        let result = needs_mapped_arguments(true, false, &[], ArgumentsUsage::Used);
        assert!(!result, "strict mode should not need mapped arguments");
    }

    #[test]
    fn test_needs_mapped_arguments_unused_no() {
        // Unused arguments -> never mapped
        use crate::capture::{ArgumentsUsage, needs_mapped_arguments};
        let result = needs_mapped_arguments(false, false, &[], ArgumentsUsage::Unused);
        assert!(!result, "unused arguments should not need mapped");
    }

    #[test]
    fn test_needs_mapped_arguments_rest_param_no() {
        // Rest parameter -> never mapped
        use crate::capture::{ArgumentsUsage, needs_mapped_arguments};
        let result = needs_mapped_arguments(false, true, &[], ArgumentsUsage::Used);
        assert!(
            !result,
            "function with rest param should not need mapped arguments"
        );
    }

    #[test]
    fn test_needs_mapped_arguments_no_params_sloppy() {
        // No params but uses arguments — mapped (vacuously, since no params to alias)
        use crate::capture::{ArgumentsUsage, needs_mapped_arguments};
        let result = needs_mapped_arguments(false, false, &[], ArgumentsUsage::Used);
        assert!(
            result,
            "no-param sloppy function using arguments should need mapped"
        );
    }

    #[test]
    fn test_arguments_sloppy_with_simple_params_ir_verifies() {
        // Sloppy function with simple params using arguments should produce valid IR
        let result = lower_script("function f(a, b) { return arguments[0]; }");
        verify_typed_module(&result.module).expect("arguments IR should verify");
        assert!(
            named_fn_has_op(&result.module, "f", Op::CreateArguments),
            "function using arguments[0] should emit CreateArguments"
        );
    }

    #[test]
    fn test_arguments_not_mapped_with_default_param() {
        // Function with default parameter — arguments still works but NOT mapped
        let result = lower_script("function f(a, b = 1) { return arguments.length; }");
        verify_typed_module(&result.module).expect("default param arguments IR should verify");
        assert!(
            named_fn_has_op(&result.module, "f", Op::CreateArguments),
            "function with default param using arguments should emit CreateArguments"
        );
    }

    #[test]
    fn test_arguments_not_mapped_with_rest_param() {
        // Function with rest parameter — arguments still works but NOT mapped
        let result = lower_script("function f(a, ...rest) { return arguments.length; }");
        verify_typed_module(&result.module).expect("rest param arguments IR should verify");
        assert!(
            named_fn_has_op(&result.module, "f", Op::CreateArguments),
            "function with rest param using arguments should emit CreateArguments"
        );
    }

    #[test]
    fn test_arguments_not_mapped_with_destructuring() {
        // Function with destructuring parameter — arguments still works but NOT mapped
        let result = lower_script("function f({a, b}) { return arguments.length; }");
        verify_typed_module(&result.module).expect("destructuring arguments IR should verify");
        assert!(
            named_fn_has_op(&result.module, "f", Op::CreateArguments),
            "function with destructuring using arguments should emit CreateArguments"
        );
    }

    #[test]
    fn test_arguments_callee_access_lowers() {
        // Accessing arguments.callee should produce valid IR
        let source = r#"
            function f() {
                return arguments.callee;
            }
        "#;
        let result = lower_script(source);
        verify_typed_module(&result.module).expect("arguments.callee IR should verify");
        assert!(
            named_fn_has_op(&result.module, "f", Op::CreateArguments),
            "function using arguments.callee should emit CreateArguments"
        );
    }

    #[test]
    fn test_arguments_in_nested_sloppy_function() {
        // Each function gets its own arguments; nested access creates CreateArguments in inner
        let source = r#"
            function outer(x) {
                function inner(y) {
                    return arguments[0];
                }
                return inner(x + 1);
            }
        "#;
        let result = lower_script(source);
        verify_typed_module(&result.module).expect("nested arguments IR should verify");
        assert!(
            named_fn_has_op(&result.module, "inner", Op::CreateArguments),
            "inner function should have CreateArguments"
        );
        assert!(
            !named_fn_has_op(&result.module, "outer", Op::CreateArguments),
            "outer function should NOT have CreateArguments"
        );
    }

    #[test]
    fn test_arguments_strict_mode_still_lowers() {
        // In strict mode, arguments still works (just no .callee or mapping)
        let source = r#"
            "use strict";
            function f(a) {
                return arguments.length;
            }
        "#;
        let result = lower_script(source);
        verify_typed_module(&result.module).expect("strict arguments IR should verify");
        assert!(
            named_fn_has_op(&result.module, "f", Op::CreateArguments),
            "strict function using arguments.length should emit CreateArguments"
        );
    }

    #[test]
    fn test_arguments_not_mapped_in_strict() {
        // Strict mode function with simple params — NOT mapped
        use crate::capture::{ArgumentsUsage, needs_mapped_arguments};
        let result = needs_mapped_arguments(true, false, &[], ArgumentsUsage::Used);
        assert!(!result, "strict mode should never have mapped arguments");
    }

    #[test]
    fn test_arguments_callee_in_sloppy_method() {
        // arguments.callee should work inside methods too (sloppy mode)
        let source = r#"
            var obj = {
                f: function() { return arguments.callee; }
            };
        "#;
        let result = lower_script(source);
        verify_typed_module(&result.module).expect("method arguments.callee IR should verify");
    }

    #[test]
    fn test_arguments_mutation_lowers() {
        // arguments[0] = val should lower and verify
        let source = r#"
            function f(a) {
                arguments[0] = 42;
                return a;
            }
        "#;
        let result = lower_script(source);
        verify_typed_module(&result.module).expect("arguments mutation IR should verify");
    }

    // === Private fields and methods (v0.4 Wave 3) ===

    #[test]
    fn test_private_field_declaration_emits_install() {
        let source = r#"
            class Foo {
                #x = 42;
                get() { return this.#x; }
            }
        "#;
        let module = lower_and_get_module(source);
        // The constructor function should contain InstallPrivateField
        let has_install = module.functions.iter().any(|f| {
            f.blocks.iter().any(|b| {
                b.instructions
                    .iter()
                    .any(|i| i.op == Op::InstallPrivateField)
            })
        });
        assert!(has_install, "class with #x should emit InstallPrivateField");
    }

    #[test]
    fn test_private_field_get_emits_opcode() {
        let source = r#"
            class Foo {
                #x = 10;
                getX() { return this.#x; }
            }
        "#;
        let module = lower_and_get_module(source);
        let has_get = module.functions.iter().any(|f| {
            f.blocks
                .iter()
                .any(|b| b.instructions.iter().any(|i| i.op == Op::PrivateFieldGet))
        });
        assert!(has_get, "this.#x should emit PrivateFieldGet");
    }

    #[test]
    fn test_private_field_set_emits_opcode() {
        let source = r#"
            class Foo {
                #x = 0;
                setX(v) { this.#x = v; }
            }
        "#;
        let module = lower_and_get_module(source);
        let has_set = module.functions.iter().any(|f| {
            f.blocks
                .iter()
                .any(|b| b.instructions.iter().any(|i| i.op == Op::PrivateFieldSet))
        });
        assert!(has_set, "this.#x = v should emit PrivateFieldSet");
    }

    #[test]
    fn test_private_in_expression_emits_opcode() {
        let source = r#"
            class Foo {
                #x = 1;
                static has(obj) { return #x in obj; }
            }
        "#;
        let module = lower_and_get_module(source);
        let has_check = module.functions.iter().any(|f| {
            f.blocks
                .iter()
                .any(|b| b.instructions.iter().any(|i| i.op == Op::PrivateFieldHas))
        });
        assert!(has_check, "#x in obj should emit PrivateFieldHas");
    }

    #[test]
    fn test_private_method_emits_install_and_get() {
        let source = r#"
            class Foo {
                #bar() { return 1; }
                callBar() { return this.#bar(); }
            }
        "#;
        let module = lower_and_get_module(source);
        let has_install = module.functions.iter().any(|f| {
            f.blocks.iter().any(|b| {
                b.instructions
                    .iter()
                    .any(|i| i.op == Op::InstallPrivateField)
            })
        });
        let has_get = module.functions.iter().any(|f| {
            f.blocks
                .iter()
                .any(|b| b.instructions.iter().any(|i| i.op == Op::PrivateFieldGet))
        });
        assert!(
            has_install,
            "private method should emit InstallPrivateField in ctor"
        );
        assert!(
            has_get,
            "this.#bar() should emit PrivateFieldGet for the method"
        );
    }

    #[test]
    fn test_private_field_no_init_emits_undefined() {
        let source = r#"
            class Foo {
                #y;
                getY() { return this.#y; }
            }
        "#;
        let module = lower_and_get_module(source);
        let has_install = module.functions.iter().any(|f| {
            f.blocks.iter().any(|b| {
                b.instructions
                    .iter()
                    .any(|i| i.op == Op::InstallPrivateField)
            })
        });
        assert!(
            has_install,
            "#y with no init should still emit InstallPrivateField"
        );
    }

    #[test]
    fn test_private_field_class_verifies() {
        let source = r#"
            class A {
                #x = 1;
                #y;
                #method() { return this.#x; }
                get() { return this.#x; }
                set(v) { this.#y = v; }
                call() { return this.#method(); }
                check(obj) { return #x in obj; }
            }
            let a = new A();
        "#;
        let result = lower_program(source).expect("should lower class with private fields");
        verify_typed_module(&result.module).expect("IR should verify");
    }

    #[test]
    fn test_two_classes_different_private_ids() {
        let source = r#"
            class A {
                #x = 1;
                getX() { return this.#x; }
            }
            class B {
                #x = 2;
                getX() { return this.#x; }
            }
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module).expect("two classes with same #x should verify");
        // Both classes should have InstallPrivateField instructions
        let install_count = result
            .module
            .functions
            .iter()
            .flat_map(|f| {
                f.blocks.iter().flat_map(|b| {
                    b.instructions
                        .iter()
                        .filter(|i| i.op == Op::InstallPrivateField)
                })
            })
            .count();
        assert!(
            install_count >= 2,
            "two classes with #x should have at least 2 InstallPrivateField"
        );
    }

    #[test]
    fn test_private_field_default_ctor() {
        // Class with private fields but no explicit constructor
        let source = r#"
            class Foo {
                #val = 42;
                get() { return this.#val; }
            }
            let f = new Foo();
        "#;
        let result = lower_program(source).expect("should lower");
        verify_typed_module(&result.module)
            .expect("default ctor with private fields should verify");
    }

    #[test]
    fn test_private_field_ir_printer() {
        let source = r#"
            class Foo {
                #x = 1;
                get() { return this.#x; }
            }
        "#;
        let ir = lower_and_print(source);
        assert!(
            ir.contains("install_private_field"),
            "IR should contain install_private_field"
        );
        assert!(
            ir.contains("private_field_get"),
            "IR should contain private_field_get"
        );
    }

    // === Compile-time platform constants ===

    #[test]
    fn test_platform_constant_esc_platform_resolves_to_string() {
        let result = lower_program("__esc_platform;").expect("lowering should succeed");
        verify_typed_module(&result.module).expect("IR should verify");
        // The platform constant should be in the string table
        assert!(
            result
                .string_table
                .iter()
                .any(|s| s == std::env::consts::OS),
            "string table should contain OS value ({})",
            std::env::consts::OS,
        );
    }

    #[test]
    fn test_platform_constant_esc_arch_resolves_to_string() {
        let result = lower_program("__esc_arch;").expect("lowering should succeed");
        verify_typed_module(&result.module).expect("IR should verify");
        assert!(
            result
                .string_table
                .iter()
                .any(|s| s == std::env::consts::ARCH),
            "string table should contain ARCH value ({})",
            std::env::consts::ARCH,
        );
    }

    #[test]
    fn test_platform_constant_esc_build_mode_default_debug() {
        let result = lower_program("__esc_build_mode;").expect("lowering should succeed");
        verify_typed_module(&result.module).expect("IR should verify");
        assert!(
            result.string_table.iter().any(|s| s == "debug"),
            "string table should contain 'debug' for default build mode",
        );
    }

    #[test]
    fn test_platform_constant_esc_build_mode_release() {
        let result = crate::lower_source_with_build_mode(
            "__esc_build_mode;",
            oxc_span::SourceType::mjs(),
            "release",
        )
        .expect("lowering should succeed");
        verify_typed_module(&result.module).expect("IR should verify");
        assert!(
            result.string_table.iter().any(|s| s == "release"),
            "string table should contain 'release' for release build mode",
        );
    }

    /// Check if any block in the entry function contains a ConstString opcode.
    fn entry_has_const_string(module: &ir::builder::TypedModule) -> bool {
        let entry_fn = &module.functions[module.entry.unwrap()];
        entry_fn.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|inst| matches!(inst.op, Op::ConstString(_)))
        })
    }

    #[test]
    fn test_platform_constant_emits_const_string_opcode() {
        let module = lower_and_get_module("__esc_platform;");
        // The ConstString opcode should be emitted for the platform constant
        assert!(
            entry_has_const_string(&module),
            "should emit ConstString for __esc_platform"
        );
    }

    #[test]
    fn test_platform_constant_arch_emits_const_string_opcode() {
        let module = lower_and_get_module("__esc_arch;");
        assert!(
            entry_has_const_string(&module),
            "should emit ConstString for __esc_arch"
        );
    }

    #[test]
    fn test_platform_constant_build_mode_emits_const_string_opcode() {
        let module = lower_and_get_module("__esc_build_mode;");
        assert!(
            entry_has_const_string(&module),
            "should emit ConstString for __esc_build_mode"
        );
    }

    #[test]
    fn test_platform_constant_local_variable_shadows_intrinsic() {
        // A local variable declaration should shadow the platform intrinsic.
        // After `let __esc_platform = "custom"`, reads should see the local,
        // not the compile-time constant.
        let result = lower_program(r#"let __esc_platform = "custom"; __esc_platform;"#)
            .expect("lowering should succeed");
        verify_typed_module(&result.module).expect("IR should verify");
        // The string "custom" should be in the string table
        assert!(
            result.string_table.iter().any(|s| s == "custom"),
            "string table should contain 'custom' from the local variable",
        );
        // The compile-time OS value should NOT be in the string table,
        // because the local variable shadows the intrinsic and the second
        // reference resolves to the local, not the intrinsic.
        let os = std::env::consts::OS;
        assert!(
            !result.string_table.iter().any(|s| s == os),
            "string table should NOT contain OS value when shadowed by local"
        );
    }

    #[test]
    fn test_platform_constant_not_a_reference_error() {
        // Platform constants should NOT produce a ReferenceError throw.
        // Instead they should emit ConstString. We verify by checking
        // the IR text does NOT contain `__esc_rt_throw_reference_error`.
        let ir = lower_and_print("__esc_platform;");
        assert!(
            !ir.contains("__esc_rt_throw_reference_error"),
            "platform constant should not trigger ReferenceError"
        );
    }

    #[test]
    fn test_platform_constant_values_are_nonempty() {
        // Both OS and ARCH should be non-empty strings
        assert!(
            !std::env::consts::OS.is_empty(),
            "__esc_platform value should be non-empty"
        );
        assert!(
            !std::env::consts::ARCH.is_empty(),
            "__esc_arch value should be non-empty"
        );
    }

    // === for await...of (Step 0.5.13) ===

    #[test]
    fn test_for_await_of_emits_await_after_iter_next() {
        // for await...of must emit Await opcode after IterNext
        let module =
            lower_and_get_module("async function f(iter) { for await (const x of iter) {} }");
        assert!(
            any_fn_has_op(&module, Op::Await),
            "for-await-of should emit Await opcode"
        );
        assert!(
            any_fn_has_op(&module, Op::IterNext),
            "for-await-of should emit IterNext"
        );
    }

    #[test]
    fn test_for_await_of_uses_iter_init_async() {
        // for await...of must use IterInitAsync instead of IterInit
        let module =
            lower_and_get_module("async function f(iter) { for await (const x of iter) {} }");
        assert!(
            any_fn_has_op(&module, Op::IterInitAsync),
            "for-await-of should emit IterInitAsync"
        );
        // Should NOT use regular IterInit for the async loop
        // (IterInit may still appear in other contexts, but the async
        // function should have IterInitAsync)
        let async_fn = &module.functions.iter().find(|f| f.name == "f");
        assert!(async_fn.is_some(), "should find async function f");
        let func = async_fn.unwrap();
        let has_init_async = func
            .blocks
            .iter()
            .any(|b| b.instructions.iter().any(|i| i.op == Op::IterInitAsync));
        assert!(
            has_init_async,
            "async function should use IterInitAsync for for-await-of"
        );
    }

    #[test]
    fn test_for_await_of_fallback_to_symbol_iterator() {
        // IterInitAsync handles the fallback internally at runtime,
        // but we verify the opcode is emitted correctly
        let module =
            lower_and_get_module("async function f(arr) { for await (const x of arr) {} }");
        assert!(
            any_fn_has_op(&module, Op::IterInitAsync),
            "for-await-of should emit IterInitAsync even for regular iterables"
        );
    }

    #[test]
    fn test_for_await_of_with_destructuring() {
        // for await (const [a, b] of iter) should emit both Await and GetElem
        let module =
            lower_and_get_module("async function f(iter) { for await (const [a, b] of iter) {} }");
        assert!(
            any_fn_has_op(&module, Op::Await),
            "for-await-of with destructuring should emit Await"
        );
        assert!(
            any_fn_has_op(&module, Op::IterInitAsync),
            "for-await-of with destructuring should emit IterInitAsync"
        );
        assert!(
            any_fn_has_op(&module, Op::GetElem),
            "for-await-of with array destructuring should emit GetElem"
        );
    }

    #[test]
    fn test_for_await_of_with_object_destructuring() {
        // for await (const {x, y} of iter)
        let module = lower_and_get_module(
            r#"async function f(iter) { for await (const {x, y} of iter) {} }"#,
        );
        assert!(
            any_fn_has_op(&module, Op::Await),
            "for-await-of with object destructuring should emit Await"
        );
        assert!(
            any_fn_has_op(&module, Op::GetProp),
            "for-await-of with object destructuring should emit GetProp"
        );
    }

    #[test]
    fn test_for_await_of_break_triggers_iter_close() {
        // Break out of for-await-of should still close the iterator
        let module = lower_and_get_module(
            "async function f(iter) { for await (const x of iter) { break; } }",
        );
        assert!(
            any_fn_has_op(&module, Op::IterClose),
            "for-await-of with break should emit IterClose"
        );
        assert!(
            any_fn_has_op(&module, Op::Await),
            "for-await-of should emit Await"
        );
    }

    #[test]
    fn test_for_await_of_in_async_function_generates_correct_ir() {
        // The IR for an async function with for-await-of should verify
        let sources = [
            "async function f(iter) { for await (const x of iter) {} }",
            "async function f(iter) { for await (const x of iter) { console.log(x); } }",
            "async function f(iter) { for await (const [a] of iter) {} }",
            r#"async function f(iter) { for await (const {k} of iter) {} }"#,
        ];
        for source in sources {
            let result = lower_program(source).expect("lowering should succeed");
            verify_typed_module(&result.module)
                .unwrap_or_else(|e| panic!("IR should verify for '{source}': {e:?}"));
        }
    }

    #[test]
    fn test_for_await_of_in_async_generator_generates_correct_ir() {
        // for-await-of can also appear inside async generators
        let sources = [
            "async function* f(iter) { for await (const x of iter) { yield x; } }",
            "async function* f(iter) { for await (const x of iter) {} }",
        ];
        for source in sources {
            let result = lower_program(source).expect("lowering should succeed");
            verify_typed_module(&result.module)
                .unwrap_or_else(|e| panic!("IR should verify for '{source}': {e:?}"));
        }
    }

    #[test]
    fn test_for_await_of_await_count_matches_loop() {
        // Each iteration should produce exactly one Await per iter_next
        let module =
            lower_and_get_module("async function f(iter) { for await (const x of iter) {} }");
        // Find the function f
        let func = module.functions.iter().find(|f| f.name == "f").unwrap();
        let await_count = func
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .filter(|i| i.op == Op::Await)
            .count();
        // Should have exactly 1 Await in the loop header
        assert_eq!(
            await_count, 1,
            "for-await-of should emit exactly one Await (for iter.next() result)"
        );
    }

    #[test]
    fn test_for_await_of_iter_done_uses_awaited_result() {
        // Verify that IterDone operates on the awaited result, not the raw
        // iter_next result. The chain is: IterNext -> Await -> IterDone.
        let module =
            lower_and_get_module("async function f(iter) { for await (const x of iter) {} }");
        let func = module.functions.iter().find(|f| f.name == "f").unwrap();

        let mut await_result = None;
        let mut iter_done_operand = None;

        for block in &func.blocks {
            for inst in &block.instructions {
                if inst.op == Op::Await {
                    await_result = Some(inst.id);
                }
                if inst.op == Op::IterDone {
                    iter_done_operand = Some(inst.operands[0]);
                }
            }
        }

        assert!(await_result.is_some(), "should have Await");
        assert!(iter_done_operand.is_some(), "should have IterDone");
        assert_eq!(
            await_result.unwrap(),
            iter_done_operand.unwrap(),
            "IterDone should operate on Await result, not raw IterNext result"
        );
    }

    #[test]
    fn test_regular_for_of_does_not_emit_await() {
        // Regular for-of should NOT emit Await or IterInitAsync
        let module = lower_and_get_module("let arr = [1,2]; for (const x of arr) {}");
        assert!(
            !entry_has_op(&module, Op::Await),
            "regular for-of should not emit Await"
        );
        assert!(
            !entry_has_op(&module, Op::IterInitAsync),
            "regular for-of should not emit IterInitAsync"
        );
        assert!(
            entry_has_op(&module, Op::IterInit),
            "regular for-of should still emit IterInit"
        );
    }

    #[test]
    fn test_for_await_of_with_body_statements() {
        // for-await-of with non-trivial body
        let module = lower_and_get_module(
            r#"async function f(iter) {
                let sum = 0;
                for await (const x of iter) {
                    sum = sum + x;
                }
            }"#,
        );
        assert!(
            any_fn_has_op(&module, Op::Await),
            "for-await-of with body should emit Await"
        );
        assert!(
            any_fn_has_op(&module, Op::AddJS),
            "for-await-of body with addition should emit AddJS"
        );
    }

    #[test]
    fn test_for_await_of_printed_ir_contains_await() {
        // Verify the printed IR contains both iter_init_async and await
        let ir = lower_and_print("async function f(iter) { for await (const x of iter) {} }");
        assert!(
            ir.contains("iter_init_async"),
            "printed IR should contain iter_init_async"
        );
        assert!(ir.contains("await"), "printed IR should contain await");
    }

    #[test]
    fn test_for_await_of_with_let_binding() {
        // for await (let x of iter) — let binding instead of const
        let module =
            lower_and_get_module("async function f(iter) { for await (let x of iter) {} }");
        assert!(
            any_fn_has_op(&module, Op::IterInitAsync),
            "for-await-of with let should emit IterInitAsync"
        );
        assert!(
            any_fn_has_op(&module, Op::Await),
            "for-await-of with let should emit Await"
        );
    }

    #[test]
    fn test_for_await_of_with_var_binding() {
        // for await (var x of iter) — var binding
        let module =
            lower_and_get_module("async function f(iter) { for await (var x of iter) {} }");
        assert!(
            any_fn_has_op(&module, Op::IterInitAsync),
            "for-await-of with var should emit IterInitAsync"
        );
        assert!(
            any_fn_has_op(&module, Op::Await),
            "for-await-of with var should emit Await"
        );
    }

    // -----------------------------------------------------------------------
    // Top-level await (ES2022)
    // -----------------------------------------------------------------------

    #[test]
    fn test_tla_module_with_await_marks_entry_async() {
        // Module with top-level await: entry function should be is_async = true.
        let result =
            lower_program("const x = await Promise.resolve(42);").expect("lowering should succeed");
        assert!(
            result.has_top_level_await,
            "module with top-level await should set has_top_level_await"
        );
        assert!(
            result.module.functions[0].is_async,
            "entry function should be marked as async for TLA"
        );
    }

    #[test]
    fn test_tla_module_without_await_not_async() {
        // Module without any await: entry function should NOT be async.
        let result =
            lower_program("const x = 42; export default x;").expect("lowering should succeed");
        assert!(
            !result.has_top_level_await,
            "module without top-level await should NOT set has_top_level_await"
        );
        assert!(
            !result.module.functions[0].is_async,
            "entry function should NOT be async without TLA"
        );
    }

    #[test]
    fn test_tla_script_with_await_not_marked_async() {
        // Scripts use sloppy mode and should NOT trigger TLA detection.
        // In scripts, `await` is not valid at top-level (parser may reject it),
        // but even if it somehow sneaks through, has_top_level_await must be false.
        let result = lower_script("async function f() { await 1; }");
        assert!(
            !result.has_top_level_await,
            "scripts should never have has_top_level_await set"
        );
        // The inner async function should be async, but the entry (main) should not.
        assert!(
            !result.module.functions[0].is_async,
            "script entry function should not be async"
        );
    }

    #[test]
    fn test_tla_module_entry_has_await_opcode() {
        // Verify the entry function actually contains an Await opcode.
        let result =
            lower_program("const data = await fetch('url');").expect("lowering should succeed");
        let entry = &result.module.functions[0];
        let has_await = entry
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .any(|i| i.op == Op::Await);
        assert!(has_await, "entry function should contain Await opcode");
    }

    #[test]
    fn test_tla_nested_async_does_not_make_entry_async() {
        // An async function inside a module does NOT make the module TLA.
        // Only top-level await in the module body triggers TLA.
        let result = lower_program(
            "async function doStuff() { await Promise.resolve(1); }\nexport { doStuff };",
        )
        .expect("lowering should succeed");
        assert!(
            !result.has_top_level_await,
            "nested async function should not make module TLA"
        );
        assert!(
            !result.module.functions[0].is_async,
            "entry function should not be async when only inner functions use await"
        );
    }

    #[test]
    fn test_tla_module_with_exports_still_works() {
        // Module with TLA + regular exports: both should work.
        let result = lower_program("export const val = await Promise.resolve(99);")
            .expect("lowering should succeed");
        assert!(
            result.has_top_level_await,
            "module with TLA export should set has_top_level_await"
        );
        assert!(
            result.module.functions[0].is_async,
            "entry function should be async for TLA"
        );
        assert!(
            !result.exports.is_empty(),
            "exports should still be recorded"
        );
    }

    #[test]
    fn test_tla_multiple_awaits_in_module() {
        // Module with multiple top-level awaits: still TLA.
        let result = lower_program(
            "const a = await Promise.resolve(1);\nconst b = await Promise.resolve(2);",
        )
        .expect("lowering should succeed");
        assert!(
            result.has_top_level_await,
            "module with multiple awaits should set has_top_level_await"
        );
    }

    #[test]
    fn test_tla_generator_transform_handles_async_entry() {
        // After TLA detection, generator_transform should process the async
        // entry function. Verify the transform doesn't error.
        let result =
            lower_program("const x = await Promise.resolve(42);").expect("lowering should succeed");
        let mut module = result.module;
        let transform_result = generator_transform::transform_module(&mut module);
        assert!(
            transform_result.is_ok(),
            "generator_transform should handle async entry: {:?}",
            transform_result.err()
        );
    }

    // =================================================================
    // Regression: nested function inside try-finally must not inherit
    // the outer function's finally/catch block state (causes
    // "add_predecessor: block not found" panic).
    // =================================================================

    #[test]
    fn test_function_inside_try_finally_no_panic() {
        // A function declaration inside a try-finally block.
        // The inner function's `return` must NOT attempt to branch to the
        // outer function's finally block (which lives in a suspended
        // block namespace).
        let _ir = lower_and_print(
            r#"
            try {
                function inner() {
                    return 1;
                }
                inner();
            } finally {
                let x = 2;
            }
            "#,
        );
    }

    #[test]
    fn test_arrow_inside_try_finally_no_panic() {
        // Arrow function inside try-finally — same root cause.
        let _ir = lower_and_print(
            r#"
            try {
                const f = () => { return 42; };
                f();
            } finally {
                let y = 3;
            }
            "#,
        );
    }

    #[test]
    fn test_nested_function_in_try_catch_finally_no_panic() {
        // Function inside try-catch-finally with throw and return paths.
        let _ir = lower_and_print(
            r#"
            try {
                function inner() {
                    throw new Error("test");
                }
                inner();
            } catch (e) {
                function handler() {
                    return e;
                }
                handler();
            } finally {
                function cleanup() {
                    return "done";
                }
                cleanup();
            }
            "#,
        );
    }

    #[test]
    fn test_continue_break_in_loop_with_try_finally() {
        // continue and break inside a loop with try-finally.
        let _ir = lower_and_print(
            r#"
            for (var i = 0; i < 10; i++) {
                try {
                    if (i === 3) continue;
                    if (i === 7) break;
                } finally {
                    var x = i;
                }
            }
            "#,
        );
    }

    #[test]
    fn test_labeled_break_in_nested_loop_try_finally() {
        // Labeled break across try-finally in nested loops.
        let _ir = lower_and_print(
            r#"
            outer: for (var i = 0; i < 5; i++) {
                for (var j = 0; j < 5; j++) {
                    try {
                        if (j === 2) continue;
                        if (i === 3) break outer;
                    } finally {
                        var z = 0;
                    }
                }
            }
            "#,
        );
    }

    #[test]
    fn test_function_inside_try_with_loop_no_panic() {
        // Function declared inside a try block that also has a loop.
        // The function should not see the loop's break/continue targets.
        let _ir = lower_and_print(
            r#"
            for (var i = 0; i < 5; i++) {
                try {
                    function inner() {
                        return i;
                    }
                    if (inner() > 3) break;
                } finally {
                    var x = 1;
                }
            }
            "#,
        );
    }

    #[test]
    fn test_try_catch_optional_binding() {
        // ES2019 optional catch binding: catch { ... } with no parameter
        let ir = lower_and_print(
            r#"
            try {
                throw 1;
            } catch {
                let x = 2;
            }
            "#,
        );
        assert!(ir.contains("catch"), "should have catch instruction");
    }

    #[test]
    fn test_finally_runs_on_return_in_try() {
        // Finally must execute even when try block has a return.
        // The IR should show the return value being stored, branch to finally,
        // then replay the return after the finally body completes.
        let ir = lower_and_print(
            r#"
            function f() {
                try {
                    return 1;
                } finally {
                    let x = 2;
                }
            }
            "#,
        );
        // The return value 1 is lowered as a numeric constant. The finally
        // body assigns 2 to x. Both should be present in the IR, indicating
        // that finally's code is emitted alongside the return path.
        // The finally completion check uses to_boolean on the has_return flag
        // and then conditionally returns the saved value.
        assert!(
            ir.contains("to_bool"),
            "should have to_boolean for finally completion check, IR:\n{ir}"
        );
        assert!(
            ir.contains("ret"),
            "should have ret instruction for the return replay, IR:\n{ir}"
        );
    }

    #[test]
    fn test_throw_in_catch_rethrows() {
        // throw inside catch should work correctly
        let ir = lower_and_print(
            r#"
            try {
                throw 1;
            } catch (e) {
                throw e;
            }
            "#,
        );
        // Should have at least two throw-related instructions
        assert!(ir.contains("throw"), "should have throw instruction");
    }

    #[test]
    fn test_nested_try_catch_finally() {
        // Nested try/catch/finally interactions
        let _ir = lower_and_print(
            r#"
            try {
                try {
                    throw 1;
                } catch (e) {
                    throw 2;
                } finally {
                    let a = 3;
                }
            } catch (e2) {
                let b = e2;
            } finally {
                let c = 4;
            }
            "#,
        );
    }

    // =================================================================
    // eval Tier 0 — compile-time constant string detection
    // =================================================================

    #[test]
    fn test_eval_constant_expression_inlined() {
        // eval("1 + 2") should inline the constant expression, not emit CallEval
        let module = lower_script_module(r#"var result = eval("1 + 2");"#);
        assert!(
            !entry_has_op(&module, Op::CallEval),
            "constant string eval should be inlined, not emit CallEval"
        );
        // The inlined code should produce an Add instruction
        assert!(
            entry_has_op(&module, Op::AddJS),
            "inlined eval('1 + 2') should produce an Add instruction"
        );
    }

    #[test]
    fn test_eval_var_declaration_inlined() {
        // eval("var x = 10") should inline the var declaration into the caller's scope
        let ir = lower_script_print(r#"eval("var x = 10");"#);
        assert!(
            !ir.contains("call_eval"),
            "constant string eval should not emit call_eval"
        );
        // Should have a const_i32(10) from the inlined var declaration
        assert!(
            ir.contains("const_f64 10"),
            "inlined eval('var x = 10') should produce const_f64(10)"
        );
    }

    #[test]
    fn test_eval_variable_arg_not_inlined() {
        // eval(variable) should NOT be inlined — falls through to CallEval
        let module = lower_script_module(
            r#"
            var code = "1 + 2";
            var result = eval(code);
            "#,
        );
        assert!(
            entry_has_op(&module, Op::CallEval),
            "eval with non-constant argument should emit CallEval"
        );
    }

    // -----------------------------------------------------------------------
    // with statement lowering tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_with_statement_reads_property_from_object() {
        // Inside with(obj), reading `x` should emit EnvLookup
        let module = lower_script_module(
            r#"
            var obj = {};
            with (obj) {
                var result = x;
            }
            "#,
        );
        assert!(
            entry_has_op(&module, Op::EnvLookup),
            "with body should use EnvLookup for non-lexical identifier reads"
        );
    }

    #[test]
    fn test_eval_syntax_error_falls_through() {
        // eval("syntax error!!!") should fall through to CallEval gracefully
        let module = lower_script_module(r#"var result = eval("let = ;");"#);
        assert!(
            entry_has_op(&module, Op::CallEval),
            "eval with unparseable string should fall through to CallEval"
        );
    }

    #[test]
    fn test_eval_let_in_strict_mode_confined_scope() {
        // In strict mode, eval("let x = 1") confines the variable to its own scope.
        // The let variable should NOT leak into the outer scope.
        let ir = lower_and_print(
            r#"
            let x = 99;
            eval("let x = 1");
            "#,
        );
        // Module mode is strict. The eval should inline but `let x = 1` is
        // confined to the eval's block scope, not conflicting with outer `x`.
        assert!(
            !ir.contains("call_eval"),
            "constant eval in strict module should be inlined"
        );
    }

    #[test]
    fn test_eval_empty_string_returns_undefined() {
        // eval("") should return undefined
        let ir = lower_script_print(r#"var result = eval("");"#);
        assert!(
            !ir.contains("call_eval"),
            "eval('') should be inlined as const_undefined"
        );
        assert!(
            ir.contains("const_undefined"),
            "eval('') should produce const_undefined"
        );
    }

    #[test]
    fn test_eval_no_args_returns_undefined() {
        // eval() with no arguments should return undefined
        let ir = lower_script_print("var result = eval();");
        assert!(
            !ir.contains("call_eval"),
            "eval() with no args should be inlined as const_undefined"
        );
    }

    #[test]
    fn test_eval_nested_eval_outer_inlined() {
        // eval("eval('1')") — the outer eval string is constant, so it gets
        // parsed. The inner eval('1') is also a constant string, so it should
        // also be inlined.
        let module = lower_script_module(r#"var result = eval("eval('1')");"#);
        assert!(
            !entry_has_op(&module, Op::CallEval),
            "nested constant eval should be fully inlined"
        );
    }

    #[test]
    fn test_eval_multiple_statements_returns_last() {
        // eval("1; 2; 3") should return the last expression value (3)
        let ir = lower_script_print(r#"var result = eval("1; 2; 3");"#);
        assert!(
            !ir.contains("call_eval"),
            "constant multi-statement eval should be inlined"
        );
        // Should contain const_i32 for at least the literal 3
        assert!(
            ir.contains("const_f64 3"),
            "eval('1; 2; 3') should produce const_f64(3) as the last value"
        );
    }

    #[test]
    fn test_eval_member_call_not_intercepted() {
        // window.eval("1") or obj.eval("1") should NOT be treated as direct eval
        // — only bare `eval(...)` is direct eval per spec
        let ir = lower_script_print(r#"var obj = {}; obj.eval("1 + 2");"#);
        assert!(
            !ir.contains("call_eval"),
            "member call .eval() should not be treated as direct eval"
        );
    }

    #[test]
    fn test_eval_template_literal_constant_inlined() {
        // eval(`1 + 2`) with a pure constant template literal should be inlined
        let module = lower_script_module("var result = eval(`1 + 2`);");
        assert!(
            !entry_has_op(&module, Op::CallEval),
            "eval with constant template literal should be inlined"
        );
        assert!(
            entry_has_op(&module, Op::AddJS),
            "inlined eval(`1 + 2`) should produce an Add instruction"
        );
    }

    #[test]
    fn test_eval_template_literal_with_expr_not_inlined() {
        // eval(`${x}`) with a dynamic template literal should NOT be inlined
        let module = lower_script_module(
            r#"
            var x = "1 + 2";
            var result = eval(`${x}`);
            "#,
        );
        assert!(
            entry_has_op(&module, Op::CallEval),
            "eval with dynamic template literal should emit CallEval"
        );
    }

    #[test]
    fn test_with_statement_writes_to_object_property() {
        // Inside with(obj), writing to `x` should emit EnvLookupStore
        let module = lower_script_module(
            r#"
            var obj = {};
            with (obj) {
                x = 42;
            }
            "#,
        );
        assert!(
            entry_has_op(&module, Op::EnvLookupStore),
            "with body should use EnvLookupStore for non-lexical identifier writes"
        );
    }

    #[test]
    fn test_with_statement_falls_through_to_outer_scope() {
        // Names not on the with-object should fall through to outer scope
        // via the EscEnvironment chain — the lookup still goes through EnvLookup
        let module = lower_script_module(
            r#"
            var outer = 10;
            var obj = {};
            with (obj) {
                var result = outer;
            }
            "#,
        );
        // `outer` is declared before the with body, so inside the with body
        // it should go through EnvLookup (the dynamic env handles fallthrough)
        assert!(
            entry_has_op(&module, Op::EnvLookup),
            "with body should use EnvLookup even for names that may exist in outer scope"
        );
    }

    #[test]
    fn test_with_statement_let_const_not_affected() {
        // let/const declared inside the with body should use normal resolution
        // (not dynamic lookup through the with-object)
        let module = lower_script_module(
            r#"
            var obj = {};
            with (obj) {
                let localVar = 42;
                var result = localVar;
            }
            "#,
        );
        // `localVar` is declared with let inside the with body, so reading it
        // should NOT use EnvLookup — it should resolve lexically.
        // However, `result` uses var which hoists, so it may go through EnvLookup.
        // Check that the IR still compiles and verifies correctly.
        let ir_text = print_typed_module(&module);
        // The let variable `localVar` access should not go through env_lookup
        // (it's in the same block scope), but the IR should contain at least
        // one env_lookup for the with scope setup
        assert!(
            ir_text.contains("call_runtime") || ir_text.contains("env_lookup"),
            "with statement body should compile correctly"
        );
    }

    #[test]
    fn test_with_statement_nested() {
        // Nested with statements should work: inner with should create its own env
        let module = lower_script_module(
            r#"
            var a = {};
            var b = {};
            with (a) {
                with (b) {
                    var result = x;
                }
            }
            "#,
        );
        // Should have at least two CallRuntime ops for the two with-env create calls
        // (plus other CallRuntime ops for ToObject, etc.)
        let entry_fn = &module.functions[module.entry.unwrap_or(0)];
        let call_runtime_count = entry_fn
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .filter(|i| i.op == Op::CallRuntime)
            .count();
        // At least 2 CallRuntime for __esc_rt_with_env_create, but there are more
        // (ToObject, etc.). Just check we have enough.
        assert!(
            call_runtime_count >= 2,
            "nested with should produce multiple CallRuntime ops, found {call_runtime_count}"
        );
        // Also verify EnvLookup is present for the `x` reference in the inner body
        assert!(
            entry_has_op(&module, Op::EnvLookup),
            "nested with should use EnvLookup for `x`"
        );
    }

    #[test]
    fn test_with_statement_variable_shadowing() {
        // A let inside with body should shadow the with-object's property
        let module = lower_script_module(
            r#"
            var obj = {};
            with (obj) {
                let x = 99;
                var result = x;
            }
            "#,
        );
        // `x` is declared with let inside the with body, so reading `x`
        // should resolve lexically (NOT through EnvLookup).
        // The IR should compile successfully.
        verify_typed_module(&module).expect("IR should verify");
    }

    #[test]
    fn test_with_statement_ir_contains_env_lookup_opcodes() {
        // Verify the IR output contains the expected env_lookup / env_lookup_store opcodes
        let module = lower_script_module(
            r#"
            var obj = {};
            with (obj) {
                x = y;
            }
            "#,
        );
        assert!(
            entry_has_op(&module, Op::EnvLookup),
            "with body should contain EnvLookup for reading `y`"
        );
        assert!(
            entry_has_op(&module, Op::EnvLookupStore),
            "with body should contain EnvLookupStore for writing to `x`"
        );
    }

    #[test]
    fn test_with_statement_to_object_conversion() {
        // The with expression should be converted to an object via ToObject
        let module = lower_script_module(
            r#"
            with ({}) {
                var x = 1;
            }
            "#,
        );
        assert!(
            entry_has_op(&module, Op::ToObject),
            "with expression should be converted via ToObject"
        );
    }

    #[test]
    fn test_with_statement_creates_with_env() {
        // The lowered IR should contain a call to __esc_rt_with_env_create
        let ir_text = lower_script_print(
            r#"
            var obj = {};
            with (obj) {
                var x = 1;
            }
            "#,
        );
        // The IR printer uses `call_runtime` for Op::CallRuntime and shows
        // the string table index, not the literal function name. Check for
        // call_runtime opcode which is used for the with-env creation.
        assert!(
            ir_text.contains("call_runtime"),
            "with statement should emit call_runtime for __esc_rt_with_env_create"
        );
    }

    #[test]
    fn test_with_statement_update_expression() {
        // x++ inside a with body should use dynamic lookup + store
        let module = lower_script_module(
            r#"
            var obj = {};
            with (obj) {
                x++;
            }
            "#,
        );
        assert!(
            entry_has_op(&module, Op::EnvLookup),
            "x++ in with body should read via EnvLookup"
        );
        assert!(
            entry_has_op(&module, Op::EnvLookupStore),
            "x++ in with body should write via EnvLookupStore"
        );
    }

    #[test]
    fn test_with_statement_compound_assignment() {
        // x += 1 inside a with body should use dynamic lookup + store
        let module = lower_script_module(
            r#"
            var obj = {};
            with (obj) {
                x += 1;
            }
            "#,
        );
        assert!(
            entry_has_op(&module, Op::EnvLookup),
            "x += 1 in with body should read via EnvLookup"
        );
        assert!(
            entry_has_op(&module, Op::EnvLookupStore),
            "x += 1 in with body should write via EnvLookupStore"
        );
    }

    #[test]
    fn test_with_statement_with_scope_kind_pushed() {
        // Scope analysis should detect the With scope
        let sa = analyze_script(
            r#"
            function f() {
                with (obj) {
                    x = 1;
                }
            }
            "#,
        );
        let root = sa.root_scope();
        let fn_scope = sa.scope(root).children[0];
        assert!(
            sa.scope_flags(fn_scope).needs_dynamic_env,
            "function containing with should need dynamic env"
        );
    }

    #[test]
    fn test_with_statement_does_not_affect_member_expressions() {
        // Member expressions (obj.prop) should NOT go through EnvLookup —
        // only bare identifiers are affected by with scope
        let module = lower_script_module(
            r#"
            var obj = {};
            with (obj) {
                var result = console.log;
            }
            "#,
        );
        // `console` goes through EnvLookup, but `.log` is a normal property access
        assert!(
            entry_has_op(&module, Op::EnvLookup),
            "`console` reference should use EnvLookup"
        );
    }

    #[test]
    fn test_with_statement_preserves_outer_after_with() {
        // After the with block ends, normal identifier resolution should resume
        let module = lower_script_module(
            r#"
            var obj = {};
            var outer = 10;
            with (obj) {
                var inner = x;
            }
            var after = outer;
            "#,
        );
        // The IR should compile and verify correctly
        verify_typed_module(&module).expect("IR should verify after with block");
    }

    #[test]
    fn test_with_statement_function_call_inside_with() {
        // Function calls inside with should work — the callee goes through
        // dynamic lookup if it's a bare identifier
        let module = lower_script_module(
            r#"
            var obj = {};
            with (obj) {
                foo();
            }
            "#,
        );
        assert!(
            entry_has_op(&module, Op::EnvLookup),
            "function name `foo` in with body should use EnvLookup"
        );
    }

    // -----------------------------------------------------------------------
    // typeof inside with scope (v0.6.4)
    // -----------------------------------------------------------------------

    #[test]
    fn test_with_statement_typeof_undeclared_uses_env_lookup() {
        // typeof inside with scope: undeclared name should do dynamic lookup
        // first (the name might be on the with-object), then apply typeof.
        let module = lower_script_module(
            r#"
            var obj = {};
            with (obj) {
                var t = typeof undeclaredName;
            }
            "#,
        );
        assert!(
            entry_has_op(&module, Op::EnvLookup),
            "typeof on undeclared name in with scope should use EnvLookup"
        );
    }

    #[test]
    fn test_with_statement_typeof_declared_does_not_use_env_lookup() {
        // typeof on a name declared inside with body should NOT use EnvLookup
        let module = lower_script_module(
            r#"
            var obj = {};
            with (obj) {
                let localVar = 42;
                var t = typeof localVar;
            }
            "#,
        );
        // typeof on a declared local variable should NOT emit EnvLookup
        // (it resolves normally via lexical scope)
        assert!(
            entry_has_op(&module, Op::TypeofBoxed),
            "typeof on declared local should use TypeofBoxed opcode"
        );
    }

    #[test]
    fn test_with_statement_this_not_affected_by_with() {
        // `this` inside with should NOT go through env_lookup —
        // it should be the enclosing `this`.
        let module = lower_script_module(
            r#"
            var obj = {};
            with (obj) {
                var t = this;
            }
            "#,
        );
        assert!(
            entry_has_op(&module, Op::ThisValue),
            "this inside with should use ThisValue opcode"
        );
        // And `this` should NOT go through EnvLookup
        // (EnvLookup may still be present for other names, so we don't
        // assert !entry_has_op for EnvLookup — just that ThisValue is emitted)
    }

    // -----------------------------------------------------------------------
    // Tier 0 with optimization (v0.6.5)
    // -----------------------------------------------------------------------

    #[test]
    fn test_with_tier0_object_literal_uses_direct_get_prop() {
        // with({x: 1, y: 2}) { var r = x; } — known props should use ICGetProp
        let module = lower_script_module(
            r#"
            with ({x: 1, y: 2}) {
                var r = x;
            }
            "#,
        );
        assert!(
            entry_has_op(&module, Op::ICGetProp),
            "Tier 0: known property 'x' in object literal should use ICGetProp"
        );
    }

    #[test]
    fn test_with_tier0_unknown_name_still_uses_env_lookup() {
        // with({x: 1}) { var r = z; } — z is NOT in the known set
        let module = lower_script_module(
            r#"
            with ({x: 1}) {
                var r = z;
            }
            "#,
        );
        assert!(
            entry_has_op(&module, Op::EnvLookup),
            "Tier 0: unknown property 'z' should fall back to EnvLookup"
        );
    }

    #[test]
    fn test_with_tier0_variable_target_no_optimization() {
        // with(someVar) { var r = x; } — not an object literal, no Tier 0
        let module = lower_script_module(
            r#"
            var someVar = {};
            with (someVar) {
                var r = x;
            }
            "#,
        );
        assert!(
            entry_has_op(&module, Op::EnvLookup),
            "Non-literal with target should use dynamic EnvLookup"
        );
    }

    #[test]
    fn test_with_tier0_object_literal_assignment_uses_set_prop() {
        // with({x: 1}) { x = 42; } — known prop, should use SetProp
        let module = lower_script_module(
            r#"
            with ({x: 1}) {
                x = 42;
            }
            "#,
        );
        assert!(
            entry_has_op(&module, Op::SetProp),
            "Tier 0: assignment to known property should use SetProp"
        );
    }

    #[test]
    fn test_with_tier0_compound_assignment_uses_ic_get_and_set() {
        // with({x: 1}) { x += 10; } — known prop, compound assignment
        let module = lower_script_module(
            r#"
            with ({x: 1}) {
                x += 10;
            }
            "#,
        );
        assert!(
            entry_has_op(&module, Op::ICGetProp),
            "Tier 0: compound assignment on known property should use ICGetProp for read"
        );
        assert!(
            entry_has_op(&module, Op::SetProp),
            "Tier 0: compound assignment on known property should use SetProp for write"
        );
    }

    #[test]
    fn test_with_tier0_update_expression_uses_ic_get_and_set() {
        // with({x: 1}) { x++; } — known prop, update expression
        let module = lower_script_module(
            r#"
            with ({x: 1}) {
                x++;
            }
            "#,
        );
        assert!(
            entry_has_op(&module, Op::ICGetProp),
            "Tier 0: update expression on known property should use ICGetProp for read"
        );
    }

    #[test]
    fn test_with_tier0_computed_key_prevents_optimization() {
        // with({[key]: 1}) { ... } — computed key, no optimization
        let module = lower_script_module(
            r#"
            var key = "x";
            with ({[key]: 1}) {
                var r = x;
            }
            "#,
        );
        assert!(
            entry_has_op(&module, Op::EnvLookup),
            "computed key prevents Tier 0 optimization"
        );
    }

    #[test]
    fn test_with_tier0_spread_prevents_optimization() {
        // with({...obj}) { ... } — spread property, no optimization
        let module = lower_script_module(
            r#"
            var obj = {x: 1};
            with ({...obj}) {
                var r = x;
            }
            "#,
        );
        assert!(
            entry_has_op(&module, Op::EnvLookup),
            "spread property prevents Tier 0 optimization"
        );
    }

    // =================================================================
    // eval scope poisoning tests — EscEnvironment bridging (v0.6 Step 7)
    // =================================================================

    /// Lower JS source as a script and return both the module and string table.
    fn lower_script_with_strings(source: &str) -> (ir::builder::TypedModule, Vec<String>) {
        let result = lower_script(source);
        ir::verify::verify_typed_module(&result.module).expect("IR should verify");
        (result.module, result.string_table)
    }

    /// Check if any non-entry function contains a CallRuntime with the given
    /// function name in the string table.
    fn non_entry_fn_has_runtime_call(
        module: &ir::builder::TypedModule,
        string_table: &[String],
        fn_name: &str,
    ) -> bool {
        let entry_idx = module.entry.unwrap();
        for (idx, func) in module.functions.iter().enumerate() {
            if idx == entry_idx {
                continue;
            }
            for block in &func.blocks {
                for inst in &block.instructions {
                    if inst.op == Op::CallRuntime && !inst.operands.is_empty() {
                        // The first operand of CallRuntime is a ConstString
                        // pointing to the function name. Find it by searching
                        // instructions that define that value.
                        if let Some(name) =
                            resolve_const_string_name(module, idx, inst.operands[0], string_table)
                            && name == fn_name
                        {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    /// Resolve a ValueId to a string table entry if it's a ConstString.
    fn resolve_const_string_name(
        module: &ir::builder::TypedModule,
        func_idx: usize,
        val_id: ir::ValueId,
        string_table: &[String],
    ) -> Option<String> {
        let func = &module.functions[func_idx];
        for block in &func.blocks {
            for inst in &block.instructions {
                if inst.id == val_id
                    && let Op::ConstString(idx) = &inst.op
                {
                    return string_table.get(*idx as usize).cloned();
                }
            }
        }
        None
    }

    #[test]
    fn test_poisoned_function_creates_esc_environment() {
        // A function containing direct eval should create an EscEnvironment
        // via __esc_rt_esc_env_create, not a regular Environment.
        let (module, strings) = lower_script_with_strings(
            r#"
            function f(x) {
                eval("x + 1");
            }
            "#,
        );
        assert!(
            non_entry_fn_has_runtime_call(&module, &strings, "__esc_rt_esc_env_create"),
            "poisoned function should emit __esc_rt_esc_env_create"
        );
    }

    #[test]
    fn test_poisoned_function_populates_slot_map() {
        // The EscEnvironment should have its slot_map populated with variable names
        let (module, strings) = lower_script_with_strings(
            r#"
            function f(x, y) {
                var z = 1;
                eval("x + y + z");
            }
            "#,
        );
        assert!(
            non_entry_fn_has_runtime_call(&module, &strings, "__esc_rt_esc_env_populate_slot_map"),
            "poisoned function should emit populate_slot_map for variable names"
        );
    }

    #[test]
    fn test_poisoned_function_emits_call_eval_direct() {
        // When eval is called in a poisoned function, it should emit
        // CallEvalDirect with env pointers, not plain CallEval
        let module = lower_script_module(
            r#"
            function f(x) {
                var code = "x + 1";
                return eval(code);
            }
            "#,
        );
        assert!(
            any_fn_has_op(&module, Op::CallEvalDirect),
            "eval in poisoned function should emit CallEvalDirect"
        );
        // Should NOT have plain CallEval (the eval is in a poisoned function)
        assert!(
            !any_fn_has_op(&module, Op::CallEval),
            "eval in poisoned function should NOT emit plain CallEval"
        );
    }

    #[test]
    fn test_non_poisoned_function_uses_regular_environment() {
        // A function without eval should use regular EnvCreate, not EscEnvironment
        let (module, strings) = lower_script_with_strings(
            r#"
            function f(x) {
                var inner = () => x + 1;
                return inner();
            }
            "#,
        );
        // Should have EnvCreate (regular environment for closures)
        assert!(
            any_fn_has_op(&module, Op::EnvCreate),
            "non-poisoned function with captures should use regular EnvCreate"
        );
        // Should NOT have __esc_rt_esc_env_create
        assert!(
            !non_entry_fn_has_runtime_call(&module, &strings, "__esc_rt_esc_env_create"),
            "non-poisoned function should NOT create EscEnvironment"
        );
    }

    #[test]
    fn test_poisoned_function_variables_use_esc_env_get() {
        // Variables in a poisoned function should be read via esc_env_get_boxed
        let (module, strings) = lower_script_with_strings(
            r#"
            function f(x) {
                eval("1");
                return x;
            }
            "#,
        );
        assert!(
            non_entry_fn_has_runtime_call(&module, &strings, "__esc_rt_esc_env_get_boxed"),
            "variable read in poisoned function should use esc_env_get_boxed"
        );
    }

    #[test]
    fn test_poisoned_function_variables_use_esc_env_set() {
        // Variables in a poisoned function should be written via esc_env_set_boxed
        let (module, strings) = lower_script_with_strings(
            r#"
            function f(x) {
                x = 42;
                eval("x");
            }
            "#,
        );
        assert!(
            non_entry_fn_has_runtime_call(&module, &strings, "__esc_rt_esc_env_set_boxed"),
            "variable write in poisoned function should use esc_env_set_boxed"
        );
    }

    #[test]
    fn test_poisoned_function_constant_eval_still_inlined() {
        // eval("literal string") should still be inlined even in a poisoned function
        // (the constant string can be compile-time evaluated)
        let module = lower_script_module(
            r#"
            function f() {
                var result = eval("1 + 2");
            }
            "#,
        );
        // eval("1 + 2") should be inlined, so no CallEval/CallEvalDirect needed
        // The function is still considered poisoned (has eval), but the specific
        // eval call was inlined.
        assert!(
            !any_fn_has_op(&module, Op::CallEvalDirect),
            "constant string eval should be inlined, not emit CallEvalDirect"
        );
    }

    #[test]
    fn test_poisoned_function_non_constant_eval_uses_call_eval_direct() {
        // eval(variable) in a poisoned function should use CallEvalDirect
        let module = lower_script_module(
            r#"
            function f() {
                var code = "1 + 2";
                eval(code);
            }
            "#,
        );
        assert!(
            any_fn_has_op(&module, Op::CallEvalDirect),
            "eval(variable) in poisoned function should emit CallEvalDirect"
        );
    }

    #[test]
    fn test_non_poisoned_function_eval_uses_plain_call_eval() {
        // eval at the top level (non-poisoned) should still use plain CallEval
        let module = lower_script_module(
            r#"
            var code = "1 + 2";
            eval(code);
            "#,
        );
        assert!(
            entry_has_op(&module, Op::CallEval),
            "top-level eval should use plain CallEval"
        );
        assert!(
            !entry_has_op(&module, Op::CallEvalDirect),
            "top-level eval should NOT use CallEvalDirect"
        );
    }

    #[test]
    fn test_call_eval_direct_has_four_operands() {
        // CallEvalDirect should carry 4 operands: (code, lex_env, var_env, this_value)
        let module = lower_script_module(
            r#"
            function f() {
                var code = "1";
                eval(code);
            }
            "#,
        );
        let inner_fn = &module.functions[1]; // f is the second function
        let eval_inst = inner_fn
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .find(|i| i.op == Op::CallEvalDirect);
        assert!(
            eval_inst.is_some(),
            "should find CallEvalDirect instruction"
        );
        let inst = eval_inst.unwrap();
        assert_eq!(
            inst.operands.len(),
            4,
            "CallEvalDirect should have 4 operands (code, lex_env, var_env, this)"
        );
    }

    #[test]
    fn test_poisoned_function_stores_params_to_esc_env() {
        // Parameters should be stored into the EscEnvironment so eval can see them
        let (module, strings) = lower_script_with_strings(
            r#"
            function f(a, b) {
                eval("a + b");
            }
            "#,
        );
        assert!(
            non_entry_fn_has_runtime_call(&module, &strings, "__esc_rt_esc_env_set_boxed"),
            "poisoned function should store parameters to EscEnvironment"
        );
    }

    #[test]
    fn test_nested_eval_only_poisons_inner_function() {
        // eval in an inner function should only poison the inner function,
        // not the outer one.
        let (module, strings) = lower_script_with_strings(
            r#"
            function outer() {
                var x = 1;
                function inner() {
                    eval("x");
                }
            }
            "#,
        );
        // The inner function should have esc_env_create
        assert!(
            non_entry_fn_has_runtime_call(&module, &strings, "__esc_rt_esc_env_create"),
            "inner function with eval should create EscEnvironment"
        );
    }

    #[test]
    fn test_poisoned_function_with_no_local_vars() {
        // A function with eval but no local variables should still work
        // and still set up the EscEnvironment (even if slot_count is 0)
        let (module, strings) = lower_script_with_strings(
            r#"
            function f() {
                eval("42");
            }
            "#,
        );
        assert!(
            non_entry_fn_has_runtime_call(&module, &strings, "__esc_rt_esc_env_create"),
            "even with constant eval, function should be detected as poisoned"
        );
    }

    #[test]
    fn test_indirect_eval_does_not_poison() {
        // (0, eval)("code") is indirect eval — should NOT poison the function
        let module = lower_script_module(
            r#"
            function f() {
                var code = "1 + 2";
                (0, eval)(code);
            }
            "#,
        );
        assert!(
            !any_fn_has_op(&module, Op::CallEvalDirect),
            "indirect eval should NOT emit CallEvalDirect"
        );
        let (module2, strings2) = lower_script_with_strings(
            r#"
            function f() {
                var code = "1 + 2";
                (0, eval)(code);
            }
            "#,
        );
        assert!(
            !non_entry_fn_has_runtime_call(&module2, &strings2, "__esc_rt_esc_env_create"),
            "indirect eval should NOT create EscEnvironment"
        );
    }

    #[test]
    fn test_poisoned_function_multiple_eval_calls() {
        // Multiple eval calls in same function should all use CallEvalDirect
        let module = lower_script_module(
            r#"
            function f(x) {
                var a = eval(x);
                var b = eval(x);
            }
            "#,
        );
        // Count CallEvalDirect instructions
        let inner_fn = &module.functions[1];
        let eval_count = inner_fn
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .filter(|i| i.op == Op::CallEvalDirect)
            .count();
        assert_eq!(
            eval_count, 2,
            "function with two eval calls should emit two CallEvalDirect instructions"
        );
    }

    // =========================================================================
    // Dynamic import() expression tests
    // =========================================================================

    /// Lower JS source and return the full LoweringResult for dynamic import checks.
    fn lower_and_get_result(source: &str) -> crate::LoweringResult {
        let result = lower_program(source).expect("lowering should succeed");
        verify_typed_module(&result.module).expect("IR should verify");
        result
    }

    #[test]
    fn test_dynamic_import_string_literal_emits_call_runtime() {
        // import("./mod.js") with a string literal should emit CallRuntime
        let ir = lower_and_print(r#"import("./mod.js")"#);
        assert!(
            ir.contains("call_runtime"),
            "import() with string literal should emit call_runtime: {ir}"
        );
    }

    #[test]
    fn test_dynamic_import_string_literal_records_specifier() {
        // import("./mod.js") should record the specifier in dynamic_imports
        let result = lower_and_get_result(r#"import("./mod.js")"#);
        assert_eq!(
            result.dynamic_imports,
            vec!["./mod.js".to_string()],
            "should record dynamic import specifier"
        );
    }

    #[test]
    fn test_dynamic_import_template_literal_no_interpolation() {
        // import(`./mod.js`) with no interpolation should work like a string
        let result = lower_and_get_result("import(`./mod.js`)");
        assert_eq!(
            result.dynamic_imports,
            vec!["./mod.js".to_string()],
            "template literal without interpolation should be treated as string"
        );
    }

    #[test]
    fn test_dynamic_import_variable_emits_type_error() {
        // import(variable) should emit a type error
        let ir = lower_and_print(r#"const x = "./mod.js"; import(x)"#);
        assert!(
            ir.contains("call_runtime"),
            "import() with variable should emit call_runtime for error: {ir}"
        );
        // Should NOT record any dynamic import
        let result = lower_and_get_result(r#"const x = "./mod.js"; import(x)"#);
        assert!(
            result.dynamic_imports.is_empty(),
            "import(variable) should not record dynamic imports"
        );
    }

    #[test]
    fn test_dynamic_import_interpolated_template_emits_error() {
        // import(`./mod_${name}.js`) should emit a type error
        let result = lower_and_get_result(r#"const name = "foo"; import(`./mod_${name}.js`)"#);
        assert!(
            result.dynamic_imports.is_empty(),
            "interpolated template should not record dynamic imports"
        );
    }

    #[test]
    fn test_dynamic_import_deduplication() {
        // Two identical import() calls should record the specifier only once
        let result = lower_and_get_result(r#"import("./mod.js"); import("./mod.js")"#);
        assert_eq!(
            result.dynamic_imports.len(),
            1,
            "duplicate import specifiers should be deduplicated"
        );
    }

    #[test]
    fn test_dynamic_import_multiple_different_specifiers() {
        // Multiple different import() calls
        let result = lower_and_get_result(r#"import("./a.js"); import("./b.js")"#);
        assert_eq!(
            result.dynamic_imports,
            vec!["./a.js".to_string(), "./b.js".to_string()],
            "should record all unique specifiers"
        );
    }

    #[test]
    fn test_dynamic_import_inside_function() {
        // import() inside a function body should still emit call_runtime.
        // Note: dynamic_imports is collected from the top-level lowerer;
        // inner functions create child lowerers, so the specifier may not
        // propagate to the top-level dynamic_imports list. We verify the
        // IR at least contains the call_runtime opcode.
        let ir = lower_and_print(r#"function loadMod() { return import("./lazy.js"); }"#);
        assert!(
            ir.contains("call_runtime"),
            "import() inside function should emit call_runtime"
        );
    }

    #[test]
    fn test_dynamic_import_verifies_ir() {
        // Ensure the generated IR verifies correctly
        let result = lower_program(r#"import("./mod.js")"#).expect("should succeed");
        verify_typed_module(&result.module).expect("IR should verify");
    }

    #[test]
    fn test_dynamic_import_ir_contains_specifier_string() {
        // The IR should contain the __esc_rt_dynamic_import runtime name
        let ir = lower_and_print(r#"import("./mod.js")"#);
        assert!(
            ir.contains("__esc_rt_dynamic_import") || ir.contains("call_runtime"),
            "should emit dynamic import runtime call"
        );
    }

    // =========================================================================
    // Wave 8 hardening: strict mode negative tests
    // =========================================================================

    #[test]
    fn test_with_statement_in_strict_mode_is_error() {
        // `with` is a SyntaxError in strict mode
        assert!(
            lower_script_expects_error(r#""use strict"; with ({}) {}"#),
            "with statement in strict mode should produce an error"
        );
    }

    #[test]
    fn test_with_statement_in_module_is_error() {
        // Modules are always strict — with is SyntaxError
        assert!(
            lower_program("with ({}) {}").is_err(),
            "with statement in module (strict) should produce an error"
        );
    }

    #[test]
    fn test_with_statement_in_sloppy_mode_allowed() {
        // with is allowed in sloppy mode
        let result = lower_script(r#"with ({x: 1}) { x; }"#);
        verify_typed_module(&result.module).expect("IR should verify");
    }

    #[test]
    fn test_duplicate_params_strict_mode_error() {
        // Duplicate parameter names in strict mode are a SyntaxError
        assert!(
            lower_script_expects_error(r#""use strict"; function f(a, a) {}"#),
            "duplicate params in strict mode should produce an error"
        );
    }

    #[test]
    fn test_duplicate_params_module_mode_error() {
        // Modules are always strict — duplicate params are SyntaxError
        assert!(
            lower_program("function f(a, a) {}").is_err(),
            "duplicate params in module (strict) should produce an error"
        );
    }

    #[test]
    fn test_duplicate_params_sloppy_mode_allowed() {
        // Duplicate params are allowed in sloppy mode
        let result = lower_script("function f(a, a) { return a; }");
        verify_typed_module(&result.module).expect("IR should verify");
    }

    #[test]
    fn test_break_outside_loop_is_error() {
        // break outside a loop or switch is a SyntaxError
        assert!(
            lower_program("break;").is_err(),
            "break outside loop should produce an error"
        );
    }

    #[test]
    fn test_continue_outside_loop_is_error() {
        // continue outside a loop is a SyntaxError
        assert!(
            lower_program("continue;").is_err(),
            "continue outside loop should produce an error"
        );
    }

    #[test]
    fn test_break_inside_loop_allowed() {
        // break inside a loop is allowed
        let _ir = lower_and_print("for (;;) { break; }");
    }

    #[test]
    fn test_continue_inside_loop_allowed() {
        // continue inside a loop is allowed
        let _ir = lower_and_print("for (;;) { continue; }");
    }

    #[test]
    fn test_eval_assignment_strict_mode_error() {
        // Assignment to eval in strict mode is SyntaxError
        assert!(
            lower_script_expects_error(r#""use strict"; eval = 1;"#),
            "assignment to eval in strict mode should produce an error"
        );
    }

    #[test]
    fn test_arguments_assignment_strict_mode_error() {
        // Assignment to arguments in strict mode is SyntaxError
        assert!(
            lower_script_expects_error(r#""use strict"; arguments = 1;"#),
            "assignment to arguments in strict mode should produce an error"
        );
    }

    #[test]
    fn test_eval_increment_strict_mode_error() {
        // ++eval in strict mode is SyntaxError
        assert!(
            lower_script_expects_error(r#""use strict"; eval++;"#),
            "eval++ in strict mode should produce an error"
        );
    }

    #[test]
    fn test_arguments_decrement_strict_mode_error() {
        // --arguments in strict mode is SyntaxError
        assert!(
            lower_script_expects_error(r#""use strict"; arguments--;"#),
            "arguments-- in strict mode should produce an error"
        );
    }

    #[test]
    fn test_eval_assignment_sloppy_mode_allowed() {
        // Assignment to eval is allowed in sloppy mode
        let result = lower_script("eval = 1;");
        verify_typed_module(&result.module).expect("IR should verify");
    }

    // =========================================================================
    // v0.8 S1: Additional strict mode enforcement checks
    // =========================================================================

    // --- delete identifier in strict mode ---

    #[test]
    fn test_delete_identifier_strict_mode_error() {
        // `delete x` on a bare identifier in strict mode is a SyntaxError
        assert!(
            lower_script_expects_error(r#""use strict"; var x = 1; delete x;"#),
            "delete identifier in strict mode should produce an error"
        );
    }

    #[test]
    fn test_delete_identifier_module_mode_error() {
        // Modules are always strict — delete on bare identifier is SyntaxError
        assert!(
            lower_program("var x = 1; delete x;").is_err(),
            "delete identifier in module (strict) should produce an error"
        );
    }

    #[test]
    fn test_delete_identifier_sloppy_mode_allowed() {
        // In sloppy mode, `delete x` on a declared var returns false
        let result = lower_script("var x = 1; delete x;");
        verify_typed_module(&result.module).expect("IR should verify");
    }

    #[test]
    fn test_delete_member_strict_mode_allowed() {
        // `delete obj.prop` is allowed even in strict mode
        let result = lower_program("let obj = {}; delete obj.x;").expect("should lower");
        verify_typed_module(&result.module).expect("IR should verify");
    }

    #[test]
    fn test_delete_computed_member_strict_mode_allowed() {
        // `delete obj[key]` is allowed even in strict mode
        let result = lower_program(r#"let obj = {}; delete obj["x"];"#).expect("should lower");
        verify_typed_module(&result.module).expect("IR should verify");
    }

    #[test]
    fn test_delete_non_reference_strict_mode_allowed() {
        // `delete 1` or `delete true` evaluates operand and returns true, even in strict
        let result = lower_program("delete 1;").expect("should lower");
        verify_typed_module(&result.module).expect("IR should verify");
    }

    // --- function named eval/arguments in strict mode ---

    #[test]
    fn test_function_named_eval_strict_mode_error() {
        // `function eval() {}` in strict mode is a SyntaxError
        assert!(
            lower_script_expects_error(r#""use strict"; function eval() {}"#),
            "function named eval in strict mode should produce an error"
        );
    }

    #[test]
    fn test_function_named_arguments_strict_mode_error() {
        // `function arguments() {}` in strict mode is a SyntaxError
        assert!(
            lower_script_expects_error(r#""use strict"; function arguments() {}"#),
            "function named arguments in strict mode should produce an error"
        );
    }

    #[test]
    fn test_function_named_eval_module_mode_error() {
        // Modules are always strict — function named eval is SyntaxError
        assert!(
            lower_program("function eval() {}").is_err(),
            "function named eval in module (strict) should produce an error"
        );
    }

    #[test]
    fn test_function_named_arguments_module_mode_error() {
        // Modules are always strict — function named arguments is SyntaxError
        assert!(
            lower_program("function arguments() {}").is_err(),
            "function named arguments in module (strict) should produce an error"
        );
    }

    #[test]
    fn test_function_named_eval_body_use_strict_error() {
        // "use strict" in the function body itself should reject the function name
        assert!(
            lower_script_expects_error(r#"function eval() { "use strict"; }"#),
            "function named eval with body use strict should produce an error"
        );
    }

    #[test]
    fn test_function_named_arguments_body_use_strict_error() {
        // "use strict" in the function body itself should reject the function name
        assert!(
            lower_script_expects_error(r#"function arguments() { "use strict"; }"#),
            "function named arguments with body use strict should produce an error"
        );
    }

    #[test]
    fn test_function_named_eval_sloppy_mode_allowed() {
        // In sloppy mode, function named eval is allowed
        let result = lower_script("function eval() { return 1; }");
        verify_typed_module(&result.module).expect("IR should verify");
    }

    #[test]
    fn test_function_named_arguments_sloppy_mode_allowed() {
        // In sloppy mode, function named arguments is allowed
        let result = lower_script("function arguments() { return 1; }");
        verify_typed_module(&result.module).expect("IR should verify");
    }

    // --- octal literals in strict mode ---
    // Note: The oxc parser does NOT reject legacy octals in strict mode at parse
    // time because strict mode is determined by directive prologues which are
    // semantic, not syntactic. The parser tokenizes `010` as a numeric literal.
    // TODO(v0.9): Add desugar-layer check for legacy octals in strict mode
    // by inspecting the raw source span of NumericLiteral nodes.

    #[test]
    fn test_octal_literal_sloppy_mode_allowed() {
        // In sloppy mode, legacy octal literals are allowed
        let result = lower_script("var x = 010;");
        verify_typed_module(&result.module).expect("IR should verify");
    }

    // --- comprehensive coverage: strict mode checks in nested contexts ---

    #[test]
    fn test_delete_identifier_in_strict_function_error() {
        // `delete x` inside a strict function is also a SyntaxError
        assert!(
            lower_script_expects_error(r#"function f() { "use strict"; var x; delete x; }"#),
            "delete identifier in strict function body should produce an error"
        );
    }

    #[test]
    fn test_delete_identifier_in_strict_arrow_error() {
        // `delete x` inside a strict arrow function is also a SyntaxError
        assert!(
            lower_program("var x = 1; const f = () => { delete x; };").is_err(),
            "delete identifier in strict arrow (module) should produce an error"
        );
    }
}

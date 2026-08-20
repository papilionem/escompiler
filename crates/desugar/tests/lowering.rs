use desugar::{globals, lower_program, lower_script};
use ir::printer::print_typed_module;
use ir::types::Op;
use ir::verify::verify_typed_module;

fn lower_and_verify(source: &str) -> String {
    let result = lower_program(source).expect("lowering should succeed");
    verify_typed_module(&result.module).expect("IR should verify");
    print_typed_module(&result.module)
}

#[test]
fn test_lower_arithmetic() {
    let ir = lower_and_verify("let x = 1 + 2 * 3;");
    assert!(
        ir.contains("const_f64"),
        "numeric literals are f64 — a JS Number IS an f64 (R1-03)"
    );
    assert!(ir.contains("mul_js"), "should have mul_js");
    assert!(ir.contains("add_js"), "should have add_js");
}

#[test]
fn test_lower_function_call() {
    let ir = lower_and_verify("let r = foo(1, 2);");
    assert!(ir.contains("call"), "should have call instruction");
    assert!(
        ir.contains("const_f64"),
        "numeric literal arguments are f64 (R1-03)"
    );
}

#[test]
fn test_lower_if_else() {
    let ir = lower_and_verify(
        r#"
        let x = 1;
        if (x) {
            let a = 2;
        } else {
            let b = 3;
        }
        "#,
    );
    assert!(ir.contains("to_bool"), "should have to_bool for condition");
    assert!(ir.contains("br_if"), "should have conditional branch");
}

#[test]
fn test_lower_for_loop() {
    let ir = lower_and_verify(
        r#"
        for (let i = 0; i < 10; i++) {
            let x = i;
        }
        "#,
    );
    assert!(ir.contains("lt_js"), "should have lt_js comparison");
    assert!(ir.contains("br_if"), "should have conditional branch");
    // Check for unconditional branch (loop back-edge)
    assert!(
        ir.contains("br bb"),
        "should have unconditional branch for loop"
    );
}

#[test]
fn test_lower_while_loop() {
    let ir = lower_and_verify(
        r#"
        let x = 10;
        while (x) {
            x = x - 1;
        }
        "#,
    );
    assert!(ir.contains("to_bool"), "should have to_bool for condition");
    assert!(ir.contains("br_if"), "should have conditional branch");
    assert!(ir.contains("sub_js"), "should have sub_js");
}

#[test]
fn test_lower_function_decl() {
    let ir = lower_and_verify(
        r#"
        function add(a, b) {
            return a + b;
        }
        "#,
    );
    // Should have at least 2 functions: main + add
    let func_count = ir.matches("fn @").count();
    assert!(
        func_count >= 2,
        "should have at least 2 functions, got {func_count}"
    );
    assert!(ir.contains("add_js"), "should have add_js in function body");
    assert!(ir.contains("ret"), "should have return");
}

#[test]
fn test_lower_arrow_function() {
    let ir = lower_and_verify("let f = (x) => x + 1;");
    let func_count = ir.matches("fn @").count();
    assert!(
        func_count >= 2,
        "should have at least 2 functions (main + arrow)"
    );
    assert!(ir.contains("add_js"), "should have add_js in arrow body");
}

#[test]
fn test_lower_try_catch() {
    let ir = lower_and_verify(
        r#"
        try {
            let x = 1;
        } catch (e) {
            let y = e;
        }
        "#,
    );
    assert!(ir.contains("try_begin"), "should have try_begin");
    assert!(ir.contains("try_end"), "should have try_end");
    assert!(ir.contains("catch"), "should have catch");
}

#[test]
fn test_lower_class() {
    let ir = lower_and_verify(
        r#"
        class Foo {
            constructor() {
                return undefined;
            }
            greet() {
                return 42;
            }
        }
        "#,
    );
    let func_count = ir.matches("fn @").count();
    assert!(
        func_count >= 3,
        "should have at least 3 functions (main + constructor + method), got {func_count}"
    );
    assert!(
        ir.contains("create_object"),
        "should have create_object for prototype"
    );
}

#[test]
fn test_lower_template_literal() {
    let ir = lower_and_verify(r#"let s = `hello ${42} world`;"#);
    assert!(
        ir.contains("const_string"),
        "should have const_string for template parts"
    );
    assert!(
        ir.contains("to_js_string") || ir.contains("string_concat"),
        "should have string operations"
    );
}

// === Console / globals tests ===

#[test]
fn test_console_log_emits_call_runtime() {
    let result = lower_program("console.log('hello');").expect("lowering should succeed");
    let module = &result.module;
    let entry_fn = &module.functions[module.entry.unwrap()];
    let has_call_runtime = entry_fn.blocks.iter().any(|block| {
        block
            .instructions
            .iter()
            .any(|inst| matches!(inst.op, Op::CallRuntime))
    });
    assert!(has_call_runtime, "console.log should emit CallRuntime");
}

#[test]
fn test_console_error_emits_call_runtime() {
    let result = lower_program("console.error('oops');").expect("lowering should succeed");
    let module = &result.module;
    let entry_fn = &module.functions[module.entry.unwrap()];
    let has_call_runtime = entry_fn.blocks.iter().any(|block| {
        block
            .instructions
            .iter()
            .any(|inst| matches!(inst.op, Op::CallRuntime))
    });
    assert!(has_call_runtime, "console.error should emit CallRuntime");
}

#[test]
fn test_console_warn_emits_call_runtime() {
    let result = lower_program("console.warn('careful');").expect("lowering should succeed");
    let module = &result.module;
    let entry_fn = &module.functions[module.entry.unwrap()];
    let has_call_runtime = entry_fn.blocks.iter().any(|block| {
        block
            .instructions
            .iter()
            .any(|inst| matches!(inst.op, Op::CallRuntime))
    });
    assert!(has_call_runtime, "console.warn should emit CallRuntime");
}

#[test]
fn test_regular_call_emits_call_not_call_runtime() {
    // Use a declared function to verify that Call (not CallRuntime) is emitted.
    // Undeclared identifiers now emit CallRuntime(__esc_rt_throw_reference_error),
    // which is the correct JS behaviour (ReferenceError at runtime).
    let result = lower_program("function foo() {} foo();").expect("lowering should succeed");
    let module = &result.module;
    let entry_fn = &module.functions[module.entry.unwrap()];
    let has_call = entry_fn.blocks.iter().any(|block| {
        block
            .instructions
            .iter()
            .any(|inst| matches!(inst.op, Op::Call))
    });
    assert!(has_call, "regular calls should emit Call");
}

#[test]
fn test_console_log_string_in_table() {
    let result = lower_program("console.log('hello');").expect("lowering should succeed");
    assert!(
        result
            .string_table
            .iter()
            .any(|s| s.contains("console_log")),
        "string table should contain console runtime name, got: {:?}",
        result.string_table
    );
}

#[test]
fn test_console_log_multiple_args() {
    let ir = lower_and_verify("console.log('a', 42, true);");
    assert!(
        ir.contains("call_runtime"),
        "should have call_runtime for console.log"
    );
    assert!(
        ir.contains("const_string"),
        "should have const_string for 'a'"
    );
    assert!(
        ir.contains("const_f64"),
        "42 lowers to const_f64, not const_i32 (R1-03)"
    );
}

#[test]
fn test_non_console_member_call_no_call_runtime() {
    let ir = lower_and_verify("Math.random();");
    // Math.random is not recognized — should be generic get_prop + call
    assert!(
        !ir.contains("call_runtime"),
        "Math.random should not emit call_runtime"
    );
}

#[test]
fn test_console_log_ir_verifies() {
    // Ensure the generated IR passes verification
    lower_and_verify("console.log('hello', 42);");
}

// === A1: Bug fixes ===

#[test]
fn test_logical_not_negation() {
    let ir = lower_and_verify("let x = !true;");
    assert!(ir.contains("to_bool"), "logical not should emit to_bool");
    assert!(
        ir.contains("eq_strict"),
        "logical not should emit eq_strict(bool, false) for negation"
    );
    assert!(
        ir.contains("const_bool false"),
        "logical not should have const_bool(false) for comparison"
    );
}

#[test]
fn test_logical_not_on_variable() {
    let ir = lower_and_verify("let x = 1; let y = !x;");
    assert!(ir.contains("to_bool"), "should convert to boolean");
    assert!(ir.contains("eq_strict"), "should negate via eq_strict");
}

#[test]
fn test_delete_on_member_expression() {
    let ir = lower_and_verify("let obj = {}; delete obj.prop;");
    assert!(
        ir.contains("delete_prop"),
        "delete obj.prop should emit delete_prop"
    );
}

#[test]
fn test_delete_computed_member() {
    let ir = lower_and_verify("let obj = {}; let k = 'x'; delete obj[k];");
    assert!(
        ir.contains("delete_prop"),
        "delete obj[k] should emit delete_prop"
    );
}

#[test]
fn test_delete_non_member_returns_true() {
    // In sloppy mode, `delete x` on a declared var returns false (cannot delete).
    // In strict mode (module), `delete x` is a SyntaxError — tested separately.
    let result = lower_script("var x = 1; delete x;").expect("lowering should succeed");
    verify_typed_module(&result.module).expect("IR should verify");
    let ir = print_typed_module(&result.module);
    assert!(
        ir.contains("const_bool false"),
        "delete on declared var in sloppy mode should emit const_bool(false), got:\n{ir}"
    );
}

#[test]
fn test_delete_identifier_strict_mode_is_error() {
    // In module mode (always strict), `delete identifier` is a SyntaxError
    assert!(
        lower_program("var x = 1; delete x;").is_err(),
        "delete identifier in module (strict) mode should produce an error"
    );
}

// === A2: Destructuring ===

#[test]
fn test_object_destructuring_basic() {
    let ir = lower_and_verify("let obj = {}; const { a, b } = obj;");
    // Should emit get_prop for each key
    let get_prop_count = ir.matches("get_prop").count();
    assert!(
        get_prop_count >= 2,
        "object destructuring should emit at least 2 get_prop, got {get_prop_count}"
    );
}

#[test]
fn test_array_destructuring_basic() {
    let ir = lower_and_verify("const arr = [1, 2, 3]; const [a, b, c] = arr;");
    // Should emit get_elem for each index
    let get_elem_count = ir.matches("get_elem").count();
    assert!(
        get_elem_count >= 3,
        "array destructuring should emit at least 3 get_elem, got {get_elem_count}"
    );
}

#[test]
fn test_nested_object_destructuring() {
    let ir = lower_and_verify("const obj = { a: { b: 1 } }; const { a: { b } } = obj;");
    // Should have multiple get_prop calls for nested access
    let get_prop_count = ir.matches("get_prop").count();
    assert!(
        get_prop_count >= 2,
        "nested destructuring should emit multiple get_prop, got {get_prop_count}"
    );
}

#[test]
fn test_destructuring_with_default() {
    let ir = lower_and_verify("const { a = 10 } = {};");
    assert!(
        ir.contains("eq_strict"),
        "destructuring default should check for undefined via eq_strict"
    );
    assert!(
        ir.contains("const_undefined"),
        "destructuring default should compare against undefined"
    );
    assert!(
        ir.contains("br_if"),
        "destructuring default should have conditional branch"
    );
}

#[test]
fn test_array_destructuring_with_default() {
    let ir = lower_and_verify("const [a = 5] = [];");
    assert!(
        ir.contains("get_elem"),
        "array destructuring should emit get_elem"
    );
    assert!(
        ir.contains("eq_strict"),
        "array default should check for undefined via eq_strict"
    );
}

// === A3: Closure & function expressions ===

#[test]
fn test_function_expression_emits_create_closure() {
    let ir = lower_and_verify("let f = function() { return 1; };");
    assert!(
        ir.contains("create_closure"),
        "function expression should emit create_closure"
    );
}

#[test]
fn test_arrow_function_emits_create_closure() {
    let ir = lower_and_verify("let f = () => 42;");
    assert!(
        ir.contains("create_closure"),
        "arrow function should emit create_closure"
    );
}

#[test]
fn test_arrow_captures_this() {
    let ir = lower_and_verify("let f = () => this;");
    assert!(
        ir.contains("this_value"),
        "arrow function should capture this for create_closure env"
    );
    assert!(
        ir.contains("create_closure"),
        "arrow should emit create_closure"
    );
}

// === A4: Class lowering improvements ===

#[test]
fn test_class_methods_emit_create_closure() {
    let ir = lower_and_verify(
        r#"
        class Foo {
            greet() { return 42; }
        }
        "#,
    );
    assert!(
        ir.contains("create_closure"),
        "class methods should emit create_closure"
    );
}

#[test]
fn test_class_extends_emits_get_prop_prototype() {
    let ir = lower_and_verify(
        r#"
        class Base {}
        class Derived extends Base {}
        "#,
    );
    // Should access Base.prototype to set up the chain
    assert!(
        ir.contains("get_prop"),
        "class extends should emit get_prop for prototype access"
    );
}

#[test]
fn test_class_static_method() {
    let ir = lower_and_verify(
        r#"
        class Foo {
            static bar() { return 1; }
        }
        "#,
    );
    // Static methods should be set on the constructor closure, not prototype.
    // The method itself should be lowered as a function.
    let func_count = ir.matches("fn @").count();
    assert!(
        func_count >= 2,
        "should have at least 2 functions (main + static method)"
    );
    assert!(
        ir.contains("create_closure"),
        "static method should emit create_closure"
    );
}

// === A5: Missing expression lowering ===

#[test]
fn test_compound_assignment_add() {
    let ir = lower_and_verify("let x = 1; x += 2;");
    assert!(ir.contains("add_js"), "compound += should emit add_js");
}

#[test]
fn test_compound_assignment_sub() {
    let ir = lower_and_verify("let x = 10; x -= 3;");
    assert!(ir.contains("sub_js"), "compound -= should emit sub_js");
}

#[test]
fn test_compound_assignment_mul() {
    let ir = lower_and_verify("let x = 5; x *= 2;");
    assert!(ir.contains("mul_js"), "compound *= should emit mul_js");
}

#[test]
fn test_compound_assignment_on_member() {
    let ir = lower_and_verify("let obj = {}; obj.x = 1; obj.x += 2;");
    assert!(
        ir.contains("get_prop"),
        "compound assignment on member should read property"
    );
    assert!(
        ir.contains("add_js"),
        "compound += on member should emit add_js"
    );
    assert!(
        ir.contains("set_prop"),
        "compound assignment on member should write property"
    );
}

#[test]
fn test_optional_chaining_member() {
    let ir = lower_and_verify("let obj = {}; let x = obj?.prop;");
    assert!(
        ir.contains("is_nullish"),
        "optional chaining should check for nullish"
    );
    assert!(
        ir.contains("br_if"),
        "optional chaining should have conditional branch"
    );
    assert!(
        ir.contains("get_prop"),
        "optional chaining should emit get_prop in non-null branch"
    );
}

#[test]
fn test_optional_chaining_call() {
    let ir = lower_and_verify("let fn1 = null; let x = fn1?.();");
    assert!(
        ir.contains("is_nullish"),
        "optional call should check for nullish"
    );
}

#[test]
fn test_default_parameter() {
    let ir = lower_and_verify("function f(x = 10) { return x; }");
    assert!(
        ir.contains("eq_strict"),
        "default parameter should check for undefined via eq_strict"
    );
    assert!(
        ir.contains("br_if"),
        "default parameter should have conditional"
    );
}

// === A6: Import/export ===

#[test]
fn test_import_declaration() {
    let ir = lower_and_verify("import { foo } from 'bar';");
    assert!(
        ir.contains("call_runtime"),
        "import should emit call_runtime for module loading"
    );
    assert!(
        ir.contains("get_prop"),
        "import should emit get_prop for import specifier"
    );
}

#[test]
fn test_import_default() {
    let ir = lower_and_verify("import myDefault from 'module';");
    assert!(
        ir.contains("call_runtime"),
        "import default should emit call_runtime"
    );
    assert!(
        ir.contains("get_prop"),
        "import default should emit get_prop for 'default'"
    );
}

#[test]
fn test_export_named_variable() {
    let ir = lower_and_verify("export const x = 42;");
    assert!(
        ir.contains("const_f64 42"),
        "export const should lower the variable normally"
    );
}

#[test]
fn test_export_default_expression() {
    let ir = lower_and_verify("export default 42;");
    assert!(
        ir.contains("const_f64 42"),
        "export default should lower the expression"
    );
}

// === A7: Error/edge cases ===

#[test]
fn test_empty_destructuring() {
    // Should not crash
    let ir = lower_and_verify("const {} = {};");
    assert!(
        ir.contains("create_object"),
        "empty destructuring should still create the object"
    );
}

#[test]
fn test_deeply_nested_destructuring() {
    let ir = lower_and_verify("const { a: { b: { c } } } = { a: { b: { c: 1 } } };");
    let get_prop_count = ir.matches("get_prop").count();
    assert!(
        get_prop_count >= 3,
        "deeply nested destructuring should emit at least 3 get_prop, got {get_prop_count}"
    );
}

#[test]
fn test_mixed_destructuring_and_defaults() {
    let ir = lower_and_verify("const { a = 1, b = 2 } = {};");
    let eq_strict_count = ir.matches("eq_strict").count();
    assert!(
        eq_strict_count >= 2,
        "each default should emit eq_strict (undefined check), got {eq_strict_count}"
    );
}

#[test]
fn test_double_logical_not() {
    let ir = lower_and_verify("let x = !!true;");
    // Double negation: should have two to_bool and two eq_strict
    let to_bool_count = ir.matches("to_bool").count();
    assert!(
        to_bool_count >= 2,
        "double logical not should emit at least 2 to_bool, got {to_bool_count}"
    );
}

#[test]
fn test_class_default_constructor() {
    // Class with no explicit constructor should still work
    let ir = lower_and_verify(
        r#"
        class Empty {
            method() { return 1; }
        }
        "#,
    );
    assert!(
        ir.contains("create_closure"),
        "class should emit create_closure even with default constructor"
    );
}

// === globals module unit tests ===

#[test]
fn test_is_console_method() {
    assert!(globals::is_console_method("console", "log"));
    assert!(globals::is_console_method("console", "error"));
    assert!(globals::is_console_method("console", "warn"));
    assert!(globals::is_console_method("console", "debug"));
    assert!(globals::is_console_method("console", "info"));
    assert!(globals::is_console_method("console", "trace"));
    assert!(!globals::is_console_method("console", "clear"));
    assert!(!globals::is_console_method("Math", "log"));
    assert!(!globals::is_console_method("", "log"));
}

#[test]
fn test_console_runtime_name() {
    assert_eq!(
        globals::console_runtime_name("log"),
        Some("__esc_rt_console_log")
    );
    assert_eq!(
        globals::console_runtime_name("info"),
        Some("__esc_rt_console_log")
    );
    assert_eq!(
        globals::console_runtime_name("trace"),
        Some("__esc_rt_console_log")
    );
    assert_eq!(
        globals::console_runtime_name("error"),
        Some("__esc_rt_console_error")
    );
    assert_eq!(
        globals::console_runtime_name("warn"),
        Some("__esc_rt_console_warn")
    );
    assert_eq!(
        globals::console_runtime_name("debug"),
        Some("__esc_rt_console_log")
    );
    assert_eq!(globals::console_runtime_name("clear"), None);
    assert_eq!(globals::console_runtime_name("table"), None);
}

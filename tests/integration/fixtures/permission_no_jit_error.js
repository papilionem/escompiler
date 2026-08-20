// @expect-error ESC-E401
// This test verifies that eval() usage with --no-jit flag is rejected.
// The compiler should produce ESC-E401 when eval appears with --no-jit flag.
// NOTE: This test will pass once the permission gate is implemented.
var code = "1 + 2";
var result = eval(code);
console.log(result);

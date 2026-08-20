// @expect-error ESC-E400
// This test verifies that eval() usage in strict-no-eval mode is rejected.
// The compiler should produce ESC-E400 when eval appears with --no-eval flag.
// NOTE: This test will pass once the permission gate is implemented.
"use strict";
var result = eval("1 + 2");
console.log(result);

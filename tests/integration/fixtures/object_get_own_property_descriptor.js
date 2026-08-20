// @expected-stdout: PASS
var obj = {x: 1};
var desc = Object.getOwnPropertyDescriptor(obj, "x");
if (desc.value !== 1) throw "FAIL: value";
console.log("PASS");

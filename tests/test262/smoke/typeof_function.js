/*---
description: typeof function is "function"
esid: sec-typeof-operator
---*/
function foo() {}
assert.sameValue(typeof foo, "function");

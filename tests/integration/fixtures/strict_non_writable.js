// @expected-stdout-begin
// caught
// 42
// @expected-stdout-end

// Strict mode assignment to non-writable should throw TypeError
"use strict";
let obj = {};
Object.defineProperty(obj, 'x', { value: 42, writable: false, enumerable: true, configurable: true });
try {
  obj.x = 99;
  console.log("not caught");
} catch (e) {
  console.log("caught");
}
console.log(obj.x);

// @expected-stdout: PASS
var obj = {};
Object.defineProperty(obj, "x", {value: 42, writable: false, enumerable: true, configurable: false});
if (obj.x !== 42) throw "FAIL: value";
console.log("PASS");

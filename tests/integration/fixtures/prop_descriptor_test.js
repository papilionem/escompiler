// @expected-stdout: PASS
// Test property descriptor checks
var obj = {};
Object.defineProperty(obj, "x", { value: 42, writable: false, enumerable: false, configurable: false });
var desc = Object.getOwnPropertyDescriptor(obj, "x");

var results = [];
results.push(desc.value === 42);
results.push(desc.writable === false);
results.push(desc.enumerable === false);
results.push(desc.configurable === false);

// Also test Math constant descriptors
var mathDesc = Object.getOwnPropertyDescriptor(Math, "PI");
results.push(typeof mathDesc === "object");
if (mathDesc) {
  results.push(mathDesc.writable === false);
  results.push(mathDesc.enumerable === false);
  results.push(mathDesc.configurable === false);
}

var allPass = results.every(function(r) { return r === true; });
if (allPass) {
  console.log("PASS");
} else {
  console.log("FAIL " + JSON.stringify(results));
}

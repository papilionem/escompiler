// @expected-stdout: PASS
// Test property descriptors on NativeFunc methods
var desc = Object.getOwnPropertyDescriptor(Math, "abs");
var results = [];
results.push(typeof desc === "object");
if (desc) {
  results.push(typeof desc.value === "function");
  results.push(desc.writable === true);
  results.push(desc.enumerable === false);
  results.push(desc.configurable === true);
}

// Test constant descriptor
var piDesc = Object.getOwnPropertyDescriptor(Math, "PI");
if (piDesc) {
  results.push(piDesc.value === Math.PI);
  results.push(piDesc.writable === false);
  results.push(piDesc.enumerable === false);
  results.push(piDesc.configurable === false);
}

// Test method .length descriptor
var absLenDesc = Object.getOwnPropertyDescriptor(Math.abs, "length");
if (absLenDesc) {
  results.push(absLenDesc.value === 1);
  results.push(absLenDesc.writable === false);
  results.push(absLenDesc.enumerable === false);
  results.push(absLenDesc.configurable === true);
}

var allPass = results.every(function(r) { return r === true; });
if (allPass && results.length >= 12) {
  console.log("PASS");
} else {
  console.log("FAIL len=" + results.length + " " + JSON.stringify(results));
}

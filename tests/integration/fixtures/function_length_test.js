// @expected-stdout: PASS
// Test builtin function .length property
var results = [];
results.push(Array.prototype.push.length === 1);
results.push(Array.prototype.map.length === 1);
results.push(Array.prototype.slice.length === 2);
results.push(Function.prototype.apply.length === 2);
results.push(Function.prototype.call.length === 1);
results.push(Function.prototype.bind.length === 1);
results.push(Object.keys.length === 1);
results.push(Object.defineProperty.length === 3);
results.push(JSON.parse.length === 2);
results.push(JSON.stringify.length === 3);

var allPass = results.every(function(r) { return r === true; });
if (allPass) {
  console.log("PASS");
} else {
  console.log("FAIL " + JSON.stringify(results));
}

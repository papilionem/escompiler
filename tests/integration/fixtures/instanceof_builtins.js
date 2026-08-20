// @expected-stdout: PASS
var results = [];
results.push([] instanceof Array);
results.push(new Date() instanceof Date);
results.push(new Map() instanceof Map);
results.push(new Set() instanceof Set);
results.push(new RegExp("a") instanceof RegExp);
results.push(new Error() instanceof Error);
results.push(new TypeError() instanceof TypeError);
results.push(new TypeError() instanceof Error);

var allPass = results.every(function(r) { return r === true; });
if (allPass) {
  console.log("PASS");
} else {
  console.log("FAIL " + JSON.stringify(results));
}

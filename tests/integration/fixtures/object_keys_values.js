// @expected-stdout: PASS
var obj = {a: 1, b: 2, c: 3};
var keys = Object.keys(obj);
if (keys.length !== 3) throw "FAIL: keys length";
var vals = Object.values(obj);
if (vals.length !== 3) throw "FAIL: values length";
console.log("PASS");

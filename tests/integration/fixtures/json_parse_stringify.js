// @expected-stdout: PASS
var obj = JSON.parse('{"a":1,"b":"hello"}');
if (obj.a !== 1) throw "FAIL: parse number";
if (obj.b !== "hello") throw "FAIL: parse string";
var str = JSON.stringify(obj);
if (typeof str !== "string") throw "FAIL: stringify type";
console.log("PASS");

// @expected-stdout-begin
// PASS
// @expected-stdout-end
var o = new Object();
o.x = 1;
if (o.x !== 1) throw "FAIL: new Object property";
var o2 = new Object();
o2.name = "test";
if (o2.name !== "test") throw "FAIL: new Object string property";
console.log("PASS");

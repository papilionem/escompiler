// @expected-stdout-begin
// PASS
// @expected-stdout-end
var a = new Array(3);
if (a.length !== 3) throw "FAIL: length " + a.length;
var b = new Array(1, 2, 3);
if (b.length !== 3) throw "FAIL: literal length " + b.length;
if (b[0] !== 1) throw "FAIL: b[0]";
if (b[1] !== 2) throw "FAIL: b[1]";
if (b[2] !== 3) throw "FAIL: b[2]";
var c = new Array();
if (c.length !== 0) throw "FAIL: empty " + c.length;
console.log("PASS");

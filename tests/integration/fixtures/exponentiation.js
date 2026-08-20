// @expected-stdout-begin
// PASS
// @expected-stdout-end
if (2 ** 3 !== 8) throw "FAIL: 2**3";
if (2 ** 0 !== 1) throw "FAIL: 2**0";
if (2 ** -1 !== 0.5) throw "FAIL: 2**-1";
var x = 3;
x **= 2;
if (x !== 9) throw "FAIL: **=";
if (10 ** 3 !== 1000) throw "FAIL: 10**3";
console.log("PASS");

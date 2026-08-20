// @expected-stdout-begin
// PASS
// @expected-stdout-end
if (typeof Math.E !== "number") throw "FAIL: E type";
if (typeof Math.LN2 !== "number") throw "FAIL: LN2 type";
if (typeof Math.PI !== "number") throw "FAIL: PI type";
if (typeof Math.SQRT2 !== "number") throw "FAIL: SQRT2 type";
var diff = Math.SQRT2 * Math.SQRT2 - 2;
if (diff < 0) diff = -diff;
if (diff > 0.0001) throw "FAIL: SQRT2 precision";
if (Math.PI < 3.14 || Math.PI > 3.15) throw "FAIL: PI value";
if (Math.E < 2.71 || Math.E > 2.72) throw "FAIL: E value";
console.log("PASS");

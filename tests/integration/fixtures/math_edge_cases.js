// @expected-stdout-begin
// PASS
// @expected-stdout-end

// Math.round edge cases
if (Math.round(3.5) !== 4) throw "FAIL: round(3.5)";
if (Math.round(3.4) !== 3) throw "FAIL: round(3.4)";
if (Math.round(-3.5) !== -3) throw "FAIL: round(-3.5)";
if (Math.round(-3.6) !== -4) throw "FAIL: round(-3.6)";

// Math.round(-0.5) must return -0
var r = Math.round(-0.5);
if (r !== 0 || (1/r) !== -Infinity) throw "FAIL: round(-0.5) should be -0, got " + r;

// Math.max/min with no arguments
if (Math.max() !== -Infinity) throw "FAIL: max()";
if (Math.min() !== Infinity) throw "FAIL: min()";

// Math.max/min with +0 and -0
var maxZ = Math.max(0, -0);
if (maxZ !== 0 || (1/maxZ) !== Infinity) throw "FAIL: max(0,-0) should be +0";
var minZ = Math.min(0, -0);
if (minZ !== 0 || (1/minZ) !== -Infinity) throw "FAIL: min(0,-0) should be -0";

// Math.max/min with NaN
if (!isNaN(Math.max(1, NaN, 3))) throw "FAIL: max with NaN";
if (!isNaN(Math.min(1, NaN, 3))) throw "FAIL: min with NaN";

// Math.hypot edge cases
if (Math.hypot() !== 0) throw "FAIL: hypot()";
if (Math.hypot(3, 4) !== 5) throw "FAIL: hypot(3,4)";
if (Math.hypot(Infinity, NaN) !== Infinity) throw "FAIL: hypot(Inf, NaN)";
if (Math.hypot(-Infinity, 1) !== Infinity) throw "FAIL: hypot(-Inf, 1)";
if (!isNaN(Math.hypot(NaN, 1))) throw "FAIL: hypot(NaN, 1)";

// Math.sign
if (Math.sign(5) !== 1) throw "FAIL: sign(5)";
if (Math.sign(-5) !== -1) throw "FAIL: sign(-5)";
if (Math.sign(0) !== 0) throw "FAIL: sign(0)";
if (!isNaN(Math.sign(NaN))) throw "FAIL: sign(NaN)";

// Math.trunc
if (Math.trunc(4.7) !== 4) throw "FAIL: trunc(4.7)";
if (Math.trunc(-4.7) !== -4) throw "FAIL: trunc(-4.7)";

console.log("PASS");

// @expected-stdout-begin
// PASS
// @expected-stdout-end
if (Number.isInteger(5) !== true) throw "FAIL: isInteger(5)";
if (Number.isInteger(5.5) !== false) throw "FAIL: isInteger(5.5)";
if (Number.isFinite(Infinity) !== false) throw "FAIL: isFinite(Inf)";
if (Number.isFinite(42) !== true) throw "FAIL: isFinite(42)";
if (Number.isNaN(NaN) !== true) throw "FAIL: isNaN(NaN)";
if (Number.isNaN(5) !== false) throw "FAIL: isNaN(5)";
if (Number.MAX_SAFE_INTEGER !== 9007199254740991) throw "FAIL: MAX_SAFE_INTEGER";
console.log("PASS");

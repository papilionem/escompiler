// @expected-stdout-begin
// PASS
// @expected-stdout-end

// toExponential basic
if ((123.456).toExponential(2) !== "1.23e+2") throw "FAIL: toExponential(2): " + (123.456).toExponential(2);
if ((0).toExponential(4) !== "0.0000e+0") throw "FAIL: 0.toExponential(4): " + (0).toExponential(4);
if ((77).toExponential(1) !== "7.7e+1") throw "FAIL: 77.toExponential(1): " + (77).toExponential(1);
if ((1).toExponential(2) !== "1.00e+0") throw "FAIL: 1.toExponential(2): " + (1).toExponential(2);

// toExponential with no fraction digits
if ((0).toExponential() !== "0e+0") throw "FAIL: 0.toExponential(): " + (0).toExponential();

// toExponential with NaN/Infinity
if (NaN.toExponential() !== "NaN") throw "FAIL: NaN.toExponential()";
if (Infinity.toExponential() !== "Infinity") throw "FAIL: Infinity.toExponential()";
if ((-Infinity).toExponential() !== "-Infinity") throw "FAIL: -Infinity.toExponential()";

// toExponential negative number
if ((-123.456).toExponential(2) !== "-1.23e+2") throw "FAIL: -123.456.toExponential(2): " + (-123.456).toExponential(2);

// toPrecision basic
if ((123.456).toPrecision(5) !== "123.46") throw "FAIL: toPrecision(5): " + (123.456).toPrecision(5);
if ((5).toPrecision(3) !== "5.00") throw "FAIL: 5.toPrecision(3): " + (5).toPrecision(3);
if ((0).toPrecision(4) !== "0.000") throw "FAIL: 0.toPrecision(4): " + (0).toPrecision(4);

// toPrecision exponential
if ((123456).toPrecision(2) !== "1.2e+5") throw "FAIL: 123456.toPrecision(2): " + (123456).toPrecision(2);
if ((123.456).toPrecision(1) !== "1e+2") throw "FAIL: 123.456.toPrecision(1): " + (123.456).toPrecision(1);

// toPrecision NaN/Infinity
if (NaN.toPrecision(2) !== "NaN") throw "FAIL: NaN.toPrecision(2)";
if (Infinity.toPrecision(2) !== "Infinity") throw "FAIL: Infinity.toPrecision(2)";

// toPrecision with no arg acts as toString
if ((42).toPrecision() !== "42") throw "FAIL: 42.toPrecision(): " + (42).toPrecision();

// Number.isSafeInteger should not coerce
if (Number.isSafeInteger(42) !== true) throw "FAIL: isSafeInteger(42)";
if (Number.isSafeInteger(9007199254740992) !== false) throw "FAIL: isSafeInteger(2^53)";
if (Number.isSafeInteger(true) !== false) throw "FAIL: isSafeInteger(true) should be false";
if (Number.isSafeInteger(null) !== false) throw "FAIL: isSafeInteger(null) should be false";

// toFixed edge cases
if ((3.14159).toFixed(2) !== "3.14") throw "FAIL: toFixed(2)";
if (NaN.toFixed(2) !== "NaN") throw "FAIL: NaN.toFixed(2)";
if (Infinity.toFixed(2) !== "Infinity") throw "FAIL: Infinity.toFixed(2)";

console.log("PASS");

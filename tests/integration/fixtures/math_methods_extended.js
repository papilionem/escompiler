// @expected-stdout-begin
// PASS
// @expected-stdout-end
if (Math.trunc(4.7) !== 4) throw "FAIL: trunc positive";
if (Math.trunc(-4.7) !== -4) throw "FAIL: trunc negative";
if (Math.sign(-5) !== -1) throw "FAIL: sign negative";
if (Math.sign(5) !== 1) throw "FAIL: sign positive";
if (Math.sign(0) !== 0) throw "FAIL: sign zero";
if (Math.log(1) !== 0) throw "FAIL: log(1)";
if (Math.exp(0) !== 1) throw "FAIL: exp(0)";
if (Math.cbrt(27) !== 3) throw "FAIL: cbrt";
if (Math.clz32(1) !== 31) throw "FAIL: clz32";
if (Math.imul(2, 3) !== 6) throw "FAIL: imul";
if (Math.log2(8) !== 3) throw "FAIL: log2";
if (Math.sin(0) !== 0) throw "FAIL: sin(0)";
if (Math.cos(0) !== 1) throw "FAIL: cos(0)";
if (Math.tan(0) !== 0) throw "FAIL: tan(0)";
console.log("PASS");

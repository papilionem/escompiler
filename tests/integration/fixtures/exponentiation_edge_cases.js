// @expected-stdout-begin
// PASS
// @expected-stdout-end

// ES2023 6.1.6.1.6 exponentiation edge cases

// NaN ** 0 === 1
if (NaN ** 0 !== 1) throw "FAIL: NaN ** 0 should be 1, got " + (NaN ** 0);

// Infinity ** 0 === 1
if (Infinity ** 0 !== 1) throw "FAIL: Infinity ** 0 should be 1, got " + (Infinity ** 0);

// (-Infinity) ** 0 === 1
if ((-Infinity) ** 0 !== 1) throw "FAIL: (-Infinity) ** 0 should be 1";

// 0 ** 0 === 1
if (0 ** 0 !== 1) throw "FAIL: 0 ** 0 should be 1, got " + (0 ** 0);

// 1 ** Infinity === NaN (ES spec divergence from IEEE 754)
if (!(isNaN(1 ** Infinity))) throw "FAIL: 1 ** Infinity should be NaN";

// 1 ** (-Infinity) === NaN
if (!(isNaN(1 ** (-Infinity)))) throw "FAIL: 1 ** (-Infinity) should be NaN";

// (-1) ** Infinity === NaN
if (!(isNaN((-1) ** Infinity))) throw "FAIL: (-1) ** Infinity should be NaN";

// (-1) ** (-Infinity) === NaN
if (!(isNaN((-1) ** (-Infinity)))) throw "FAIL: (-1) ** (-Infinity) should be NaN";

// 2 ** Infinity === Infinity
if (2 ** Infinity !== Infinity) throw "FAIL: 2 ** Infinity should be Infinity";

// 2 ** (-Infinity) === 0
if (2 ** (-Infinity) !== 0) throw "FAIL: 2 ** (-Infinity) should be 0";

// 0.5 ** Infinity === 0
if (0.5 ** Infinity !== 0) throw "FAIL: 0.5 ** Infinity should be 0";

// 0.5 ** (-Infinity) === Infinity
if (0.5 ** (-Infinity) !== Infinity) throw "FAIL: 0.5 ** (-Infinity) should be Infinity";

// Right-associativity: 2 ** 3 ** 2 = 2 ** 9 = 512
if (2 ** 3 ** 2 !== 512) throw "FAIL: 2 ** 3 ** 2 should be 512, got " + (2 ** 3 ** 2);

// **= compound assignment
var a = 2;
a **= 10;
if (a !== 1024) throw "FAIL: 2 **= 10 should be 1024";

// NaN ** non-zero === NaN
if (!isNaN(NaN ** 2)) throw "FAIL: NaN ** 2 should be NaN";

// x ** NaN === NaN
if (!isNaN(2 ** NaN)) throw "FAIL: 2 ** NaN should be NaN";

console.log("PASS");

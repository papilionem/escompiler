// ToString type coercion edge cases
// @expected-stdout-begin
// null
// undefined
// true
// false
// 42
// NaN
// Infinity
// -Infinity
// 0
// @expected-stdout-end
console.log(String(null));        // ToString(null) = "null"
console.log(String(undefined));   // ToString(undefined) = "undefined"
console.log(String(true));        // ToString(true) = "true"
console.log(String(false));       // ToString(false) = "false"
console.log(String(42));          // ToString(42) = "42"
console.log(String(NaN));         // ToString(NaN) = "NaN"
console.log(String(Infinity));    // ToString(Infinity) = "Infinity"
console.log(String(-Infinity));   // ToString(-Infinity) = "-Infinity"
console.log(String(-0));          // ToString(-0) = "0"

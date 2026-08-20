// @expected-stdout-begin
// Infinity
// -Infinity
// NaN
// 1.7976931348623157e+308
// 5e-324
// 2.220446049250313e-16
// 9007199254740991
// -9007199254740991
// NaN
// @expected-stdout-end
console.log(Infinity);
console.log(-Infinity);
console.log(NaN);
console.log(Number.MAX_VALUE);
console.log(Number.MIN_VALUE);
console.log(Number.EPSILON);
console.log(Number.MAX_SAFE_INTEGER);
console.log(Number.MIN_SAFE_INTEGER);
console.log(Number.NaN);

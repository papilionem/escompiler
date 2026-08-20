// @expected-stdout-begin
// true
// false
// true
// false
// @expected-stdout-end
console.log(Number.isInteger(42));
console.log(Number.isInteger(3.14));
console.log(Number.isFinite(100));
console.log(Number.isFinite(Infinity));

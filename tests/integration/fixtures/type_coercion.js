// @expected-stdout-begin
// 0
// 42
// 0
// NaN
// true
// false
// true
// false
// false
// true
// @expected-stdout-end
console.log(Number());
console.log(Number(42));
console.log(Number(false));
console.log(Number("abc"));
console.log(Boolean(1));
console.log(Boolean(0));
console.log(Boolean("hello"));
console.log(Boolean(""));
console.log(Boolean(null));
console.log(Boolean(undefined === undefined));

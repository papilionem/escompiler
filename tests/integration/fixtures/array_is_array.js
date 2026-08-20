// @expected-stdout-begin
// true
// false
// false
// false
// false
// @expected-stdout-end
console.log(Array.isArray([1, 2]));
console.log(Array.isArray("str"));
console.log(Array.isArray(123));
console.log(Array.isArray({}));
console.log(Array.isArray(undefined));

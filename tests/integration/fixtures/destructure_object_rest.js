// Test object destructuring with rest element
// @expected-stdout-begin
// 1
// 2
// 3
// @expected-stdout-end
let { a, ...rest } = { a: 1, b: 2, c: 3 };
console.log(a);
console.log(rest.b);
console.log(rest.c);

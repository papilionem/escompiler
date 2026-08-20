// @expected-stdout-begin
// 5
// HELLO
// hello
// ell
// @expected-stdout-end
let s = "hello";
console.log(s.length);
console.log(s.toUpperCase());
console.log(s.toLowerCase());
console.log(s.slice(1, 4));

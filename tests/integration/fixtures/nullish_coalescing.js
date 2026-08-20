// @expected-stdout-begin
// default
// hello
// 0
// @expected-stdout-end
let a = null ?? "default";
console.log(a);
let b = "hello" ?? "world";
console.log(b);
let c = 0 ?? 42;
console.log(c);

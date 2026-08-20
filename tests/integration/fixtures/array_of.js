// @expected-stdout-begin
// 3
// 1
// 2
// 3
// 1
// hello
// @expected-stdout-end
var a = Array.of(1, 2, 3);
console.log(a.length);
console.log(a[0]);
console.log(a[1]);
console.log(a[2]);
var b = Array.of("hello");
console.log(b.length);
console.log(b[0]);

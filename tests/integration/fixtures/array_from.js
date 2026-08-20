// @expected-stdout-begin
// 3
// a
// b
// c
// 3
// 1
// 2
// 3
// @expected-stdout-end
var a = Array.from("abc");
console.log(a.length);
console.log(a[0]);
console.log(a[1]);
console.log(a[2]);
var b = Array.from([1, 2, 3]);
console.log(b.length);
console.log(b[0]);
console.log(b[1]);
console.log(b[2]);

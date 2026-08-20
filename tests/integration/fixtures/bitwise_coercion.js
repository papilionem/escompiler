// @expected-stdout-begin
// 1
// 0
// 2
// 0
// 7
// 5
// @expected-stdout-end
var x = "5" & 3;
console.log(x);
var y = null | 0;
console.log(y);
var z = true << 1;
console.log(z);
var w = undefined ^ 0;
console.log(w);
var a = "3" | "5";
console.log(a);
var b = 7 & "5";
console.log(b);

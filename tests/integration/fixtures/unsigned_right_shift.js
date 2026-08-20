// @expected-stdout-begin
// 4294967295
// 4
// 0
// 2147483647
// @expected-stdout-end
var x = -1 >>> 0;
console.log(x);
var y = "8" >>> 1;
console.log(y);
var z = 0 >>> 0;
console.log(z);
var w = -1 >>> 1;
console.log(w);

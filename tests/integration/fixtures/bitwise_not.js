// @expected-stdout-begin
// -1
// 0
// -6
// -1
// -1
// @expected-stdout-end
var x = ~0;
console.log(x);
var y = ~-1;
console.log(y);
var z = ~"5";
console.log(z);
var w = ~null;
console.log(w);
var v = ~undefined;
console.log(v);

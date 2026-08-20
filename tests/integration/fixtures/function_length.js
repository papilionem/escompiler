// @expected-stdout-begin
// 3
// 1
// 0
// 1
// @expected-stdout-end
function a(x, y, z) {}
console.log(a.length);
function b(x, y = 1) {}
console.log(b.length);
function c() {}
console.log(c.length);
function d(x, ...rest) {}
console.log(d.length);

// @expected-stdout-begin
// 3
// a
// b
// c
// @expected-stdout-end
let parts = "a,b,c".split(",");
console.log(parts.length);
console.log(parts[0]);
console.log(parts[1]);
console.log(parts[2]);

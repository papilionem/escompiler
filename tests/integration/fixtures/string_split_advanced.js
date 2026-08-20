// @expected-stdout-begin
// 3
// a
// b
// c
// 5
// h
// e
// l
// l
// o
// 1
// hello
// 2
// a
// b
// @expected-stdout-end
var parts = "a,b,c".split(",");
console.log(parts.length);
console.log(parts[0]);
console.log(parts[1]);
console.log(parts[2]);
var chars = "hello".split("");
console.log(chars.length);
console.log(chars[0]);
console.log(chars[1]);
console.log(chars[2]);
console.log(chars[3]);
console.log(chars[4]);
var noSep = "hello".split();
console.log(noSep.length);
console.log(noSep[0]);
var limited = "a,b,c,d".split(",", 2);
console.log(limited.length);
console.log(limited[0]);
console.log(limited[1]);

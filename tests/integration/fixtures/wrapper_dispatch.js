// @expected-stdout-begin
// h
// o
// 5
// true
// false
// true
// false
// @expected-stdout-end
console.log("hello"[0]);
console.log("hello"[4]);
console.log("hello".length);
console.log(true.toString());
console.log(false.toString());
console.log(true.valueOf());
console.log(false.valueOf());

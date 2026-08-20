// @expected-stdout: first second
const arr = ["first", "second", "third"];
const { 0: a, 1: b } = arr;
console.log(a, b);

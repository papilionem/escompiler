// @expected-stdout: 3
let obj = { a: 1, b: 2 };
let merged = { ...obj, c: 3 };
console.log(merged.c);

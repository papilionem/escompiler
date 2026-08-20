// @expected-stdout: 10 20 30
const key = "b";
const obj = { a: 10, b: 20, c: 30 };
const { a, [key]: val, c } = obj;
console.log(a, val, c);

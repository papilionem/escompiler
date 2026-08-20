// @expected-stdout: 1 42 hello
const obj = { a: { x: 1 }, c: "hello" };
const { a: { x }, b: { y } = { y: 42 }, c } = obj;
console.log(x, y, c);

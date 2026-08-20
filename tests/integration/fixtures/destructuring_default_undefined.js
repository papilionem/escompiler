// @expected-stdout: 42 null
const { a = 42, b = "default" } = { a: undefined, b: null };
console.log(a, b);

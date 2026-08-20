// @expected-stdout: 42
let key = "hello";
let obj = { [key]: 42 };
console.log(obj.hello);

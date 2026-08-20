// @expected-stdout: 42
let obj = {};
Object.defineProperty(obj, 'x', { value: 42, writable: false });
console.log(obj.x);

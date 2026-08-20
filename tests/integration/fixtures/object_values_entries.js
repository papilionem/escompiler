// Test Object.values and Object.entries
// @expected-stdout-begin
// 1
// 2
// 2
// @expected-stdout-end

let obj = { a: 1, b: 2 };
let vals = Object.values(obj);
console.log(vals[0]);
console.log(vals[1]);
console.log(Object.entries(obj).length);

// @expected-stdout: 0,1,2
let obj = { [Symbol.iterator]() { let i = 0; return { next() { return i < 3 ? { value: i++, done: false } : { done: true }; } }; } };
let arr = [];
for (let v of obj) arr.push(v);
console.log(arr.join(","));

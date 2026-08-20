// @expected-stdout-begin
// 10
// 20
// 30
// @expected-stdout-end
// Mix of static object literals and dynamic objects in the same program.
// Static literals use CreateObjectLiteral; computed keys fall back.
let a = {x: 10, y: 20};
console.log(a.x);
console.log(a.y);

// Dynamic object (computed key) falls back to CreateObject + SetProp
let key = "z";
let b = {};
b[key] = 30;
console.log(b.z);

// @expected-stdout-begin
// a
// b
// c
// 3
// @expected-stdout-end
var obj = { a: 1, b: 2, c: 3 };
var keys = Reflect.ownKeys(obj);
console.log(keys[0]);
console.log(keys[1]);
console.log(keys[2]);
console.log(keys.length);

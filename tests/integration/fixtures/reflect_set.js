// @expected-stdout-begin
// true
// 99
// @expected-stdout-end
var obj = { x: 1 };
var result = Reflect.set(obj, "x", 99);
console.log(result);
console.log(obj.x);

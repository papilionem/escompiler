// @expected-stdout-begin
// true
// 42
// @expected-stdout-end
var obj = {};
var result = Reflect.defineProperty(obj, "x", { value: 42, writable: true, configurable: true, enumerable: true });
console.log(result);
console.log(obj.x);

// @expected-stdout-begin
// true
// false
// @expected-stdout-end
var obj = { x: 1 };
var result = Reflect.preventExtensions(obj);
console.log(result);
console.log(Reflect.isExtensible(obj));

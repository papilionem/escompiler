// @expected-stdout-begin
// true
// false
// @expected-stdout-end
var obj = { x: 1 };
console.log(Reflect.isExtensible(obj));
Object.preventExtensions(obj);
console.log(Reflect.isExtensible(obj));

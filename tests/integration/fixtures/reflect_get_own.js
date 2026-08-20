// @expected-stdout-begin
// 42
// true
// undefined
// @expected-stdout-end
var obj = { x: 42 };
var desc = Reflect.getOwnPropertyDescriptor(obj, "x");
console.log(desc.value);
console.log(desc.writable);
var missing = Reflect.getOwnPropertyDescriptor(obj, "y");
console.log(typeof missing);

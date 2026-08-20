// @expected-stdout-begin
// true
// undefined
// @expected-stdout-end
var obj = { x: 1, y: 2 };
var result = Reflect.deleteProperty(obj, "x");
console.log(result);
console.log(typeof obj.x);

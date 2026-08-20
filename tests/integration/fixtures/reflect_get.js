// @expected-stdout-begin
// true
// 42
// hello
// @expected-stdout-end
var obj = { x: 42, y: "hello" };
console.log(Reflect.get(obj, "x") === obj.x);
console.log(Reflect.get(obj, "x"));
console.log(Reflect.get(obj, "y"));

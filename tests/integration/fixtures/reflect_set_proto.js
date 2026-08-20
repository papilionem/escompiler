// @expected-stdout-begin
// true
// hello
// @expected-stdout-end
var parent = { greet: "hello" };
var obj = {};
var result = Reflect.setPrototypeOf(obj, parent);
console.log(result);
console.log(obj.greet);

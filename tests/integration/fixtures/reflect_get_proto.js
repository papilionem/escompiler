// @expected-stdout-begin
// true
// null
// @expected-stdout-end
var parent = { greet: "hello" };
var child = Object.create(parent);
console.log(Reflect.getPrototypeOf(child) === parent);
console.log(Reflect.getPrototypeOf(Object.prototype));

// @expected-stdout-begin
// 6
// hello world
// @expected-stdout-end
function sum(a, b, c) { return a + b + c; }
console.log(Reflect.apply(sum, undefined, [1, 2, 3]));
function greet(name) { return this.prefix + " " + name; }
var ctx = { prefix: "hello" };
console.log(Reflect.apply(greet, ctx, ["world"]));

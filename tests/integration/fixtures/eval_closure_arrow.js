// @expected-stdout-begin
// 10
// 30
// @expected-stdout-end
// eval inside arrow function captures outer scope
var x = 10;
var f = () => eval("x");
console.log(f());
var g = (a) => eval("a + x + 10");
console.log(g(10));

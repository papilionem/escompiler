// @expected-stdout: 42
// eval inside eval
var x = 42;
console.log(eval("eval('x')"));

// @expected-stdout: 100
// eval creates function, that function uses eval
eval("function inner() { return eval('100'); }");
console.log(inner());

// Test logical assignment operators with short-circuit
// @expected-stdout-begin
// 42
// 42
// default
// @expected-stdout-end

let a = 1;
a &&= 42;  // a is truthy, so RHS evaluates: a = 42
console.log(a);

let b = 0;
b ||= 42;
console.log(b);

let c = null;
c ??= "default";
console.log(c);

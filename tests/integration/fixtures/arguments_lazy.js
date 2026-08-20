// @expected-stdout-begin
// 3
// 3
// 14
// 42
// @expected-stdout-end

// Function that uses arguments
function f() { return arguments.length; }
console.log(f(1, 2, 3));

// Function that does NOT use arguments (should skip CreateArguments)
function g(a, b) { return a + b; }
console.log(g(1, 2));

// Nested: inner references arguments, outer doesn't
function outer(x) {
  function inner() { return arguments.length; }
  return inner(1, 2, 3, 4) + x;
}
console.log(outer(10));

// Arrow function — never has arguments (inherits from parent)
function h() {
  var arrow = function() { return 42; };
  return arrow();
}
console.log(h());

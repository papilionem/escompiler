// Object coercion in comparison operators with valueOf
// @expected-stdout-begin
// true
// false
// true
// @expected-stdout-end
var a = {valueOf: function() { return 1; }};
var b = {valueOf: function() { return 2; }};
console.log(a < b);    // true (1 < 2)
console.log(a > b);    // false (1 > 2)
console.log(a == 1);   // true (valueOf returns 1)

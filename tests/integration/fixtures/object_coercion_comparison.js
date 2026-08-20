// @expected-stdout-begin
// 6
// true
// false
// hello world
// @expected-stdout-end
var obj = { valueOf: function() { return 5; } };
console.log(obj + 1);
var obj2 = { valueOf: function() { return 1; } };
console.log(obj2 == 1);
console.log(obj2 == 2);
var obj3 = { toString: function() { return "hello"; } };
console.log(obj3 + " world");

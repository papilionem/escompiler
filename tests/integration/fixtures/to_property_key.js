// @expected-stdout-begin
// 42
// undefined
// 1
// 2
// 3
// @expected-stdout-end
var obj = {};
var key = { toString: function() { return "x"; } };
obj[key] = 42;
console.log(obj.x);
console.log(obj[null]);
obj[null] = 1;
console.log(obj["null"]);
obj[undefined] = 2;
console.log(obj["undefined"]);
obj[true] = 3;
console.log(obj["true"]);

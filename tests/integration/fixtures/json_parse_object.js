// @expected-stdout-begin
// 1
// hello
// @expected-stdout-end
let obj = JSON.parse('{"a":1,"b":"hello"}');
console.log(obj.a);
console.log(obj.b);

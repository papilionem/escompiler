// @expected-stdout-begin
// 1
// true
// 2
// @expected-stdout-end
let m = new Map();
m.set("a", 1);
m.set("b", 2);
console.log(m.get("a"));
console.log(m.has("b"));
console.log(m.size);

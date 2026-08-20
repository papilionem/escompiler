// @expected-stdout-begin
// true
// 2
// @expected-stdout-end
let s = new Set();
s.add(1);
s.add(2);
s.add(2);
console.log(s.has(1));
console.log(s.size);

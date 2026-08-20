// @expected-stdout-begin
// 1
// 2
// 3
// true
// @expected-stdout-end
function* gen() { yield 1; yield 2; yield 3; }
let g = gen();
console.log(g.next().value);
console.log(g.next().value);
console.log(g.next().value);
console.log(g.next().done);

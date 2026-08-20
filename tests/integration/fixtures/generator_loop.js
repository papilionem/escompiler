// @expected-stdout-begin
// 0
// 1
// 2
// 3
// 4
// true
// @expected-stdout-end
function* range(n) {
  for (var i = 0; i < n; i++) {
    yield i;
  }
}
var g = range(5);
console.log(g.next().value);
console.log(g.next().value);
console.log(g.next().value);
console.log(g.next().value);
console.log(g.next().value);
console.log(g.next().done);

// @expected-stdout: 1,2,3
function* gen() {
  yield 1;
  yield 2;
  yield 3;
}
let result = [];
for (let v of gen()) {
  result.push(v);
}
console.log(result.join(","));

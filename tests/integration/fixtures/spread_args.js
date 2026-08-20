// @expected-stdout: 6
function add(a, b, c) {
  return a + b + c;
}
let args = [1, 2, 3];
console.log(add(...args));

// @expected-stdout: 10
function sum(first, ...rest) {
  let s = first;
  for (let i = 0; i < rest.length; i++) {
    s += rest[i];
  }
  return s;
}
console.log(sum(1, 2, 3, 4));

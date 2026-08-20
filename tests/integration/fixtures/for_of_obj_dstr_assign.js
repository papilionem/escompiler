// @expected-stdout: 1,2,3
var x;
var result = [];
for ({x} of [{x: 1}, {x: 2}, {x: 3}]) {
  result.push(x);
}
console.log(result.join(","));

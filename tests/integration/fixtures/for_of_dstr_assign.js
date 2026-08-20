// @expected-stdout: 0,1,2
var x;
var result = [];
for ([x] of [[0], [1], [2]]) {
  result.push(x);
}
console.log(result.join(","));

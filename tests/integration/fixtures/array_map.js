// @expected-stdout: 2,4,6
let result = [1, 2, 3].map(function(x) { return x * 2; });
console.log(result.join(","));

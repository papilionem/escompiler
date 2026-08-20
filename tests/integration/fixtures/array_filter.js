// @expected-stdout: 3,4
let result = [1, 2, 3, 4].filter(function(x) { return x > 2; });
console.log(result.join(","));

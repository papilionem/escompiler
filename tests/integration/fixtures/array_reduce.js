// @expected-stdout: 6
let result = [1, 2, 3].reduce(function(a, b) { return a + b; }, 0);
console.log(result);

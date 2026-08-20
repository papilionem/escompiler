// Test array reduce with real callback
// @expected-stdout: 15

let arr = [1, 2, 3, 4, 5];
let sum = arr.reduce(function(acc, x) { return acc + x; }, 0);
console.log(sum);

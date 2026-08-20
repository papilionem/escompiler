// Test array destructuring assignment
// @expected-stdout: 1 2 3

let a, b, c;
let arr = [1, 2, 3];
[a, b, c] = arr;
console.log(a, b, c);

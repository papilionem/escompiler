// @expected-stdout: 1 2,3,4
const arr = [1, 2, 3, 4];
const [first, ...rest] = arr;
console.log(first, rest.join(","));

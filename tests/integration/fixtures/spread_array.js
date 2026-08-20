// @expected-stdout: 1,2,3,4,5
var a = [1, 2];
var b = [3, 4, 5];
var c = [...a, ...b];
console.log(c.join(","));

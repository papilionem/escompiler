// @expected-stdout-begin
// 3
// 1
// 2
// 3
// @expected-stdout-end
let arr = JSON.parse('[1,2,3]');
console.log(arr.length);
console.log(arr[0]);
console.log(arr[1]);
console.log(arr[2]);

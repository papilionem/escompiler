// @expected-stdout-begin
// 3
// 3
// 2
// @expected-stdout-end
let arr = [1, 2];
arr.push(3);
console.log(arr.length);
console.log(arr.pop());
console.log(arr.length);

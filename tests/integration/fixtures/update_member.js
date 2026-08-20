// Test update expressions on member expressions
// @expected-stdout-begin
// 1
// 2
// 11
// 11
// @expected-stdout-end

let obj = { x: 1 };
console.log(obj.x++);   // postfix: prints 1, then obj.x becomes 2
console.log(obj.x);     // prints 2

let arr = [10];
console.log(++arr[0]);  // prefix: arr[0] becomes 11, prints 11
console.log(arr[0]--);  // postfix: prints 11, then arr[0] becomes 10

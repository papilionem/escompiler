// Test array destructuring with rest element
// @expected-stdout-begin
// 1
// 2,3,4
// @expected-stdout-end
let [first, ...rest] = [1, 2, 3, 4];
console.log(first);
console.log(rest.join(","));

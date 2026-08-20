// Test array find, some, every methods
// @expected-stdout-begin
// 4
// true
// false
// @expected-stdout-end

let arr = [1, 2, 3, 4, 5];
let found = arr.find(function(x) { return x > 3; });
console.log(found);

let hasBig = arr.some(function(x) { return x > 4; });
console.log(hasBig);

let allPositive = arr.every(function(x) { return x > 2; });
console.log(allPositive);

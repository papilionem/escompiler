// @expected-stdout-begin
// {"a":1}
// [1,2,3]
// null
// true
// 42
// "hello"
// @expected-stdout-end
let obj = {a: 1};
console.log(JSON.stringify(obj));
console.log(JSON.stringify([1, 2, 3]));
console.log(JSON.stringify(null));
console.log(JSON.stringify(true));
console.log(JSON.stringify(42));
console.log(JSON.stringify("hello"));

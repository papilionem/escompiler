// @expected-stdout-begin
// true
// true
// true
// true
// true
// true
// true
// @expected-stdout-end
console.log(typeof 42 === "number");
console.log(typeof "hello" === "string");
console.log(typeof true === "boolean");
console.log(typeof undefined === "undefined");
console.log(typeof null === "object");
console.log(typeof function() {} === "function");
console.log(typeof {} === "object");

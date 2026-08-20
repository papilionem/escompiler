// @expected-stdout-begin
// true
// true
// false
// false
// @expected-stdout-end
var obj = { a: 1, b: 2 };
console.log(Reflect.has(obj, "a"));
console.log("a" in obj);
console.log(Reflect.has(obj, "z"));
console.log("z" in obj);

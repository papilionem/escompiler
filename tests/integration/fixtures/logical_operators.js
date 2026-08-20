// @expected-stdout-begin
// true
// false
// hello
// default
// @expected-stdout-end
console.log(true || false);
console.log(true && false);
console.log("hello" || "world");
console.log(null || "default");

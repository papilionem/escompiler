// @expected-stdout-begin
// true
// false
// @expected-stdout-end
function regular() {}
var arrow = () => {};
console.log(typeof regular.prototype === "object");
console.log("prototype" in arrow);

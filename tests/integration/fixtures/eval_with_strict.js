// @expected-stdout-begin
// 42
// false
// @expected-stdout-end
// strict eval inside with gets its own variable scope
var obj = { x: 42 };
with (obj) {
    eval('"use strict"; var strictVar = 100;');
    console.log(eval('"use strict"; x'));
}
console.log(typeof strictVar !== "undefined");

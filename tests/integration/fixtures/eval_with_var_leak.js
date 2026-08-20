// @expected-stdout-begin
// 10
// 10
// @expected-stdout-end
// eval creates var that leaks through with scope to function scope
var obj = { a: 1 };
function test() {
    with (obj) {
        eval("var leaked = 10");
    }
    return leaked;
}
console.log(test());
console.log(eval("var outerLeak = 10; outerLeak"));

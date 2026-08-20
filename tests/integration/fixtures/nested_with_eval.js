// @expected-stdout-begin
// 2
// 20
// @expected-stdout-end
// with inside with, eval inside inner reads correct scope
var outer = { x: 1, y: 10 };
var inner = { x: 2, y: 20 };
with (outer) {
    with (inner) {
        console.log(eval("x"));
        console.log(eval("y"));
    }
}

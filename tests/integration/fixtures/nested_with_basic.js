// @expected-stdout-begin
// inner_x
// inner_y
// @expected-stdout-end
// with inside with, inner shadows outer
var outer = { x: "outer_x", y: "outer_y" };
var inner = { x: "inner_x", y: "inner_y" };
with (outer) {
    with (inner) {
        console.log(x);
        console.log(y);
    }
}

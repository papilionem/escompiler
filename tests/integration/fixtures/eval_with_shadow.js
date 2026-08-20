// @expected-stdout-begin
// 999
// 5
// @expected-stdout-end
// with-object shadows outer variable, eval reads the shadowed one
var x = 5;
var obj = { x: 999 };
with (obj) {
    console.log(eval("x"));
}
console.log(x);

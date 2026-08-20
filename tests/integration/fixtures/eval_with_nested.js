// @expected-stdout-begin
// inner
// 99
// @expected-stdout-end
// nested with inside eval
var outer = { name: "outer" };
var inner = { name: "inner", value: 99 };
with (outer) {
    eval("with (inner) { console.log(name); console.log(value); }");
}

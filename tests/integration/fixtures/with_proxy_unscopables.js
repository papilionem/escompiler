// @expected-stdout-begin
// outer
// on-obj
// @expected-stdout-end
var a = "outer";
var b = "outer";
var obj = {
    a: "on-obj",
    b: "on-obj"
};
obj[Symbol.unscopables] = { a: true };
with (obj) {
    // a is excluded by unscopables, so resolves to outer var
    console.log(a);
    // b is not excluded, so resolves to obj.b
    console.log(b);
}

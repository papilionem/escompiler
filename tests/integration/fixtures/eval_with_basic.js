// @expected-stdout: 42
// eval inside with statement reads with-object properties
var obj = { x: 42 };
with (obj) {
    console.log(eval("x"));
}

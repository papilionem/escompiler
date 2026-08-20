// @expected-stdout: 42
// eval creates closure that captures with-scope variable
var obj = { value: 42 };
var fn;
with (obj) {
    eval("fn = function() { return value; }");
}
console.log(fn());

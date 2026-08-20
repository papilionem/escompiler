// @expected-stdout: true
// eval inside with, `this` still refers to the enclosing this binding
var obj = { a: 1 };
var outerThis = this;
with (obj) {
    var result = eval("this");
    console.log(result === outerThis);
}

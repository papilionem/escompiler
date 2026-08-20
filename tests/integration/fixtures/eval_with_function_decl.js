// @expected-stdout: hello from eval
// eval declares function inside with block
var obj = { msg: "hello from eval" };
with (obj) {
    eval("function greet() { return msg; }");
    console.log(greet());
}

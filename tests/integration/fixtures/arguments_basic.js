// @expected-stdout-begin
// 3
// 10
// 20
// 30
// @expected-stdout-end
function f(a, b, c) {
    console.log(arguments.length);
    console.log(arguments[0]);
    console.log(arguments[1]);
    console.log(arguments[2]);
}
f(10, 20, 30);

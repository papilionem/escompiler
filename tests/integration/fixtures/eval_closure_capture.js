// @expected-stdout: 42
// eval inside closure reads captured variable
function outer() {
    var x = 42;
    var inner = function() {
        return eval("x");
    };
    return inner();
}
console.log(outer());

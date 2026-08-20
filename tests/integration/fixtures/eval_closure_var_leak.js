// @expected-stdout: 77
// eval inside closure, var leaks to enclosing function scope
function outer() {
    var inner = function() {
        eval("var leaked = 77");
    };
    inner();
    return typeof leaked;
}
// var in eval inside closure leaks to the closure's function scope,
// not to outer. The closure sees it; outer does not.
function test() {
    var fn2 = function() {
        eval("var v = 77");
        return v;
    };
    console.log(fn2());
}
test();

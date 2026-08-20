// @expected-stdout: 30
// nested closures with eval
function a() {
    var x = 10;
    function b() {
        var y = 20;
        function c() {
            return eval("x + y");
        }
        return c();
    }
    return b();
}
console.log(a());

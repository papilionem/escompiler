// @expected-stdout: 42
// Return from catch handler
function f(o) {
    function innerf(o, x) {
        try {
            throw o;
        } catch (e) {
            return x;
        }
    }
    return innerf(o, 42);
}
console.log(f({}));

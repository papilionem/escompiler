// @expected-stdout: finally
// finally return overrides catch return
function f() {
    try {
        throw "err";
    } catch (e) {
        return "catch";
    } finally {
        return "finally";
    }
}
console.log(f());

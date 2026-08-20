// @expected-stdout: finally
// finally return overrides try return
function f() {
    try {
        return "try";
    } finally {
        return "finally";
    }
}
console.log(f());

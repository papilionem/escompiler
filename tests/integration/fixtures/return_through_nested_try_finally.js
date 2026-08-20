// @expected-stdout-begin
// inner finally
// outer finally
// 42
// @expected-stdout-end
// Return crossing two try-finally boundaries must execute both finally
// blocks and return the original value.
function f() {
    try {
        try {
            return 42;
        } finally {
            console.log("inner finally");
        }
    } finally {
        console.log("outer finally");
    }
}
console.log(f());

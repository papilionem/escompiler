// @expected-stdout-begin
// finally
// @expected-stdout-end
// finally block should execute even when try has a return
function f() {
    try {
        return "try";
    } finally {
        console.log("finally");
    }
}
f();

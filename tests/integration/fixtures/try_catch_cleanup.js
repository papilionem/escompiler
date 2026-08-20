// Test that catch_end cleanup works correctly with nested try/catch
// @expected-stdout-begin
// inner caught: inner error
// after inner try
// outer caught: outer error
// done
// @expected-stdout-end
try {
    try {
        throw "inner error";
    } catch (e) {
        console.log("inner caught:", e);
    }
    console.log("after inner try");
    throw "outer error";
} catch (e) {
    console.log("outer caught:", e);
}
console.log("done");

// @expected-stdout-begin
// inner caught: inner error
// outer ok
// @expected-stdout-end
// Nested try/catch without finally blocks
try {
    try {
        throw "inner error";
    } catch (e) {
        console.log("inner caught:", e);
    }
    console.log("outer ok");
} catch (e) {
    console.log("outer caught:", e);
}

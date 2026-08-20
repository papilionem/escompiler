// @expected-stdout-begin
// inner caught: inner error
// outer caught: outer error
// done
// @expected-stdout-end
// Nested try/catch: exceptions should route to the correct catch handler.
try {
    try {
        throw "inner error";
    } catch (e) {
        console.log("inner caught:", e);
    }
    throw "outer error";
} catch (e) {
    console.log("outer caught:", e);
}
console.log("done");

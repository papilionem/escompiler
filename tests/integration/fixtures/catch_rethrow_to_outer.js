// @expected-stdout-begin
// outer caught: rethrown
// done
// @expected-stdout-end
// Rethrowing in a catch handler should route to the outer try/catch,
// not loop back to the same catch handler.
try {
    try {
        throw "rethrown";
    } catch (e) {
        throw e;
    }
} catch (e) {
    console.log("outer caught:", e);
}
console.log("done");

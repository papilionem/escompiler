// @expected-stdout-begin
// outer caught: original
// @expected-stdout-end
// Catch rethrows, outer catch catches it
try {
    try {
        throw "original";
    } catch (e) {
        throw e;
    }
} catch (e) {
    console.log("outer caught:", e);
}

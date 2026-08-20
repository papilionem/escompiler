// @expected-stdout-begin
// finally ran
// outer caught: original
// @expected-stdout-end
// Catch rethrows, finally runs, outer catches
try {
    try {
        throw "original";
    } catch (e) {
        throw e;
    } finally {
        console.log("finally ran");
    }
} catch (e) {
    console.log("outer caught:", e);
}

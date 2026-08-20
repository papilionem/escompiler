// @expected-stdout-begin
// inner: ex2
// finally
// done
// @expected-stdout-end
// Nested try-catch inside a catch body with finally on the outer try.
// The inner throw must go to the inner catch, not loop through the
// outer finally block.
try {
    throw "ex1";
} catch (er1) {
    try {
        throw "ex2";
    } catch (er2) {
        console.log("inner:", er2);
    }
} finally {
    console.log("finally");
}
console.log("done");

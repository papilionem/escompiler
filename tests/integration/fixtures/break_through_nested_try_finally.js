// @expected-stdout-begin
// inner finally
// outer finally
// after
// @expected-stdout-end
// Labeled break crossing two try-finally boundaries must execute both
// finally blocks in inner-to-outer order.
outer: {
    try {
        try {
            break outer;
        } finally {
            console.log("inner finally");
        }
    } finally {
        console.log("outer finally");
    }
}
console.log("after");

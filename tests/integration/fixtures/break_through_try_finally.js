// @expected-stdout-begin
// finally
// after
// @expected-stdout-end
// Labeled break through try-finally should execute the finally block.
outer: {
    try {
        break outer;
    } finally {
        console.log("finally");
    }
}
console.log("after");

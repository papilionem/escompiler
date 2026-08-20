// @expected-stdout-begin
// outer caught: outer
// inner caught: inner
// after inner try
// done
// @expected-stdout-end
// Nested try-catch inside a catch body (exercises catch_block_targets
// self-reference avoidance so the inner throw goes to the inner catch
// handler rather than escaping to the function exit).
try {
    throw "outer";
} catch (e1) {
    console.log("outer caught:", e1);
    try {
        throw "inner";
    } catch (e2) {
        console.log("inner caught:", e2);
    }
    console.log("after inner try");
}
console.log("done");

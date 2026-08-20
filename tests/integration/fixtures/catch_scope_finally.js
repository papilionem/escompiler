// @expected-stdout-begin
// caught: thrown
// finally
// outer
// @expected-stdout-end
// Catch parameter scoping with finally: `e` should not leak.
let e = "outer";
try {
    throw "thrown";
} catch (e) {
    console.log("caught:", e);
} finally {
    console.log("finally");
}
console.log(e);

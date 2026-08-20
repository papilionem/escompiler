// @expected-stdout-begin
// outer
// caught: thrown
// outer
// @expected-stdout-end
// Catch parameter should not leak into the outer scope.
// After the catch block, `e` should still be "outer".
let e = "outer";
console.log(e);
try {
    throw "thrown";
} catch (e) {
    console.log("caught:", e);
}
console.log(e);

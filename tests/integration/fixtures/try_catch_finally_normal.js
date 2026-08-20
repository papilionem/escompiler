// @expected-stdout-begin
// try body
// finally body
// after
// @expected-stdout-end
// try-catch-finally without throw (normal flow, catch not executed)
try {
    console.log("try body");
} catch (e) {
    console.log("catch body");
} finally {
    console.log("finally body");
}
console.log("after");

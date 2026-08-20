// @expected-stdout-begin
// caught: boom
// finally body
// after
// @expected-stdout-end
// try-catch-finally with throw in try (catch + finally both execute)
try {
    throw "boom";
} catch (e) {
    console.log("caught:", e);
} finally {
    console.log("finally body");
}
console.log("after");

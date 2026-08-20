// @expected-stdout-begin
// caught: hello
// after try
// @expected-stdout-end
// Basic try-catch without finally
try {
    throw "hello";
} catch (e) {
    console.log("caught:", e);
}
console.log("after try");
